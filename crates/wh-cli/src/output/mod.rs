pub mod error;

pub use error::{DeployError, LintError, WhError};

use serde::Serialize;

/// Output format selector (SCV-05).
/// All output goes through this format switch — never serialize JSON directly in handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Human,
    Json,
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Format::Human => write!(f, "human"),
            Format::Json => write!(f, "json"),
        }
    }
}

/// JSON envelope: `{ "v": 1, "status": "ok"|"error", "data": { ... } }` (SCV-01, RT-B3).
/// All field names are `snake_case`.
#[derive(Debug, Serialize)]
pub struct OutputEnvelope<T: Serialize> {
    pub v: u32,
    pub status: &'static str,
    pub data: T,
}

/// JSON error envelope.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub v: u32,
    pub status: &'static str,
    pub code: String,
    pub message: String,
}

impl<T: Serialize> OutputEnvelope<T> {
    pub fn ok(data: T) -> Self {
        Self {
            v: 1,
            status: "ok",
            data,
        }
    }

    pub fn error(data: T) -> Self {
        Self {
            v: 1,
            status: "error",
            data,
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
