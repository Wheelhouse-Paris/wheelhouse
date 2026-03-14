//! Telegram surface configuration.
//!
//! Reads configuration from environment variables set by `wh secrets init`
//! and the surface provisioning layer (Story 9.1).

use tracing::instrument;

use crate::error::TelegramError;

/// Default stream name when neither `WH_STREAM` nor `WH_TELEGRAM_STREAM` is set.
const DEFAULT_STREAM: &str = "main";

/// Default startup timeout in seconds.
const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 120;

/// Default surface name when `WH_SURFACE_NAME` is not set.
const DEFAULT_SURFACE_NAME: &str = "telegram";

/// Configuration for the Telegram surface.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    bot_token: String,
    stream_name: String,
    startup_timeout_secs: u64,
    /// Broker ZMQ PUB endpoint URL (e.g., `tcp://127.0.0.1:5555`).
    wh_url: String,
    /// Surface name for identification in the topology.
    surface_name: String,
}

impl TelegramConfig {
    /// Reads configuration from environment variables.
    ///
    /// - `WH_TELEGRAM_BOT_TOKEN` (required): Telegram bot token from BotFather.
    /// - `WH_URL` (required): Broker ZMQ PUB endpoint.
    /// - `WH_STREAM` (optional): Stream name from provisioning layer. Falls back to
    ///   `WH_TELEGRAM_STREAM`, then default "main".
    /// - `WH_SURFACE_NAME` (optional, default "telegram"): Surface name for identification.
    #[instrument]
    pub fn from_env() -> Result<Self, TelegramError> {
        let bot_token = std::env::var("WH_TELEGRAM_BOT_TOKEN").map_err(|_| {
            TelegramError::ConfigError("WH_TELEGRAM_BOT_TOKEN environment variable not set".into())
        })?;

        if bot_token.is_empty() {
            return Err(TelegramError::ConfigError(
                "WH_TELEGRAM_BOT_TOKEN cannot be empty".into(),
            ));
        }

        let wh_url = std::env::var("WH_URL").map_err(|_| {
            TelegramError::ConfigError(
                "WH_URL environment variable not set -- set it in the .wh topology file".into(),
            )
        })?;

        if wh_url.is_empty() {
            return Err(TelegramError::ConfigError("WH_URL cannot be empty".into()));
        }

        // Stream name: WH_STREAM (provisioning layer) > WH_TELEGRAM_STREAM (backward compat) > default
        let stream_name = std::env::var("WH_STREAM")
            .or_else(|_| std::env::var("WH_TELEGRAM_STREAM"))
            .unwrap_or_else(|_| DEFAULT_STREAM.to_string());

        validate_stream_name(&stream_name)?;

        let surface_name =
            std::env::var("WH_SURFACE_NAME").unwrap_or_else(|_| DEFAULT_SURFACE_NAME.to_string());

        Ok(Self {
            bot_token,
            stream_name,
            startup_timeout_secs: DEFAULT_STARTUP_TIMEOUT_SECS,
            wh_url,
            surface_name,
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

    /// Returns the broker ZMQ PUB endpoint URL.
    pub fn wh_url(&self) -> &str {
        &self.wh_url
    }

    /// Returns the surface name.
    pub fn surface_name(&self) -> &str {
        &self.surface_name
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
            "stream name must start with lowercase letter, got '{first}'"
        )));
    }

    for ch in chars {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(TelegramError::ConfigError(format!(
                "stream name contains invalid character: '{ch}'"
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

    #[test]
    fn config_wh_url_accessor_exists() {
        // Compile-time check that the accessor method exists with correct signature
        let _: fn(&TelegramConfig) -> &str = TelegramConfig::wh_url;
    }

    #[test]
    fn config_surface_name_accessor_exists() {
        // Compile-time check that the accessor method exists with correct signature
        let _: fn(&TelegramConfig) -> &str = TelegramConfig::surface_name;
    }
}
