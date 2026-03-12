//! Telegram surface configuration.
//!
//! Reads configuration from environment variables set by `wh secrets init`.

use tracing::instrument;

use crate::error::TelegramError;

/// Default stream name when `WH_TELEGRAM_STREAM` is not set.
const DEFAULT_STREAM: &str = "main";

/// Default startup timeout in seconds.
const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 120;

/// Configuration for the Telegram surface.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    bot_token: String,
    stream_name: String,
    startup_timeout_secs: u64,
}

impl TelegramConfig {
    /// Reads configuration from environment variables.
    ///
    /// - `WH_TELEGRAM_BOT_TOKEN` (required): Telegram bot token from BotFather.
    /// - `WH_TELEGRAM_STREAM` (optional, default "main"): Stream to publish/subscribe.
    #[instrument]
    pub fn from_env() -> Result<Self, TelegramError> {
        let bot_token = std::env::var("WH_TELEGRAM_BOT_TOKEN")
            .map_err(|_| TelegramError::ConfigError(
                "WH_TELEGRAM_BOT_TOKEN environment variable not set".into(),
            ))?;

        if bot_token.is_empty() {
            return Err(TelegramError::ConfigError(
                "WH_TELEGRAM_BOT_TOKEN cannot be empty".into(),
            ));
        }

        let stream_name = std::env::var("WH_TELEGRAM_STREAM")
            .unwrap_or_else(|_| DEFAULT_STREAM.to_string());

        validate_stream_name(&stream_name)?;

        Ok(Self {
            bot_token,
            stream_name,
            startup_timeout_secs: DEFAULT_STARTUP_TIMEOUT_SECS,
        })
    }

    /// Returns the bot token.
    pub fn bot_token(&self) -> &str {
        &self.bot_token
    }

    /// Returns the stream name.
    pub fn stream_name(&self) -> &str {
        &self.stream_name
    }

    /// Returns the startup timeout in seconds.
    pub fn startup_timeout_secs(&self) -> u64 {
        self.startup_timeout_secs
    }
}

/// Validates stream name matches `[a-z][a-z0-9-]*`.
fn validate_stream_name(name: &str) -> Result<(), TelegramError> {
    if name.is_empty() {
        return Err(TelegramError::ConfigError(
            "stream name cannot be empty".into(),
        ));
    }

    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(TelegramError::ConfigError(format!(
            "stream name must start with lowercase letter, got '{}'",
            first
        )));
    }

    for ch in chars {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(TelegramError::ConfigError(format!(
                "stream name contains invalid character: '{}'",
                ch
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_stream_name_accepts_valid() {
        assert!(validate_stream_name("main").is_ok());
        assert!(validate_stream_name("my-stream-1").is_ok());
        assert!(validate_stream_name("a").is_ok());
    }

    #[test]
    fn validate_stream_name_rejects_invalid() {
        assert!(validate_stream_name("").is_err());
        assert!(validate_stream_name("UPPER").is_err());
        assert!(validate_stream_name("1starts-with-number").is_err());
        assert!(validate_stream_name("has spaces").is_err());
        assert!(validate_stream_name("has_underscore").is_err());
    }
}
