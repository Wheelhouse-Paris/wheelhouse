//! WhError hierarchy with exit code mapping (ADR-014).
//!
//! Typed errors using `thiserror`. `anyhow` is never used in library modules (SCV-04).
//! User-facing messages never mention "broker", "connection refused", or port numbers (RT-B1).
//!
//! Exit codes (ADR-014):
//! - 0: success
//! - 1: error
//! - 2: plan change detected

use serde::Serialize;
use wh_broker::deploy::DeployError;

/// Contextual information about where an error occurred (ADR-014).
///
/// All fields are optional — populate whichever are relevant to the error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ErrorContext {
    /// Source file where the error was detected (e.g., "topology.wh")
    pub file: Option<String>,
    /// Line number in the source file
    pub line: Option<u32>,
    /// Field name that caused the error (e.g., "replicas")
    pub field: Option<String>,
}

/// Sub-categories for deploy-related errors (maps to WH-2001/2002/2003).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployErrorKind {
    /// Lint validation failure (WH-2001)
    LintError,
    /// Plan generation failure (WH-2002)
    PlanError,
    /// Apply execution failure (WH-2003)
    ApplyError,
}

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

    /// Specified stream was not found in the topology.
    #[error("Stream not found: {0}")]
    StreamNotFound(String),

    /// An internal error occurred.
    #[error("Internal error: {0}")]
    Internal(String),

    /// An internal error occurred (alias for backward compatibility).
    #[error("Internal error: {0}")]
    InternalError(String),

    /// A stream operation failed.
    #[error("Stream error: {0}")]
    StreamError(String),

    /// Control socket request timed out (treated as connection failure for user messaging).
    #[error("Wheelhouse not running")]
    Timeout,

    /// Control socket returned an invalid response.
    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    /// A required secret is not configured (neither env var nor keychain).
    #[error("Secret '{0}' not configured. Run 'wh secrets init' to set up credentials.")]
    SecretNotFound(String),

    /// Generic error (used by ControlClient).
    #[error("{0}")]
    Other(String),
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
            WhError::StreamNotFound(_) => WhErrorCode("STREAM_NOT_FOUND"),
            WhError::Internal(_) => WhErrorCode("INTERNAL_ERROR"),
            WhError::InternalError(_) => WhErrorCode("INTERNAL_ERROR"),
            WhError::StreamError(_) => WhErrorCode("STREAM_ERROR"),
            WhError::Timeout => WhErrorCode("CONNECTION_ERROR"),
            WhError::InvalidResponse(_) => WhErrorCode("INVALID_RESPONSE"),
            WhError::SecretNotFound(_) => WhErrorCode("SECRET_NOT_FOUND"),
            WhError::Other(_) => WhErrorCode("INTERNAL_ERROR"),
        }
    }

    /// Returns the exit code for this error.
    pub fn exit_code(&self) -> i32 {
        EXIT_ERROR
    }

    /// Print the error and exit with the appropriate exit code.
    pub fn exit(&self) -> ! {
        eprintln!("Error: {self}");
        std::process::exit(self.exit_code());
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
