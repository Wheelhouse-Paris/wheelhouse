//! Monitor error types.
//!
//! Typed error hierarchy for the monitor module using `thiserror`.
//! Error codes follow SCREAMING_SNAKE_CASE per SCV-01.

/// Errors that can occur in the monitor module.
#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    /// Invalid timeout string provided in `.wh` configuration.
    #[error("invalid loop_detection_timeout value: \"{input}\" — expected format: \"0\", \"30s\", \"15m\", or \"1h\"")]
    InvalidTimeout { input: String },

    /// Failed to send alert via channel.
    #[error("alert channel closed for agent \"{agent_name}\"")]
    ChannelClosed { agent_name: String },

    /// Alert delivery failed.
    #[error("alert delivery failed for agent \"{agent_name}\": {source}")]
    AlertFailed {
        agent_name: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl MonitorError {
    /// Returns SCREAMING_SNAKE_CASE error code per SCV-01.
    pub fn code(&self) -> &'static str {
        match self {
            MonitorError::InvalidTimeout { .. } => "INVALID_TIMEOUT",
            MonitorError::ChannelClosed { .. } => "CHANNEL_CLOSED",
            MonitorError::AlertFailed { .. } => "ALERT_FAILED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_timeout_display() {
        let err = MonitorError::InvalidTimeout {
            input: "abc".to_string(),
        };
        assert!(err.to_string().contains("abc"));
        assert_eq!(err.code(), "INVALID_TIMEOUT");
    }

    #[test]
    fn test_channel_closed_display() {
        let err = MonitorError::ChannelClosed {
            agent_name: "researcher-2".to_string(),
        };
        assert!(err.to_string().contains("researcher-2"));
        assert_eq!(err.code(), "CHANNEL_CLOSED");
    }

    #[test]
    fn test_alert_failed_display() {
        let err = MonitorError::AlertFailed {
            agent_name: "assistant-1".to_string(),
            source: Box::new(std::io::Error::other("test")),
        };
        assert!(err.to_string().contains("assistant-1"));
        assert_eq!(err.code(), "ALERT_FAILED");
    }

    #[test]
    fn test_all_codes_are_screaming_snake_case() {
        let codes = [
            MonitorError::InvalidTimeout {
                input: "x".to_string(),
            }
            .code(),
            MonitorError::ChannelClosed {
                agent_name: "a".to_string(),
            }
            .code(),
            MonitorError::AlertFailed {
                agent_name: "a".to_string(),
                source: Box::new(std::io::Error::other("t")),
            }
            .code(),
        ];
        for code in &codes {
            assert!(
                code.chars().all(|c| c.is_ascii_uppercase() || c == '_'),
                "code '{code}' is not SCREAMING_SNAKE_CASE"
            );
        }
    }
}
