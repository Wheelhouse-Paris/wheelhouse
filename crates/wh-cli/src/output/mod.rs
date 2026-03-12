//! Output formatting module.
//!
//! Routes all command output through a format switch (SCV-05):
//! - Human-readable (default)
//! - JSON (`--format json`)
//!
//! Never serialize JSON directly in command handlers.

pub mod error;

use serde::Serialize;

/// Output format for CLI commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text output (default)
    Human,
    /// Machine-readable JSON output
    Json,
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Human
    }
}

/// Standard API response envelope matching architecture spec.
/// `{ "v": 1, "status": "ok", "data": { ... } }`
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub v: u32,
    pub status: String,
    pub data: T,
}

/// Standard API error response.
/// `{ "v": 1, "status": "error", "code": "ERROR_CODE", "message": "..." }`
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub v: u32,
    pub status: String,
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
