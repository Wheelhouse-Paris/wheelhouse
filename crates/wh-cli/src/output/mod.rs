//! Output formatting layer — format switch: human vs `--format json` (SCV-05).
//!
//! ALL output goes through this module — never serialize JSON directly in command handlers.

pub mod error;
pub mod json;
pub mod table;

use clap::ValueEnum;
use serde::Serialize;

/// Output format selector for `--format` flag.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table output (default).
    #[default]
    Human,
    /// Machine-readable JSON output with `"v": 1` schema version.
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
    pub v: u32,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T: Serialize> OutputEnvelope<T> {
    pub fn ok(data: T) -> Self {
        Self { v: 1, status: "ok".to_string(), data: Some(data), code: None, message: None }
    }
}

impl OutputEnvelope<()> {
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
