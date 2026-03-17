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

use teloxide::prelude::*;
use teloxide::types::{ChatAction, ChatId, MessageId, ThreadId};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{error, instrument};

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

    /// Processes an incoming Telegram message.
    ///
    /// 1. Registers user profile via UserStore
    /// 2. Records chat_id <-> user_id mapping
    /// 3. Creates and queues TextMessage for stream publication
    /// 4. Starts ack timer
    #[instrument(skip(self, bot, msg))]
    pub async fn handle_incoming(&self, bot: &Bot, msg: &Message) -> Result<(), TelegramError> {
        let chat_id = msg.chat.id.0;
        let text = msg.text().unwrap_or("").to_string();
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
