//! Output formatting layer — format switch: human vs `--format json` (SCV-05).
//!
//! ALL output goes through this module — never serialize JSON directly in command handlers.

pub mod error;
pub mod json;
pub mod table;

use clap::ValueEnum;

/// Output format selector for `--format` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table output (default).
    Human,
    /// Machine-readable JSON output with `"v": 1` schema version.
    Json,
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Human
    }
}
