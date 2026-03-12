//! WhError hierarchy with exit code mapping (ADR-014).
//!
//! Typed errors using `thiserror`. `anyhow` is never used in library modules (SCV-04).
//! User-facing messages never mention "broker", "connection refused", or port numbers (RT-B1).

use std::fmt;

/// Exit codes per ADR-014:
/// - 0 = success
/// - 1 = error
/// - 2 = plan change detected
pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_PLAN_CHANGE: i32 = 2;

/// Machine-readable error codes (SCREAMING_SNAKE_CASE per SCV-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    ConnectionError,
    InternalError,
}

impl ErrorCode {
    /// Returns the SCREAMING_SNAKE_CASE string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::ConnectionError => "CONNECTION_ERROR",
            ErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed error hierarchy for the CLI.
///
/// All variants produce user-friendly messages using approved vocabulary (RT-B1).
#[derive(Debug, thiserror::Error)]
pub enum WhError {
    /// Wheelhouse is not running (control socket unreachable).
    #[error("Wheelhouse not running")]
    ConnectionError,

    /// An internal error occurred.
    #[error("{0}")]
    InternalError(String),
}

impl WhError {
    /// Returns the machine-readable error code.
    pub fn code(&self) -> ErrorCode {
        match self {
            WhError::ConnectionError => ErrorCode::ConnectionError,
            WhError::InternalError(_) => ErrorCode::InternalError,
        }
    }

    /// Returns the exit code for this error.
    pub fn exit_code(&self) -> i32 {
        EXIT_ERROR
    }
}
