//! WhError hierarchy with exit code mapping (ADR-014).
//!
//! Typed errors using `thiserror`. `anyhow` is never used in library modules (SCV-04).
//! User-facing messages never mention "broker", "connection refused", or port numbers (RT-B1).
//!
//! Exit codes (ADR-014):
//! - 0: success
//! - 1: error
//! - 2: plan change detected

use wh_broker::deploy::DeployError;

/// Wrapper for error code strings to allow `.as_str()` calls.
#[derive(Debug, Clone, Copy)]
pub struct WhErrorCode(pub &'static str);

impl WhErrorCode {
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

/// CLI exit codes per ADR-014.
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

    /// Specified agent was not found in the topology.
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    /// An internal error occurred.
    #[error("Internal error: {0}")]
    Internal(String),

    /// An internal error occurred (alias for backward compatibility).
    #[error("Internal error: {0}")]
    InternalError(String),

    /// A stream operation failed.
    #[error("Stream error: {0}")]
    StreamError(String),
}

impl WhError {
    /// Returns the machine-readable error code (SCREAMING_SNAKE_CASE per SCV-01).
    pub fn error_code(&self) -> &'static str {
        self.code().as_str()
    }

    /// Returns the machine-readable error code as a `WhErrorCode`.
    pub fn code(&self) -> WhErrorCode {
        match self {
            WhError::ConnectionError => WhErrorCode("CONNECTION_ERROR"),
            WhError::GitNotFound(_) => WhErrorCode("GIT_NOT_FOUND"),
            WhError::KeychainError(_) => WhErrorCode("KEYCHAIN_ERROR"),
            WhError::PromptFailed(_) => WhErrorCode("PROMPT_FAILED"),
            WhError::NonInteractive => WhErrorCode("NON_INTERACTIVE"),
            WhError::AgentNotFound(_) => WhErrorCode("AGENT_NOT_FOUND"),
            WhError::Internal(_) => WhErrorCode("INTERNAL_ERROR"),
            WhError::InternalError(_) => WhErrorCode("INTERNAL_ERROR"),
            WhError::StreamError(_) => WhErrorCode("STREAM_ERROR"),
        }
    }

    /// Returns the exit code for this error.
    pub fn exit_code(&self) -> i32 {
        EXIT_ERROR
    }
}

/// Map a DeployError to its error code string.
pub fn deploy_error_code(err: &DeployError) -> &'static str {
    err.code()
}

/// Map a DeployError to its exit code.
pub fn exit_code_for_deploy_error(_err: &DeployError) -> i32 {
    EXIT_ERROR
}

/// Errors produced by the `.wh` file lint engine.
#[derive(Debug, thiserror::Error)]
pub enum LintError {
    #[error("failed to read file: {0}")]
    FileReadError(std::io::Error),

    #[error("failed to parse YAML: {0}")]
    YamlParseError(String),
}

impl LintError {
    pub fn error_code(&self) -> &'static str {
        match self {
            LintError::FileReadError(_) => "LINT_FILE_ERROR",
            LintError::YamlParseError(_) => "LINT_PARSE_ERROR",
        }
    }
}
