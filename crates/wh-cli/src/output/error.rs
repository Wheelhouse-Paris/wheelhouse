//! WhError hierarchy and exit code mapping (ADR-014).
//!
//! User-facing messages never mention "broker" — use "Wheelhouse not running",
//! "stream", etc. (RT-B1).

use std::process;

/// Typed error enum for CLI operations (ADR-014).
///
/// Exit codes: 0 = success, 1 = error, 2 = plan change detected.
#[derive(Debug, thiserror::Error)]
pub enum WhError {
    #[error("Wheelhouse is not running: {0}")]
    ConnectionError(String),

    #[error("Stream error: {0}")]
    StreamError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

impl WhError {
    /// Map error to exit code per ADR-014.
    pub fn exit_code(&self) -> i32 {
        match self {
            WhError::ConnectionError(_) => 1,
            WhError::StreamError(_) => 1,
            WhError::InternalError(_) => 1,
        }
    }

    /// Exit the process with the appropriate exit code.
    pub fn exit(&self) -> ! {
        eprintln!("{self}");
        process::exit(self.exit_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_error_exit_code() {
        let err = WhError::ConnectionError("test".into());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_stream_error_exit_code() {
        let err = WhError::StreamError("test".into());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_internal_error_exit_code() {
        let err = WhError::InternalError("test".into());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn test_error_messages_no_broker_mention() {
        let err = WhError::ConnectionError("unable to reach endpoint".into());
        let msg = format!("{err}");
        assert!(!msg.contains("broker"), "user-facing error must not mention 'broker'");
    }
}
