//! JSON output with `"v": 1` schema version field (RT-B3).
//!
//! All JSON field names are `snake_case` at every nesting level (SCV-01).
//! All `--format json` responses include `"v": 1` (RT-B3).

use serde::Serialize;

use super::error::WhError;

/// Schema version for all JSON output (RT-B3).
const SCHEMA_VERSION: u32 = 1;

/// Wrapper for successful JSON responses.
///
/// Format: `{ "v": 1, "status": "ok", "data": { ... } }`
#[derive(Debug, Serialize)]
pub struct JsonSuccess<T: Serialize> {
    pub v: u32,
    pub status: &'static str,
    pub data: T,
}

impl<T: Serialize> JsonSuccess<T> {
    pub fn new(data: T) -> Self {
        Self {
            v: SCHEMA_VERSION,
            status: "ok",
            data,
        }
    }
}

/// Wrapper for error JSON responses.
///
/// Format: `{ "v": 1, "status": "error", "code": "ERROR_CODE", "message": "..." }`
#[derive(Debug, Serialize)]
pub struct JsonError {
    pub v: u32,
    pub status: &'static str,
    pub code: String,
    pub message: String,
}

impl JsonError {
    pub fn from_error(err: &WhError) -> Self {
        Self {
            v: SCHEMA_VERSION,
            status: "error",
            code: err.error_code().to_string(),
            message: err.to_string(),
        }
    }
}

/// Render a successful JSON response to stdout.
pub fn print_json_success<T: Serialize>(data: &T) -> Result<(), WhError> {
    let response = JsonSuccess::new(data);
    let json = serde_json::to_string(&response)
        .map_err(|e| WhError::Internal(format!("JSON serialization failed: {e}")))?;
    println!("{json}");
    Ok(())
}

/// Render an error JSON response to stdout.
pub fn print_json_error(err: &WhError) {
    let response = JsonError::from_error(err);
    // Best-effort: if this serialization fails, we have bigger problems
    if let Ok(json) = serde_json::to_string(&response) {
        println!("{json}");
    }
}
