//! Telegram surface core — connects Telegram bot to Wheelhouse streams.
//!
//! Handles:
//! - Incoming Telegram messages -> user registration + TextMessage publish
//! - Outgoing TextMessages from stream -> Telegram chat delivery
//! - Error sanitization (RT-B1)
//! - Ack timeout ("Working on it...")

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{ChatAction, ChatId, MessageId, ThreadId};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{error, info, instrument, warn};

/// Maximum attachment size accepted by the Library ingest path.
///
/// 5 MiB is the Library's practical ingest budget: Telegram's free-bot
/// `getFile` API caps at 20 MiB, LLM context economics make pages >500 KiB
/// after extraction a bad tradeoff, and our transport carries the bytes
/// inline in a proto3 message. Files larger than this limit are rejected
/// surface-side with a clear user-visible error before the envelope is
/// built — we never pay the download cost for oversized files.
const MAX_ATTACHMENT_BYTES: u32 = 5 * 1024 * 1024;

/// Classify a document's MIME type against the Library ingest allow-list.
///
/// Returns `true` for types the 13-7 / 13-8 / 13-9 ingest pipeline can
/// currently handle: PDFs (`application/pdf`), plain text, and markdown
/// in its various reported forms. Other types (Office documents, images,
/// archives, etc.) are not yet ingestable — the surface still forwards
/// them so the agent can decide, but we log a warning when we see one.
fn is_ingestable_mime(mime: &str) -> bool {
    matches!(
        mime,
        "application/pdf"
            | "text/plain"
            | "text/markdown"
            | "text/x-markdown"
            | "text/x-web-markdown"
    )
}

use wh_proto::TextMessage;
use wh_user::UserStore;

use crate::config::TelegramConfig;
use crate::error::{sanitize_for_user, TelegramError};
use crate::mapping::ChatMapping;
use crate::routing::RoutingTable;

/// The Telegram surface connects Telegram users to Wheelhouse streams.
pub struct TelegramSurface {
    config: TelegramConfig,
    user_store: Arc<UserStore>,
    chat_mapping: Arc<Mutex<ChatMapping>>,
    /// Routing table for multi-chat mode: tracks user -> (chat_id, thread_id).
    routing: Arc<Mutex<RoutingTable>>,
    /// Per-user cancellation senders for the typing indicator loop.
    typing_cancel: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
    /// Channel for outbound messages (TextMessages to publish to stream).
    outbound_tx: mpsc::UnboundedSender<TextMessage>,
    /// Channel receiver for outbound messages.
    /// Consumed by the stream publication loop via `take_outbound_rx()`.
    outbound_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<TextMessage>>>>,
}

impl TelegramSurface {
    /// Creates a new Telegram surface.
    #[instrument(skip_all)]
    pub fn new(
        config: TelegramConfig,
        user_store: UserStore,
        chat_mapping: ChatMapping,
        routing: RoutingTable,
    ) -> Self {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();

        Self {
            config,
            user_store: Arc::new(user_store),
            chat_mapping: Arc::new(Mutex::new(chat_mapping)),
            routing: Arc::new(Mutex::new(routing)),
            typing_cancel: Arc::new(Mutex::new(HashMap::new())),
            outbound_tx,
            outbound_rx: Arc::new(Mutex::new(Some(outbound_rx))),
        }
    }

    /// Returns a clone of the outbound sender for publishing messages to stream.
    pub fn outbound_sender(&self) -> mpsc::UnboundedSender<TextMessage> {
        self.outbound_tx.clone()
    }

    /// Takes the outbound receiver for the stream publication loop.
    ///
    /// Can only be called once — subsequent calls return `None`.
    /// The runner binary calls this to drain outbound messages and publish
    /// them to the broker via ZMQ (Story 9.2).
    #[instrument(skip_all)]
    pub fn take_outbound_rx(&self) -> Option<mpsc::UnboundedReceiver<TextMessage>> {
        // Use try_lock to avoid blocking — this is called once at startup
        if let Ok(mut guard) = self.outbound_rx.try_lock() {
            guard.take()
        } else {
            None
        }
    }

    /// Attempt to download a document attachment from an incoming message.
    ///
    /// Returns:
    /// - `Ok(Some((bytes, filename, mime_type)))` — document present and
    ///   within the size cap, downloaded successfully.
    /// - `Ok(None)` — no document attached (plain text or non-document
    ///   message type like photo/voice/video).
    /// - `Err(AttachmentTooLarge)` — document present but over
    ///   [`MAX_ATTACHMENT_BYTES`]. Caller should reply to the user and
    ///   NOT publish an envelope.
    /// - `Err(AttachmentDownloadFailed)` — Telegram API error during
    ///   `get_file` or `download_file`. Propagate.
    ///
    /// Only the `document` message kind is in scope for v1 — photos
    /// (which arrive as resized `PhotoSize` entries), voice, video, and
    /// stickers are intentionally skipped because they have no
    /// meaningful Library ingest mapping.
    async fn try_download_document(
        &self,
        bot: &Bot,
        msg: &Message,
    ) -> Result<Option<(Vec<u8>, String, String)>, TelegramError> {
        let Some(doc) = msg.document() else {
            return Ok(None);
        };

        // Size gate: reject before even touching the Telegram API so we
        // never pay for the download on oversized files.
        if doc.file.size > MAX_ATTACHMENT_BYTES {
            return Err(TelegramError::AttachmentTooLarge {
                size: u64::from(doc.file.size),
                limit: u64::from(MAX_ATTACHMENT_BYTES),
            });
        }

        let file_meta = bot
            .get_file(doc.file.id.clone())
            .await
            .map_err(|e| TelegramError::AttachmentDownloadFailed(format!("get_file: {e}")))?;

        // Re-check the real reported size in case `doc.file.size` was an
        // estimate — Telegram's `getFile` returns the authoritative value.
        if file_meta.size > MAX_ATTACHMENT_BYTES {
            return Err(TelegramError::AttachmentTooLarge {
                size: u64::from(file_meta.size),
                limit: u64::from(MAX_ATTACHMENT_BYTES),
            });
        }

        let mut buffer: Vec<u8> = Vec::with_capacity(file_meta.size as usize);
        bot.download_file(&file_meta.path, &mut buffer)
            .await
            .map_err(|e| TelegramError::AttachmentDownloadFailed(format!("download_file: {e}")))?;

        let filename = doc
            .file_name
            .clone()
            .unwrap_or_else(|| format!("telegram-{}", doc.file.id));
        let mime_type = doc
            .mime_type
            .as_ref()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());

        // Soft-warn on mime types we do not (yet) know how to ingest —
        // the agent-side router will reject them cleanly, but flagging
        // here helps trace the path in the surface logs.
        if !is_ingestable_mime(&mime_type) {
            warn!(
                %filename,
                %mime_type,
                "document mime_type is not a known Library ingest type; \
                 forwarding anyway — agent will decide"
            );
        }

        Ok(Some((buffer, filename, mime_type)))
    }

    /// Processes an incoming Telegram message.
    ///
    /// 1. Registers user profile via UserStore
    /// 2. Records chat_id <-> user_id mapping
    /// 3. Downloads any file attachment (document) if present — capped at
    ///    [`MAX_ATTACHMENT_BYTES`]; oversized files are rejected with a
    ///    user-visible error reply and no envelope is published.
    /// 4. Creates and queues TextMessage (with optional attachment bytes)
    ///    for stream publication
    /// 5. Starts ack timer
    #[instrument(skip(self, bot, msg))]
    pub async fn handle_incoming(&self, bot: &Bot, msg: &Message) -> Result<(), TelegramError> {
        let chat_id = msg.chat.id.0;

        // Read text body: prefer `caption` when a document/photo is present
        // (Telegram puts any accompanying text there), else fall back to
        // `text` for plain-text messages.
        let text = msg
            .caption()
            .or_else(|| msg.text())
            .unwrap_or("")
            .to_string();

        // Attempt to download a file attachment if the message is a
        // document. Photos / voice / video are intentionally out of scope
        // for v1 — only the `document` path is handled since that is how
        // PDFs, markdown files, and text files arrive via Telegram.
        let (attachment_bytes, attachment_filename, attachment_mime_type) =
            match self.try_download_document(bot, msg).await {
                Ok(Some(a)) => (a.0, a.1, a.2),
                Ok(None) => (Vec::new(), String::new(), String::new()),
                Err(TelegramError::AttachmentTooLarge { size, limit }) => {
                    // Tell the user directly without publishing an envelope.
                    let reply = format!(
                        "⚠ File too large for Library ingest ({size} bytes). \
                         Maximum accepted size is {limit} bytes ({} MiB).",
                        limit / (1024 * 1024)
                    );
                    let send_result = bot.send_message(ChatId(chat_id), reply).await;
                    if let Err(e) = send_result {
                        error!(error = %e, "failed to send oversized-file reply");
                    }
                    return Ok(());
                }
                Err(e) => {
                    error!(error = %e, "failed to download Telegram attachment");
                    return Err(e);
                }
            };
        let has_attachment = !attachment_bytes.is_empty();
        if has_attachment {
            info!(
                filename = %attachment_filename,
                mime = %attachment_mime_type,
                size = attachment_bytes.len(),
                "Telegram attachment downloaded; forwarding as TextMessage"
            );
        }

        let display_name = msg
            .from
            .as_ref()
            .map(|u| {
                u.first_name.clone()
                    + &u.last_name
                        .as_ref()
                        .map(|ln| format!(" {ln}"))
                        .unwrap_or_default()
            })
            .unwrap_or_else(|| "Unknown".to_string());

        // Register user profile (deduplicates automatically)
        let profile = self
            .user_store
            .register("telegram", &chat_id.to_string(), &display_name)
            .map_err(|e| {
                error!(error = %e, "failed to register user profile");
                TelegramError::UserStoreError(e)
            })?;

        // Record chat mapping for response routing
        let thread_id: Option<i32> = msg.thread_id.map(|t| t.0 .0);
        {
            let mut mapping = self.chat_mapping.lock().await;
            mapping.register(&profile.user_id, chat_id)?;
        }
        // Record user location and resolve source stream/topic in one lock (Story 10.2)
        let (source_stream, source_topic) = {
            let mut routing = self.routing.lock().await;
            routing.record_user_location(&profile.user_id, chat_id, thread_id);
            match routing.resolve_inbound_with_topic(chat_id, thread_id) {
                Some((stream, topic)) => (stream.to_string(), topic.unwrap_or("").to_string()),
                None => (self.config.stream_name().to_string(), String::new()),
            }
        };

        // Create TextMessage for stream publication
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let text_msg = TextMessage {
            content: text,
            publisher_id: "telegram-surface".to_string(),
            timestamp_ms,
            user_id: profile.user_id.clone(),
            reply_to_user_id: String::new(),
            source_stream,
            source_topic,
            attachment_bytes,
            attachment_filename,
            attachment_mime_type,
        };

        // Queue for stream publication
        self.outbound_tx
            .send(text_msg)
            .map_err(|e| TelegramError::StreamError(e.to_string()))?;

        // Cancel any previous typing indicator for this user, then start a new one.
        // The typing action lasts ~5s on Telegram; we refresh every 4s until cancelled.
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
        {
            let mut cancel_map = self.typing_cancel.lock().await;
            if let Some(prev) = cancel_map.insert(profile.user_id.clone(), cancel_tx) {
                let _ = prev.send(());
            }
        }

        let bot_clone = bot.clone();
        let typing_chat_id = ChatId(chat_id);
        tokio::spawn(async move {
            tokio::pin!(cancel_rx);
            loop {
                if let Err(e) = bot_clone
                    .send_chat_action(typing_chat_id, ChatAction::Typing)
                    .await
                {
                    error!(error = %e, "failed to send typing action");
                    break;
                }
                tokio::select! {
                    _ = &mut cancel_rx => break,
                    _ = tokio::time::sleep(Duration::from_secs(4)) => {}
                }
            }
        });

        Ok(())
    }

    /// Processes an outgoing TextMessage (from stream) and delivers to Telegram.
    ///
    /// Routes based on `reply_to_user_id` field.
    #[instrument(skip(self, bot, text_msg))]
    pub async fn handle_outgoing(
        &self,
        bot: &Bot,
        text_msg: &TextMessage,
    ) -> Result<(), TelegramError> {
        let target_user_id = if text_msg.reply_to_user_id.is_empty() {
            return Err(TelegramError::StreamError(
                "outgoing message has no reply_to_user_id".into(),
            ));
        } else {
            text_msg.reply_to_user_id.as_str()
        };

        // Cancel typing indicator for this user
        if let Some(cancel) = self.typing_cancel.lock().await.remove(target_user_id) {
            let _ = cancel.send(());
        }

        // Look up (chat_id, thread_id) from routing table (preferred) or chat mapping.
        let (chat_id, thread_id) = {
            let routing = self.routing.lock().await;
            if let Some((cid, tid)) = routing.resolve_outbound(target_user_id) {
                (cid, tid)
            } else {
                let mapping = self.chat_mapping.lock().await;
                let cid = mapping.lookup_chat_id(target_user_id).ok_or_else(|| {
                    TelegramError::SendFailed("no chat mapping found for user".into())
                })?;
                (cid, None)
            }
        };

        // Send to Telegram, routing to the correct topic thread when known.
        let mut req = bot.send_message(ChatId(chat_id), &text_msg.content);
        if let Some(tid) = thread_id {
            req = req.message_thread_id(ThreadId(MessageId(tid)));
        }
        req.await.map_err(|e| {
            error!(error = %e, "failed to send Telegram message");
            TelegramError::SendFailed("message delivery failed".into())
        })?;

        Ok(())
    }

    /// Handles errors by sending a sanitized message to the user.
    #[instrument(skip(self, bot))]
    pub async fn send_error_to_user(&self, bot: &Bot, chat_id: i64, err: &TelegramError) {
        error!(error = %err, "telegram surface error");
        let safe_msg = sanitize_for_user(err);
        if let Err(send_err) = bot.send_message(ChatId(chat_id), safe_msg).await {
            error!(error = %send_err, "failed to send error fallback to user");
        }
    }

    /// Returns the config.
    pub fn config(&self) -> &TelegramConfig {
        &self.config
    }
}
