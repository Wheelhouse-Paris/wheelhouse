pub mod error;

use serde::Serialize;

/// Output format for CLI commands. Supports human-readable and JSON.
/// Registered as a global flag via clap.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// Human-readable output (default).
    #[default]
    Human,
    /// Structured JSON output with v1 envelope.
    Json,
}

/// Standard JSON response envelope per architecture spec (RT-B3, SCV-01).
///
/// All `--format json` output uses this shape:
/// ```json
/// { "v": 1, "status": "ok", "data": { ... } }
/// { "v": 1, "status": "error", "code": "ERROR_CODE", "message": "..." }
/// ```
#[derive(Debug, Serialize)]
pub struct OutputEnvelope<T: Serialize> {
    /// Schema version — always 1 in MVP.
    pub v: u32,
    /// Response status: "ok" or "error".
    pub status: String,
    /// Response payload (present when status == "ok").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// Error code (present when status == "error").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Human-readable error message (present when status == "error").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T: Serialize> OutputEnvelope<T> {
    /// Create a successful response envelope.
    pub fn ok(data: T) -> Self {
        Self {
            v: 1,
            status: "ok".to_string(),
            data: Some(data),
            code: None,
            message: None,
        }
    }
}

impl OutputEnvelope<()> {
    /// Create an error response envelope.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            v: 1,
            status: "error".to_string(),
            data: None,
            code: Some(code.into()),
            message: Some(message.into()),
        }
    }
}
