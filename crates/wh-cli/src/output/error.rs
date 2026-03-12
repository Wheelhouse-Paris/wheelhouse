//! WhError hierarchy with exit code mapping (ADR-014).
//!
//! Typed errors using `thiserror`. `anyhow` is never used in library modules (SCV-04).
//! User-facing messages never mention "broker", "connection refused", or port numbers (RT-B1).

pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_PLAN_CHANGE: i32 = 2;

/// Typed error hierarchy for the `wh` CLI.
#[derive(Debug, thiserror::Error)]
pub enum WhError {
    /// Wheelhouse is not running (control socket unreachable).
    #[error("Wheelhouse not running")]
    ConnectionError,

    /// Git is not installed or not found on PATH.
    #[error("Git not found: {0}")]
    GitNotFound(String),

    /// Keychain operation failed (macOS Keychain / Linux Secret Service).
    #[error("Keychain error: {0}")]
    KeychainError(String),

    /// Interactive prompt failed (e.g., stdin closed).
    #[error("Prompt failed: {0}")]
    PromptFailed(String),

    /// Command requires an interactive terminal but stdin is not a TTY.
    #[error("Interactive terminal required")]
    NonInteractive,

    /// An internal error occurred.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl WhError {
    /// Returns the machine-readable error code (SCREAMING_SNAKE_CASE per SCV-01).
    pub fn error_code(&self) -> &'static str {
        match self {
            WhError::ConnectionError => "CONNECTION_ERROR",
            WhError::GitNotFound(_) => "GIT_NOT_FOUND",
            WhError::KeychainError(_) => "KEYCHAIN_ERROR",
            WhError::PromptFailed(_) => "PROMPT_FAILED",
            WhError::NonInteractive => "NON_INTERACTIVE",
            WhError::Internal(_) => "INTERNAL_ERROR",
        }
    }

    /// Returns the exit code for this error.
    pub fn exit_code(&self) -> i32 {
        EXIT_ERROR
    }
}
