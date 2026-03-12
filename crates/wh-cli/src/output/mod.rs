//! Output formatting layer — format switch: human vs `--format json` (SCV-05).
//!
//! ALL output goes through this module — never serialize JSON directly in command handlers.

pub mod error;
pub mod json;
pub mod table;

pub use error::LintError;

/// A text message exchanged on a surface stream (CLI, Telegram, etc.).
///
/// This is a CLI-layer type for surface commands; it uses string fields
/// for human-readable timestamps. The proto-layer `wh_proto::TextMessage`
/// uses numeric `timestamp_ms` and is used for broker wire encoding.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SurfaceMessage {
    pub content: String,
    pub publisher: String,
    pub timestamp: String,
}

/// Format a surface message for display.
///
/// Human: `[timestamp] publisher: content`
/// JSON:  `{ "v": 1, "status": "ok", "data": { ... } }`
pub fn format_message(msg: &SurfaceMessage, format: OutputFormat) -> String {
    match format {
        OutputFormat::Human => {
            format!("[{}] {}: {}", msg.timestamp, msg.publisher, msg.content)
        }
        OutputFormat::Json => {
            #[derive(serde::Serialize)]
            struct MsgData<'a> {
                publisher: &'a str,
                timestamp: &'a str,
                content: &'a str,
            }
            let data = MsgData {
                publisher: &msg.publisher,
                timestamp: &msg.timestamp,
                content: &msg.content,
            };
            let envelope = OutputEnvelope::ok(data);
            serde_json::to_string(&envelope).unwrap_or_else(|_| {
                format!("{{\"v\":1,\"status\":\"error\",\"code\":\"SERIALIZATION_ERROR\",\"message\":\"failed\"}}")
            })
        }
    }
}

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

impl OutputFormat {
    /// Parse format string from a CLI string flag value.
    pub fn from_str_value(s: &str) -> Result<Self, String> {
        match s {
            "human" => Ok(OutputFormat::Human),
            "json" => Ok(OutputFormat::Json),
            other => Err(format!("invalid format '{other}': expected 'human' or 'json'")),
        }
    }
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

/// Standard API response envelope (used by deploy commands).
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub v: u32,
    pub status: String,
    pub data: T,
}

/// Standard API error response.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub v: u32,
    pub status: String,
    pub code: String,
    pub message: String,
}

/// JSON error envelope (used by commands that return Result<i32, WhError>).
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub v: u32,
    pub status: &'static str,
    pub code: String,
    pub message: String,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        ApiResponse {
            v: 1,
            status: "ok".to_string(),
            data,
        }
    }
}

impl ApiError {
    pub fn new(code: &str, message: &str) -> Self {
        ApiError {
            v: 1,
            status: "error".to_string(),
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

impl ErrorEnvelope {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            v: 1,
            status: "error",
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Format a successful response for the given output format.
pub fn format_response<T: Serialize + std::fmt::Display>(
    data: &T,
    format: OutputFormat,
) -> String {
    match format {
        OutputFormat::Json => {
            let response = ApiResponse::ok(data);
            serde_json::to_string_pretty(&response).unwrap_or_else(|e| {
                format!("{{\"v\":1,\"status\":\"error\",\"code\":\"SERIALIZATION_ERROR\",\"message\":\"{e}\"}}")
            })
        }
        OutputFormat::Human => {
            format!("{data}")
        }
    }
}

/// Format an error response for the given output format.
pub fn format_error(code: &str, message: &str, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => {
            let error = ApiError::new(code, message);
            serde_json::to_string_pretty(&error).unwrap_or_else(|e| {
                format!("{{\"v\":1,\"status\":\"error\",\"code\":\"SERIALIZATION_ERROR\",\"message\":\"{e}\"}}")
            })
        }
        OutputFormat::Human => {
            format!("Error: {message}")
        }
    }
}
