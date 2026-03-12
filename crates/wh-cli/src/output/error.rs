/// Typed error hierarchy for the `wh` CLI.
///
/// All user-facing errors go through `WhError` — never raw `eprintln!`.
/// Exit code mapping follows the CLI reference table.
#[derive(Debug, thiserror::Error)]
pub enum WhError {
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

    /// Internal error (serialization, unexpected state).
    #[error("Internal error: {0}")]
    Internal(String),
}

impl WhError {
    /// Map error to CLI exit code per the CLI reference table.
    /// - 0: success
    /// - 1: error
    /// - 2: plan change detected (not used here)
    pub fn exit_code(&self) -> i32 {
        1
    }

    /// Machine-readable error code for JSON output (SCREAMING_SNAKE_CASE per architecture).
    pub fn error_code(&self) -> &'static str {
        match self {
            WhError::GitNotFound(_) => "GIT_NOT_FOUND",
            WhError::KeychainError(_) => "KEYCHAIN_ERROR",
            WhError::PromptFailed(_) => "PROMPT_FAILED",
            WhError::NonInteractive => "NON_INTERACTIVE",
            WhError::Internal(_) => "INTERNAL_ERROR",
        }
    }
}
