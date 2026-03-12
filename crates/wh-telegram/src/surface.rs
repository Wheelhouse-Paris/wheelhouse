//! Telegram surface core — connects Telegram bot to Wheelhouse streams.
//!
//! Handles:
//! - Incoming Telegram messages -> user registration + TextMessage publish
//! - Outgoing TextMessages from stream -> Telegram chat delivery
//! - Error sanitization (RT-B1)
//! - Ack timeout ("Working on it...")

use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::ChatId;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, instrument};

use wh_proto::TextMessage;
use wh_user::UserStore;

use crate::ack::AckTracker;
use crate::config::TelegramConfig;
use crate::error::{sanitize_for_user, TelegramError};
use crate::mapping::ChatMapping;

/// The Telegram surface connects Telegram users to Wheelhouse streams.
pub struct TelegramSurface {
    config: TelegramConfig,
    user_store: Arc<UserStore>,
    chat_mapping: Arc<Mutex<ChatMapping>>,
    ack_tracker: Arc<AckTracker>,
    /// Channel for outbound messages (TextMessages to publish to stream).
    outbound_tx: mpsc::UnboundedSender<TextMessage>,
    /// Channel receiver for outbound messages.
    /// Consumed by the stream publication loop.
    #[allow(dead_code)]
    outbound_rx: Arc<Mutex<mpsc::UnboundedReceiver<TextMessage>>>,
}

impl TelegramSurface {
    /// Creates a new Telegram surface.
    #[instrument(skip_all)]
    pub fn new(config: TelegramConfig, user_store: UserStore, chat_mapping: ChatMapping) -> Self {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();

        Self {
            config,
            user_store: Arc::new(user_store),
            chat_mapping: Arc::new(Mutex::new(chat_mapping)),
            ack_tracker: Arc::new(AckTracker::new(std::time::Duration::from_secs(5))),
            outbound_tx,
            outbound_rx: Arc::new(Mutex::new(outbound_rx)),
        }
    }

    /// Returns a clone of the outbound sender for publishing messages to stream.
    pub fn outbound_sender(&self) -> mpsc::UnboundedSender<TextMessage> {
        self.outbound_tx.clone()
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
                        .map(|ln| format!(" {}", ln))
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
        {
            let mut mapping = self.chat_mapping.lock().await;
            mapping.register(&profile.user_id, chat_id)?;
        }

        // Create TextMessage for stream publication
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let msg_id = timestamp_ms.to_string();
        let text_msg = TextMessage {
            content: text,
            publisher_id: "telegram-surface".to_string(),
            timestamp_ms,
            user_id: profile.user_id.clone(),
            reply_to_user_id: String::new(),
        };

        // Queue for stream publication
        self.outbound_tx
            .send(text_msg)
            .map_err(|e| TelegramError::StreamError(e.to_string()))?;

        // Start ack timer
        let ack_tracker = self.ack_tracker.clone();
        let bot_clone = bot.clone();
        let ack_chat_id = ChatId(chat_id);
        let user_id = profile.user_id.clone();

        tokio::spawn(async move {
            let mut rx = ack_tracker.track(&user_id, &msg_id).await;
            if rx.recv().await.is_some() {
                if let Err(e) = bot_clone
                    .send_message(ack_chat_id, "Working on it...")
                    .await
                {
                    error!(error = %e, "failed to send ack message");
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

        // Cancel ack timer for this user
        self.ack_tracker.cancel_all_for_user(target_user_id).await;

        // Look up chat_id
        let chat_id = {
            let mapping = self.chat_mapping.lock().await;
            mapping.lookup_chat_id(target_user_id)
        };

        let chat_id = chat_id
            .ok_or_else(|| TelegramError::SendFailed("no chat mapping found for user".into()))?;

        // Send to Telegram
        bot.send_message(ChatId(chat_id), &text_msg.content)
            .await
            .map_err(|e| {
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
