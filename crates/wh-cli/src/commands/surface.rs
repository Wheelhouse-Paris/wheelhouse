//! `wh surface cli` — interactive terminal surface for agent interaction.
//!
//! Publishes user input as `TextMessage` to a stream and displays incoming
//! messages from agents. Uses a stub connection layer (broker not yet available).

use std::sync::LazyLock;

use crate::output::{format_message, OutputFormat, SurfaceMessage as TextMessage};
use chrono::Utc;
use clap::Subcommand;
use regex::Regex;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::instrument;

use crate::output::error::WhError;

/// Compiled regex for stream name validation (compiled once).
static STREAM_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9-]*$").expect("stream name regex is valid"));

/// Surface commands.
#[derive(Debug, Subcommand)]
pub enum SurfaceCommand {
    /// Start an interactive CLI surface on a stream.
    Cli {
        /// The stream name to connect to.
        #[arg(long)]
        stream: String,

        /// Output format: "human" (default) or "json".
        #[arg(long, default_value = "human", value_parser = ["human", "json"])]
        format: String,
    },
}

/// Validate a stream name matches the allowed pattern: `[a-z][a-z0-9-]*`.
///
/// Returns `Ok(())` if valid, `Err(WhError::StreamError)` with INVALID_NAME if not.
pub fn validate_stream_name(name: &str) -> Result<(), WhError> {
    if STREAM_NAME_RE.is_match(name) {
        Ok(())
    } else {
        Err(WhError::StreamError(format!(
            "INVALID_NAME: stream name '{name}' must match pattern [a-z][a-z0-9-]*"
        )))
    }
}

/// Trait for surface connections. Allows swapping stub for real ZMQ later.
pub trait SurfaceConnection: Send + Sync {
    /// Publish a message to the stream.
    fn publish(
        &self,
        stream: &str,
        msg: &TextMessage,
    ) -> impl std::future::Future<Output = Result<(), WhError>> + Send;
}

/// Stub connection for MVP — no broker available yet.
///
/// Published messages are echoed back via the subscriber channel so the CLI
/// surface can be tested end-to-end without a broker.
pub struct StubConnection {
    tx: mpsc::Sender<TextMessage>,
    rx: Option<mpsc::Receiver<TextMessage>>,
}

impl StubConnection {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self { tx, rx: Some(rx) }
    }
}

impl Default for StubConnection {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceConnection for StubConnection {
    async fn publish(&self, _stream: &str, msg: &TextMessage) -> Result<(), WhError> {
        self.tx
            .send(msg.clone())
            .await
            .map_err(|e| WhError::InternalError(format!("failed to send message: {e}")))
    }
}

/// Run the interactive CLI surface.
///
/// Reads lines from stdin, publishes them as TextMessage, and displays
/// incoming messages from the stream.
#[instrument(name = "surface::run_cli", skip_all, fields(stream = %stream, format = %format_str))]
pub async fn run_cli(stream: &str, format_str: &str) -> Result<(), WhError> {
    // Validate inputs before any connection attempt (architecture spec)
    validate_stream_name(stream)?;

    let output_format =
        OutputFormat::from_str_value(format_str).map_err(|e| WhError::StreamError(e))?;

    let mut conn = StubConnection::new();
    // Take the receiver before moving into tasks
    let mut rx = conn.rx.take().expect("receiver should be available");

    let shutdown = std::sync::Arc::new(tokio::sync::Notify::new());
    let shutdown_pub = shutdown.clone();
    let shutdown_disp = shutdown.clone();

    // Task: read stdin and publish
    let stream_name = stream.to_string();
    let publish_handle = tokio::spawn(async move {
        let stdin = io::stdin();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        loop {
            tokio::select! {
                line = lines.next_line() => {
                    match line {
                        Ok(Some(text)) => {
                            if text.is_empty() {
                                continue;
                            }
                            let msg = TextMessage {
                                content: text,
                                publisher: "cli-surface".to_string(),
                                timestamp: Utc::now().to_rfc3339(),
                            };
                            if let Err(e) = conn.publish(&stream_name, &msg).await {
                                tracing::error!("{e}");
                                break;
                            }
                        }
                        Ok(None) => {
                            // EOF — stdin closed
                            break;
                        }
                        Err(e) => {
                            tracing::error!("Error reading input: {e}");
                            break;
                        }
                    }
                }
                _ = shutdown_pub.notified() => {
                    break;
                }
            }
        }
    });

    // Task: receive and display messages
    let display_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(text_msg) => {
                            let formatted = format_message(&text_msg, output_format);
                            println!("{formatted}");
                        }
                        None => break,
                    }
                }
                _ = shutdown_disp.notified() => {
                    break;
                }
            }
        }
    });

    // Wait for Ctrl-C
    tokio::signal::ctrl_c()
        .await
        .map_err(|e| WhError::InternalError(format!("failed to listen for Ctrl-C: {e}")))?;

    // Graceful shutdown — notify all tasks to stop
    shutdown.notify_waiters();

    // Wait briefly for tasks to finish, then abort if still running
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), publish_handle).await;
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), display_handle).await;

    // Exit cleanly with code 0, no error message
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_name_validation_valid() {
        assert!(validate_stream_name("main").is_ok());
        assert!(validate_stream_name("my-stream").is_ok());
        assert!(validate_stream_name("a").is_ok());
        assert!(validate_stream_name("stream1").is_ok());
        assert!(validate_stream_name("test-stream-123").is_ok());
    }

    #[test]
    fn test_stream_name_validation_invalid() {
        assert!(validate_stream_name("").is_err());
        assert!(validate_stream_name("1stream").is_err());
        assert!(validate_stream_name("Main").is_err());
        assert!(validate_stream_name("my_stream").is_err());
        assert!(validate_stream_name("STREAM").is_err());
        assert!(validate_stream_name("-stream").is_err());
        assert!(validate_stream_name("stream name").is_err());
    }

    #[test]
    fn test_stream_name_validation_error_contains_invalid_name() {
        let err = validate_stream_name("Bad").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("INVALID_NAME"),
            "error should contain INVALID_NAME code"
        );
    }

    #[tokio::test]
    async fn test_stub_connection_publish_and_receive() {
        let mut conn = StubConnection::new();
        let mut rx = conn.rx.take().unwrap();

        let msg = TextMessage {
            content: "hello".to_string(),
            publisher: "cli-surface".to_string(),
            timestamp: "2026-03-12T10:30:00Z".to_string(),
        };

        conn.publish("main", &msg).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.content, "hello");
        assert_eq!(received.publisher, "cli-surface");
    }
}
