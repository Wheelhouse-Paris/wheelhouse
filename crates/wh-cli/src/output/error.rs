//! CLI error hierarchy (ADR-014).
//!
//! Typed `WhError` enum with variants for different error categories.
//! Exit codes: 0 = success, 1 = error, 2 = plan change detected.
//! User-facing messages never mention "broker" (RT-B1).

use std::process;

#[derive(Debug, thiserror::Error)]
pub enum WhError {
    #[error("Wheelhouse is not running -- start it first")]
    ConnectionError,

    #[error("Wheelhouse did not respond in time")]
    Timeout,

    #[error("Invalid response from Wheelhouse: {0}")]
    InvalidResponse(String),

    #[error("{0}")]
    Other(String),
}

impl WhError {
    /// Exit code for this error type.
    pub fn exit_code(&self) -> i32 {
        match self {
            WhError::ConnectionError => 1,
            WhError::Timeout => 1,
            WhError::InvalidResponse(_) => 1,
            WhError::Other(_) => 1,
        }
    }

    /// Print the error and exit with the appropriate code.
    pub fn exit(&self) -> ! {
        eprintln!("Error: {self}");
        process::exit(self.exit_code());
    }
}
