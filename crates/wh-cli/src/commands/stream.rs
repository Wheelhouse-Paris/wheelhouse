//! `wh stream` command implementation (Story 1.3).
//!
//! Manages named streams via the broker's control socket.
//! Supports create, list, and delete operations.
//! Human-readable output uses approved vocabulary — never "broker" (RT-B1).

use clap::Subcommand;
use serde_json::json;

use crate::client::ControlClient;
use crate::output::{self, OutputFormat};

/// Stream management commands.
#[derive(Debug, Subcommand)]
pub enum StreamCommand {
    /// Create a new named stream.
    Create {
        /// Stream name (alphanumeric and hyphens, 1-64 chars).
        name: String,
        /// Time-based retention (e.g., "7d", "24h", "30m").
        #[arg(long)]
        retention: Option<String>,
        /// Size-based retention limit (e.g., "500mb", "1gb").
        #[arg(long)]
        retention_size: Option<String>,
    },
    /// List all streams.
    List {
        /// Output format: human (default) or json.
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },
    /// Delete a named stream and its data.
    Delete {
        /// Stream name to delete.
        name: String,
    },
}

/// Execute a stream command.
pub async fn execute(cmd: &StreamCommand) {
    let client = ControlClient::new();

    match cmd {
        StreamCommand::Create {
            name,
            retention,
            retention_size,
        } => {
            let mut payload = json!({
                "command": "stream_create",
                "name": name,
            });
            if let Some(r) = retention {
                payload["retention"] = json!(r);
            }
            if let Some(rs) = retention_size {
                payload["retention_size"] = json!(rs);
            }

            match client.send_command_with_payload(payload).await {
                Ok(response) => {
                    let status = response
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");
                    if status == "ok" {
                        println!("Stream '{}' created", name);
                    } else {
                        output::print_error(&response, OutputFormat::Human);
                        std::process::exit(1);
                    }
                }
                Err(e) => e.exit(),
            }
        }
        StreamCommand::List { format } => {
            match client.send_command("stream_list").await {
                Ok(response) => {
                    let status = response
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");
                    if status == "ok" {
                        print_stream_list(&response, *format);
                    } else {
                        output::print_error(&response, *format);
                        std::process::exit(1);
                    }
                }
                Err(e) => e.exit(),
            }
        }
        StreamCommand::Delete { name } => {
            let payload = json!({
                "command": "stream_delete",
                "name": name,
            });

            match client.send_command_with_payload(payload).await {
                Ok(response) => {
                    let status = response
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");
                    if status == "ok" {
                        println!("Stream '{}' deleted", name);
                    } else {
                        output::print_error(&response, OutputFormat::Human);
                        std::process::exit(1);
                    }
                }
                Err(e) => e.exit(),
            }
        }
    }
}

/// Print stream list in human-readable or JSON format.
fn print_stream_list(response: &serde_json::Value, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(response).unwrap_or_default()
            );
        }
        OutputFormat::Human => {
            let streams = response
                .get("data")
                .and_then(|d| d.get("streams"))
                .and_then(|s| s.as_array());

            match streams {
                Some(streams) if streams.is_empty() => {
                    println!("No streams");
                }
                Some(streams) => {
                    println!("{:<20} {:<12} {:<15} CREATED", "NAME", "RETENTION", "MESSAGES");
                    for stream in streams {
                        let name = stream.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                        let retention = stream
                            .get("retention")
                            .and_then(|v| {
                                if v.is_null() {
                                    None
                                } else {
                                    v.as_str()
                                }
                            })
                            .unwrap_or("none");
                        let message_count = stream
                            .get("message_count")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let created = stream
                            .get("created_at")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-");
                        println!("{:<20} {:<12} {:<15} {}", name, retention, message_count, created);
                    }
                }
                None => {
                    eprintln!("Error: invalid response from Wheelhouse");
                    std::process::exit(1);
                }
            }
        }
    }
}
