//! `wh surface cli` — interactive terminal surface for agent interaction.
//!
//! Publishes user input as `TextMessage` to a stream via ZMQ and displays
//! incoming messages from agents in real time. Connects to the broker's
//! PUB/SUB data plane (Story 9.4, FR27).

use std::sync::LazyLock;

use crate::output::{format_message, OutputFormat, SurfaceMessage};
use crate::reconnect::{self, ConnectionEvent};
use chrono::Utc;
use clap::Subcommand;
use prost::Message;
use regex::Regex;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use wh_proto::{StreamEnvelope, TextMessage};
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

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

/// Resolve the PUB endpoint (broker's PUB socket — clients subscribe here).
fn resolve_pub_endpoint() -> String {
    std::env::var("WH_PUB_ENDPOINT").unwrap_or_else(|_| {
        let port = std::env::var("WH_PUB_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(5555);
        format!("tcp://127.0.0.1:{port}")
    })
}

/// Resolve the SUB endpoint (broker's SUB socket — clients publish here).
fn resolve_sub_endpoint() -> String {
    std::env::var("WH_SUB_ENDPOINT").unwrap_or_else(|_| {
        let port = std::env::var("WH_SUB_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(5556);
        format!("tcp://127.0.0.1:{port}")
    })
}

/// Resolve the control endpoint for broker liveness probe.
fn resolve_control_endpoint() -> String {
    std::env::var("WH_CONTROL_ENDPOINT").unwrap_or_else(|_| {
        let port = std::env::var("WH_CONTROL_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(5557);
        format!("tcp://127.0.0.1:{port}")
    })
}

/// Generate a deterministic user_id for the CLI surface user.
///
/// Format: `cli-{username}` where username comes from the USER/USERNAME env var.
fn generate_cli_user_id() -> String {
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    format!("cli-{username}")
}

/// Probe broker liveness via TCP before attempting ZMQ connections.
///
/// ZMQ `connect()` is async and always succeeds — recv() would block forever
/// if the broker isn't running. A TCP probe to the control port gives us an
/// immediate ECONNREFUSED on localhost, or a 1-second timeout otherwise.
/// Same pattern as `execute_tail` in `stream.rs`.
pub async fn probe_broker_liveness() -> Result<(), WhError> {
    let control_addr = resolve_control_endpoint();
    let tcp_addr = control_addr
        .strip_prefix("tcp://")
        .unwrap_or(&control_addr)
        .to_string();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tokio::net::TcpStream::connect(&tcp_addr),
    )
    .await
    .map_err(|_| WhError::ConnectionError)?
    .map_err(|_| WhError::ConnectionError)?;
    Ok(())
}

/// Run the interactive CLI surface with real ZMQ connections (Story 9.4).
///
/// Connects to the broker's data plane:
/// - PUB socket -> broker SUB endpoint (for publishing user messages)
/// - SUB socket -> broker PUB endpoint (for receiving agent responses)
///
/// Architecture: The stdin reader runs in a spawned task and sends lines via
/// an mpsc channel. The main task runs the select! loop over stdin channel,
/// ZMQ subscribe, and Ctrl-C. This avoids spawning the subscribe loop (which
/// needs `&ConnectionEventCallback` for reconnect — not Send+Sync).
///
/// Wire format: `{stream_name}\0{StreamEnvelope protobuf bytes}`
#[instrument(name = "surface::run_cli", skip_all, fields(stream = %stream, format = ?output_format))]
pub async fn run_cli(stream: &str, output_format: OutputFormat) -> Result<(), WhError> {
    // Validate inputs before any connection attempt
    validate_stream_name(stream)?;

    // Probe broker liveness (AC-3)
    if probe_broker_liveness().await.is_err() {
        eprintln!("Wheelhouse is not running. Start it with: wh broker start");
        return Err(WhError::ConnectionError);
    }

    let cli_user_id = generate_cli_user_id();
    let sub_endpoint = resolve_sub_endpoint();
    let pub_endpoint = resolve_pub_endpoint();
    let topic = format!("{stream}\0");

    // Connect PUB socket to broker's SUB endpoint (for publishing)
    let mut pub_socket = PubSocket::new();
    pub_socket
        .connect(&sub_endpoint)
        .await
        .map_err(|_| WhError::ConnectionError)?;

    // ZMQ PUB sockets need a brief delay for subscription handshake (WW-02).
    // Same pattern as execute_publish in stream.rs.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect SUB socket to broker's PUB endpoint (for subscribing)
    let mut sub_socket = SubSocket::new();
    sub_socket
        .connect(&pub_endpoint)
        .await
        .map_err(|_| WhError::ConnectionError)?;

    sub_socket
        .subscribe(&topic)
        .await
        .map_err(|e| WhError::Other(format!("Failed to subscribe: {e}")))?;

    eprintln!("Connected to stream '{stream}' — type a message and press Enter");

    // Spawn stdin reader task — sends lines via channel (stdin is blocking IO,
    // needs its own task). The main loop handles publish + subscribe + Ctrl-C.
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(100);
    let cancel = CancellationToken::new();
    let cancel_stdin = cancel.clone();

    tokio::spawn(async move {
        let stdin = io::stdin();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        loop {
            tokio::select! {
                biased;
                _ = cancel_stdin.cancelled() => break,
                line = lines.next_line() => {
                    match line {
                        Ok(Some(text)) => {
                            if !text.is_empty() && stdin_tx.send(text).await.is_err() {
                                break;
                            }
                        }
                        Ok(None) => break, // EOF
                        Err(_) => break,
                    }
                }
            }
        }
    });

    // Connection event callback for reconnect — prints status to stderr (RT-B1)
    let stream_name_for_event = stream.to_string();
    let on_event: reconnect::ConnectionEventCallback = Box::new(move |event| match &event {
        ConnectionEvent::Disconnected { reason } => {
            eprintln!("\nConnection lost: {reason}");
        }
        ConnectionEvent::Reconnecting { attempt } => {
            eprintln!("Reconnecting to Wheelhouse (attempt {attempt})...");
        }
        ConnectionEvent::Reconnected => {
            eprintln!(
                "Reconnected — listening on stream '{}'",
                stream_name_for_event
            );
        }
        ConnectionEvent::ReconnectFailed {
            attempts,
            last_error,
        } => {
            eprintln!("Reconnect attempt {attempts} failed: {last_error}");
        }
    });

    let stream_name = stream.to_string();

    // Main event loop — runs in the current task (not spawned) so that
    // &ConnectionEventCallback can be used for reconnect (not Send+Sync).
    loop {
        tokio::select! {
            biased;

            // Shutdown on Ctrl-C (SC-06)
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nDisconnected");
                break;
            }

            // Stdin input — publish to broker
            Some(text) = stdin_rx.recv() => {
                // Build TextMessage proto
                let timestamp_ms = Utc::now().timestamp_millis();
                let text_msg = TextMessage {
                    content: text.clone(),
                    publisher_id: "cli-surface".to_string(),
                    timestamp_ms,
                    user_id: cli_user_id.clone(),
                    reply_to_user_id: String::new(),
                };

                // Wrap in StreamEnvelope
                let envelope = StreamEnvelope {
                    stream_name: stream_name.clone(),
                    object_id: uuid::Uuid::new_v4().to_string(),
                    type_url: "wheelhouse.v1.TextMessage".to_string(),
                    payload: text_msg.encode_to_vec(),
                    publisher_id: "cli-surface".to_string(),
                    published_at_ms: timestamp_ms,
                    sequence_number: 0, // Broker assigns (FR54)
                };

                let envelope_bytes = envelope.encode_to_vec();

                // Build wire format: stream_name\0envelope_bytes
                let mut wire: Vec<u8> =
                    Vec::with_capacity(stream_name.len() + 1 + envelope_bytes.len());
                wire.extend_from_slice(stream_name.as_bytes());
                wire.push(0);
                wire.extend_from_slice(&envelope_bytes);

                // Publish via PUB socket
                let msg = ZmqMessage::from(wire);
                if let Err(e) = pub_socket.send(msg).await {
                    tracing::error!("Failed to publish: {e}");
                    break;
                }

                // Echo user's own message locally (Task 2.4)
                let surface_msg = SurfaceMessage {
                    content: text,
                    publisher: "you".to_string(),
                    timestamp: Utc::now().to_rfc3339(),
                };
                let formatted = format_message(&surface_msg, output_format);
                println!("{formatted}");
            }

            // Subscribe — receive messages from broker
            result = sub_socket.recv() => {
                match result {
                    Ok(msg) => {
                        let raw: Vec<u8> = msg.try_into().unwrap_or_default();

                        // Strip stream_name\0 prefix
                        let Some(null_pos) = raw.iter().position(|&b| b == 0) else {
                            continue;
                        };
                        let payload = &raw[null_pos + 1..];

                        // Decode StreamEnvelope
                        let envelope = match StreamEnvelope::decode(payload) {
                            Ok(e) => e,
                            Err(_) => continue,
                        };

                        // Filter: only display TextMessage types (Task 3.2)
                        if envelope.type_url != "wheelhouse.v1.TextMessage" {
                            continue;
                        }

                        // Filter: skip own messages echoed back by broker (Task 3.3)
                        if envelope.publisher_id == "cli-surface" {
                            continue;
                        }

                        // Decode inner TextMessage
                        let text_msg = match TextMessage::decode(envelope.payload.as_slice()) {
                            Ok(t) => t,
                            Err(_) => continue,
                        };

                        // Convert to SurfaceMessage for display (Task 3.4)
                        let timestamp =
                            chrono::DateTime::from_timestamp_millis(envelope.published_at_ms)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_else(|| "unknown".to_string());

                        let surface_msg = SurfaceMessage {
                            content: text_msg.content,
                            publisher: envelope.publisher_id,
                            timestamp,
                        };

                        let formatted = format_message(&surface_msg, output_format);
                        println!("{formatted}");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "subscribe recv error — attempting reconnect");

                        // Fire disconnected event before reconnect (ADR-011)
                        on_event(ConnectionEvent::Disconnected {
                            reason: format!("Receive error: {e}"),
                        });

                        // Reconnect with exponential backoff (ADR-011, NFR-R4)
                        match reconnect::reconnect_subscribe(
                            &pub_endpoint,
                            &topic,
                            &cancel,
                            Some(&on_event),
                        )
                        .await
                        {
                            Ok(new_socket) => {
                                sub_socket = new_socket;
                            }
                            Err(reconnect::ReconnectError::Cancelled) => {
                                eprintln!("\nDisconnected");
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // Graceful shutdown (SC-06)
    cancel.cancel();

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

    #[test]
    fn test_generate_cli_user_id_format() {
        let user_id = generate_cli_user_id();
        assert!(
            user_id.starts_with("cli-"),
            "CLI user ID should start with 'cli-', got: {user_id}"
        );
    }

    #[test]
    fn test_resolve_pub_endpoint_default() {
        let endpoint = resolve_pub_endpoint();
        assert!(
            endpoint.starts_with("tcp://"),
            "endpoint should be tcp://, got: {endpoint}"
        );
    }

    #[test]
    fn test_resolve_sub_endpoint_default() {
        let endpoint = resolve_sub_endpoint();
        assert!(
            endpoint.starts_with("tcp://"),
            "endpoint should be tcp://, got: {endpoint}"
        );
    }

    #[tokio::test]
    async fn test_probe_broker_liveness_fails_when_no_broker() {
        // No broker running — probe should fail with ConnectionError
        // Use an unlikely port to avoid conflicts
        std::env::set_var("WH_CONTROL_ENDPOINT", "tcp://127.0.0.1:19999");
        let result = probe_broker_liveness().await;
        std::env::remove_var("WH_CONTROL_ENDPOINT");
        assert!(
            result.is_err(),
            "probe should fail when no broker is running"
        );
    }
}
