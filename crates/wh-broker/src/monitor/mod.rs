//! Agent loop detection and behavioral alerting.
//!
//! Monitors per-agent publish activity and alerts operators when an agent
//! has been silent beyond a configurable timeout. Embedded in the broker
//! process alongside the cron module (ADR-012 pattern).

pub mod error;
pub mod registry;
pub mod silence;

pub use error::MonitorError;
pub use registry::MonitorRegistry;
pub use silence::{ActivityHandle, SilenceAlert, SilenceMonitor};

use std::time::Duration;

/// Configuration for an agent's loop detection monitor.
///
/// Parsed from the `.wh` file's per-agent `loop_detection_timeout` field.
#[derive(Debug, Clone)]
pub struct AgentMonitorConfig {
    /// Agent name as declared in the `.wh` file.
    pub agent_name: String,
    /// Target stream name for this agent.
    pub stream_name: String,
    /// Silence timeout. If zero, monitoring is disabled.
    pub timeout: Duration,
}

impl AgentMonitorConfig {
    /// Returns `true` if loop detection is enabled for this agent.
    ///
    /// A timeout of zero means monitoring is disabled (AC #4).
    pub fn is_enabled(&self) -> bool {
        !self.timeout.is_zero()
    }
}

/// Parse a human-friendly duration string into a `Duration`.
///
/// Supported formats:
/// - `"0"` — zero duration (disables monitoring)
/// - `"30s"` — 30 seconds
/// - `"15m"` — 15 minutes
/// - `"1h"` — 1 hour
///
/// No external crate — simple manual parsing.
pub fn parse_duration_str(s: &str) -> Result<Duration, MonitorError> {
    let s = s.trim();

    if s == "0" {
        return Ok(Duration::ZERO);
    }

    if s.len() < 2 {
        return Err(MonitorError::InvalidTimeout {
            input: s.to_string(),
        });
    }

    let (num_str, suffix) = s.split_at(s.len() - 1);
    let num: u64 = num_str.parse().map_err(|_| MonitorError::InvalidTimeout {
        input: s.to_string(),
    })?;

    match suffix {
        "s" => Ok(Duration::from_secs(num)),
        "m" => Ok(Duration::from_secs(num * 60)),
        "h" => Ok(Duration::from_secs(num * 3600)),
        _ => Err(MonitorError::InvalidTimeout {
            input: s.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_15m() {
        let d = parse_duration_str("15m").unwrap();
        assert_eq!(d, Duration::from_secs(900));
    }

    #[test]
    fn test_parse_duration_1h() {
        let d = parse_duration_str("1h").unwrap();
        assert_eq!(d, Duration::from_secs(3600));
    }

    #[test]
    fn test_parse_duration_30s() {
        let d = parse_duration_str("30s").unwrap();
        assert_eq!(d, Duration::from_secs(30));
    }

    #[test]
    fn test_parse_duration_zero() {
        let d = parse_duration_str("0").unwrap();
        assert_eq!(d, Duration::ZERO);
    }

    #[test]
    fn test_parse_duration_zero_disables() {
        let config = AgentMonitorConfig {
            agent_name: "test".to_string(),
            stream_name: "test".to_string(),
            timeout: parse_duration_str("0").unwrap(),
        };
        assert!(!config.is_enabled());
    }

    #[test]
    fn test_parse_duration_invalid_returns_error() {
        assert!(parse_duration_str("abc").is_err());
    }

    #[test]
    fn test_parse_duration_unknown_suffix() {
        assert!(parse_duration_str("10d").is_err());
    }

    #[test]
    fn test_parse_duration_empty_returns_error() {
        assert!(parse_duration_str("").is_err());
    }

    #[test]
    fn test_is_enabled_with_timeout() {
        let config = AgentMonitorConfig {
            agent_name: "test".to_string(),
            stream_name: "test".to_string(),
            timeout: Duration::from_secs(60),
        };
        assert!(config.is_enabled());
    }

    #[test]
    fn test_is_enabled_with_zero_timeout() {
        let config = AgentMonitorConfig {
            agent_name: "test".to_string(),
            stream_name: "test".to_string(),
            timeout: Duration::ZERO,
        };
        assert!(!config.is_enabled());
    }
}
