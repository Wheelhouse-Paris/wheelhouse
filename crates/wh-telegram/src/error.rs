//! Error types for the Telegram surface.
//!
//! All errors shown to Telegram users are sanitized to a generic message.
//! Raw error details are logged internally only (RT-B1).

use thiserror::Error;

/// Errors that can occur in the Telegram surface.
#[derive(Error, Debug)]
pub enum TelegramError {
    /// Configuration error (missing or invalid config).
    #[error("configuration error: {0}")]
    ConfigError(String),

    /// Telegram Bot API error.
    #[error("bot error: {0}")]
    BotError(String),

    /// Stream communication error.
    #[error("stream error: {0}")]
    StreamError(String),

    /// User store error (profile registration/lookup).
    #[error("user store error: {0}")]
    UserStoreError(#[from] wh_user::UserError),

    /// Failed to send message via Telegram.
    #[error("send failed: {0}")]
    SendFailed(String),

    /// Invalid bot token.
    #[error("invalid bot token")]
    InvalidToken,

    /// Chat mapping error.
    #[error("chat mapping error: {0}")]
    MappingError(String),

    /// State persistence error.
    #[error("state error: {0}")]
    StateError(String),
}

/// Sanitizes any error to a user-safe message.
///
/// ALWAYS returns the same generic message regardless of error type.
/// This is a hard rule from AC #4 and RT-B1: never expose technical
/// details (error codes, stack traces, broker names, stream terminology,
/// port numbers, socket types, internal process names) to Telegram users.
///
/// Raw error details must be logged at `tracing::error!` BEFORE calling
/// this function.
pub fn sanitize_for_user(_err: &TelegramError) -> String {
    "Something went wrong. Please try again or contact support.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_returns_same_message_for_all_variants() {
        let errors = vec![
            TelegramError::ConfigError("missing TELEGRAM_BOT_TOKEN".into()),
            TelegramError::BotError("API rate limit exceeded".into()),
            TelegramError::StreamError("broker:5555 connection refused on zmq socket".into()),
            TelegramError::SendFailed("network timeout after 30s".into()),
            TelegramError::InvalidToken,
            TelegramError::MappingError("corrupt YAML".into()),
            TelegramError::StateError("file not found".into()),
        ];
        for err in &errors {
            assert_eq!(
                sanitize_for_user(err),
                "Something went wrong. Please try again or contact support."
            );
        }
    }

    #[test]
    fn sanitized_message_contains_no_technical_terms() {
        let err = TelegramError::StreamError("broker:5555 zmq XPUB socket failed".into());
        let sanitized = sanitize_for_user(&err);
        // Note: check for exact technical terms, not substrings like "port" (which appears in "support")
        let forbidden = [
            "broker",
            "stream",
            " port ",
            "socket",
            "zmq",
            "error code",
            "stack trace",
            "internal error",
            "xpub",
            "xsub",
            ":5555",
            "localhost",
        ];
        for term in &forbidden {
            assert!(
                !sanitized.to_lowercase().contains(term),
                "sanitized message must not contain '{term}'"
            );
        }
    }
}
