//! `wh compact` subcommand: trigger stream compaction.
//!
//! Runs compaction directly on WAL files (does not require a running broker).
//! All output routed through the format switch (SCV-05).

use std::path::PathBuf;

use clap::Args;
use serde::Serialize;
use wh_broker::wal::compaction;
use wh_broker::wal::WalWriter;

use crate::output::{OutputEnvelope, OutputFormat};

/// Arguments for the `wh compact` command.
#[derive(Debug, Args)]
pub struct CompactArgs {
    /// Stream name to compact.
    #[arg(long)]
    pub stream: String,
    /// Workspace root directory (defaults to current directory).
    #[arg(long, default_value = ".")]
    pub wh_dir: PathBuf,
    /// Compact records since this ISO-8601 timestamp (defaults to 24h ago).
    #[arg(long)]
    pub since: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
}

/// JSON data payload for compaction result.
#[derive(Debug, Serialize)]
struct CompactResultData {
    stream_name: String,
    date: String,
    record_count: u64,
    total_payload_bytes: u64,
    summary_path: String,
    commit_hash: String,
}

impl CompactArgs {
    /// Execute the compact command. Returns the process exit code.
    pub async fn execute(self) -> i32 {
        // Parse --since or default to 24h ago
        let since_ms = match &self.since {
            Some(s) => match chrono::DateTime::parse_from_rfc3339(s) {
                Ok(dt) => dt.timestamp_millis(),
                Err(e) => {
                    return self.print_error(
                        "INVALID_TIMESTAMP",
                        format!("invalid --since timestamp: {e}"),
                    );
                }
            },
            None => chrono::Utc::now().timestamp_millis() - 86_400_000,
        };

        // Open WAL for the stream
        let data_dir = self.wh_dir.join(".wh");
        let wal_writer = match WalWriter::open(&data_dir, &self.stream) {
            Ok(w) => w,
            Err(e) => {
                return self.print_error("WAL_OPEN_FAILED", format!("failed to open WAL: {e}"));
            }
        };

        let workspace_root = self.wh_dir.clone();
        let stream = self.stream.clone();

        match compaction::compact_stream(
            &workspace_root,
            &stream,
            &wal_writer,
            since_ms,
        ).await {
            Ok(summary) => {
                match self.format {
                    OutputFormat::Json => {
                        let data = CompactResultData {
                            stream_name: summary.stream_name,
                            date: summary.date,
                            record_count: summary.record_count,
                            total_payload_bytes: summary.total_payload_bytes,
                            summary_path: summary.summary_path.to_string_lossy().to_string(),
                            commit_hash: summary.commit_hash,
                        };
                        let envelope = OutputEnvelope::ok(data);
                        if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                            println!("{json}");
                        }
                    }
                    OutputFormat::Human => {
                        println!(
                            "Compaction complete for stream '{}' ({})",
                            summary.stream_name, summary.date
                        );
                        println!("  Records: {}", summary.record_count);
                        println!(
                            "  Summary: {}",
                            summary.summary_path.display()
                        );
                        println!("  Commit:  {}", summary.commit_hash);
                    }
                }
                0
            }
            Err(e) => self.print_error(e.code(), e.to_string()),
        }
    }

    fn print_error(&self, code: &str, message: String) -> i32 {
        match self.format {
            OutputFormat::Json => {
                let envelope = OutputEnvelope::<()>::error(code, message);
                if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                    eprintln!("{json}");
                } else {
                    eprintln!("Error: {code}");
                }
            }
            OutputFormat::Human => {
                eprintln!("Error: {message}");
            }
        }
        1
    }
}
