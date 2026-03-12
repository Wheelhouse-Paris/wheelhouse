//! `wh memory` subcommands: write, read, append.
//!
//! Implements the agent-facing CLI for MEMORY.md persistence (FR62).
//! All output routed through the format switch (SCV-05).

use std::path::PathBuf;

use clap::Subcommand;
use wh_broker::deploy::memory;

use crate::output::{OutputEnvelope, OutputFormat};

/// Memory subcommands for managing agent MEMORY.md files.
#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Write (overwrite) an agent's MEMORY.md and commit to git.
    Write {
        /// Agent name (alphanumeric and hyphens only).
        #[arg(long)]
        agent_name: String,
        /// Content to write to MEMORY.md. Mutually exclusive with --content-file.
        #[arg(long, required_unless_present = "content_file")]
        content: Option<String>,
        /// Path to a file whose content will be written to MEMORY.md.
        #[arg(long, conflicts_with = "content")]
        content_file: Option<PathBuf>,
        /// Reason for the memory update (appears in git commit message).
        #[arg(long)]
        reason: String,
        /// Workspace root directory (defaults to current directory).
        #[arg(long, default_value = ".")]
        wh_dir: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Read an agent's MEMORY.md content.
    Read {
        /// Agent name.
        #[arg(long)]
        agent_name: String,
        /// Workspace root directory (defaults to current directory).
        #[arg(long, default_value = ".")]
        wh_dir: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Append an entry to an agent's MEMORY.md and commit to git.
    Append {
        /// Agent name.
        #[arg(long)]
        agent_name: String,
        /// Content to append.
        #[arg(long, required_unless_present = "content_file")]
        content: Option<String>,
        /// Path to a file whose content will be appended.
        #[arg(long, conflicts_with = "content")]
        content_file: Option<PathBuf>,
        /// Reason for the memory update.
        #[arg(long)]
        reason: String,
        /// Workspace root directory.
        #[arg(long, default_value = ".")]
        wh_dir: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

impl MemoryCommand {
    /// Execute the memory subcommand. Returns the process exit code.
    pub fn execute(self) -> i32 {
        match self {
            MemoryCommand::Write {
                agent_name,
                content,
                content_file,
                reason,
                wh_dir,
                format,
            } => {
                let body = match resolve_content(content, content_file) {
                    Ok(c) => c,
                    Err(e) => return print_error(&e, format),
                };
                execute_write(&wh_dir, &agent_name, &body, &reason, format)
            }
            MemoryCommand::Read {
                agent_name,
                wh_dir,
                format,
            } => execute_read(&wh_dir, &agent_name, format),
            MemoryCommand::Append {
                agent_name,
                content,
                content_file,
                reason,
                wh_dir,
                format,
            } => {
                let body = match resolve_content(content, content_file) {
                    Ok(c) => c,
                    Err(e) => return print_error(&e, format),
                };
                execute_append(&wh_dir, &agent_name, &body, &reason, format)
            }
        }
    }
}

/// JSON envelope payload for write/append results.
#[derive(serde::Serialize)]
struct MemoryResultData {
    agent_name: String,
    commit_hash: String,
    file_path: String,
    timestamp: String,
}

impl MemoryResultData {
    fn from_result(agent_name: &str, result: &memory::MemoryUpdateResult) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            commit_hash: result.commit_hash.clone(),
            file_path: result.file_path.display().to_string(),
            timestamp: result.timestamp.clone(),
        }
    }
}

/// Resolve content from either --content or --content-file.
fn resolve_content(
    content: Option<String>,
    content_file: Option<PathBuf>,
) -> Result<String, memory::MemoryError> {
    if let Some(path) = content_file {
        Ok(std::fs::read_to_string(&path)?)
    } else {
        Ok(content.unwrap_or_default())
    }
}

fn execute_write(
    wh_dir: &std::path::Path,
    agent_name: &str,
    content: &str,
    reason: &str,
    format: OutputFormat,
) -> i32 {
    match memory::write_memory(wh_dir, agent_name, content, reason) {
        Ok(result) => {
            match format {
                OutputFormat::Human => {
                    println!(
                        "Memory updated for agent '{}'\n  Commit: {}\n  File: {}",
                        agent_name,
                        result.commit_hash,
                        result.file_path.display()
                    );
                }
                OutputFormat::Json => {
                    let data = MemoryResultData::from_result(agent_name, &result);
                    let envelope = OutputEnvelope::ok(data);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&envelope).unwrap_or_default()
                    );
                }
            }
            0
        }
        Err(e) => print_error(&e, format),
    }
}

fn execute_read(wh_dir: &std::path::Path, agent_name: &str, format: OutputFormat) -> i32 {
    match memory::read_memory(wh_dir, agent_name) {
        Ok(content) => {
            let text = content.unwrap_or_default();
            match format {
                OutputFormat::Human => {
                    if text.is_empty() {
                        // No output for empty memory (FR61)
                    } else if text.ends_with('\n') {
                        print!("{text}");
                    } else {
                        println!("{text}");
                    }
                }
                OutputFormat::Json => {
                    #[derive(serde::Serialize)]
                    struct ReadData {
                        agent_name: String,
                        content: String,
                    }
                    let data = ReadData {
                        agent_name: agent_name.to_string(),
                        content: text,
                    };
                    let envelope = OutputEnvelope::ok(data);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&envelope).unwrap_or_default()
                    );
                }
            }
            0
        }
        Err(e) => print_error(&e, format),
    }
}

fn execute_append(
    wh_dir: &std::path::Path,
    agent_name: &str,
    entry: &str,
    reason: &str,
    format: OutputFormat,
) -> i32 {
    match memory::append_memory(wh_dir, agent_name, entry, reason) {
        Ok(result) => {
            match format {
                OutputFormat::Human => {
                    println!(
                        "Memory appended for agent '{}'\n  Commit: {}\n  File: {}",
                        agent_name,
                        result.commit_hash,
                        result.file_path.display()
                    );
                }
                OutputFormat::Json => {
                    let data = MemoryResultData::from_result(agent_name, &result);
                    let envelope = OutputEnvelope::ok(data);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&envelope).unwrap_or_default()
                    );
                }
            }
            0
        }
        Err(e) => print_error(&e, format),
    }
}

/// Print an error message and return exit code 1 (ADR-014).
fn print_error(err: &memory::MemoryError, format: OutputFormat) -> i32 {
    match format {
        OutputFormat::Human => {
            eprintln!("Error: {err}");
        }
        OutputFormat::Json => {
            let envelope = OutputEnvelope::<()>::error(err.code(), err.to_string());
            if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                eprintln!("{json}");
            } else {
                eprintln!("Error: {err}");
            }
        }
    }
    1
}
