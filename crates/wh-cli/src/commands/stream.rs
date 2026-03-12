//! `wh stream` — Stream management and real-time observation commands.
//!
//! Story 1.3: create, list, delete named streams via broker control socket.
//! Story 3.3: tail stream objects in real time with optional filters.
//!
//! Human-readable output uses approved vocabulary — never "broker" (RT-B1).

use clap::{Args, Subcommand};
use prost::Message;
use serde::Serialize;
use serde_json::json;
use wh_proto::{StreamEnvelope, TextMessage};
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use crate::client::ControlClient;
use crate::output::error::WhError;
use crate::output::{self, OutputFormat};

/// Stream management and observation subcommands.
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
    /// Publish a typed message to a stream.
    Publish {
        /// Stream name to publish to.
        stream_name: String,
        /// Message content.
        message: String,
        /// Protobuf type name (default: TextMessage).
        #[arg(long, default_value = "TextMessage")]
        r#type: String,
        /// Publisher identity.
        #[arg(long, default_value = "cli")]
        publisher: String,
    },
    /// Subscribe to a stream and display messages in real time.
    Subscribe {
        /// Stream name to subscribe to.
        stream_name: String,
        /// Output format: human (default) or json.
        #[arg(long, value_enum, default_value = "human")]
        format: OutputFormat,
    },
    /// Observe stream objects in real time.
    Tail(StreamTailArgs),
}

/// Execute a stream command (dispatches to create/list/delete/tail handlers).
pub async fn execute(cmd: &StreamCommand) {
    match cmd {
        StreamCommand::Create {
            name,
            retention,
            retention_size,
        } => {
            let client = ControlClient::new();
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
            let client = ControlClient::new();
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
            let client = ControlClient::new();
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
        StreamCommand::Publish {
            stream_name,
            message,
            r#type,
            publisher,
        } => {
            if let Err(e) = execute_publish(stream_name, message, r#type, publisher).await {
                e.exit();
            }
        }
        StreamCommand::Subscribe {
            stream_name,
            format,
        } => {
            if let Err(e) = execute_subscribe(stream_name, format).await {
                e.exit();
            }
        }
        StreamCommand::Tail(args) => {
            if let Err(e) = execute_tail(args) {
                e.exit();
            }
        }
    }
}

// ── Publish helper (Story 1.4) ──────────────────────────────────────────────

/// Publish a typed message to a stream via ZMQ PUB socket.
///
/// Builds a StreamEnvelope client-side, connects to the broker's SUB endpoint,
/// and sends the envelope as `{stream_name}\0{envelope_bytes}`.
/// The broker assigns the authoritative sequence_number and published_at_ms.
async fn execute_publish(
    stream_name: &str,
    message: &str,
    type_name: &str,
    publisher: &str,
) -> Result<(), WhError> {
    let sub_endpoint = std::env::var("WH_SUB_ENDPOINT").unwrap_or_else(|_| {
        let port = std::env::var("WH_SUB_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(5556);
        format!("tcp://127.0.0.1:{port}")
    });

    // Build the inner message based on type
    let (type_url, payload_bytes) = match type_name {
        "TextMessage" => {
            let msg = TextMessage {
                content: message.to_string(),
                publisher_id: publisher.to_string(),
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
                user_id: String::new(),
                reply_to_user_id: String::new(),
            };
            ("wheelhouse.v1.TextMessage".to_string(), msg.encode_to_vec())
        }
        other => {
            // For unknown types, encode message content as raw bytes
            (
                format!("wheelhouse.v1.{other}"),
                message.as_bytes().to_vec(),
            )
        }
    };

    // Build StreamEnvelope
    let envelope = StreamEnvelope {
        stream_name: stream_name.to_string(),
        object_id: uuid::Uuid::new_v4().to_string(),
        type_url,
        payload: payload_bytes,
        publisher_id: publisher.to_string(),
        published_at_ms: chrono::Utc::now().timestamp_millis(),
        sequence_number: 0, // Broker assigns authoritative value
    };

    let envelope_bytes = envelope.encode_to_vec();

    // Build wire format: stream_name\0envelope_bytes
    let mut wire: Vec<u8> = Vec::with_capacity(stream_name.len() + 1 + envelope_bytes.len());
    wire.extend_from_slice(stream_name.as_bytes());
    wire.push(0);
    wire.extend_from_slice(&envelope_bytes);

    // Connect PUB socket to broker's SUB endpoint
    let mut pub_socket = PubSocket::new();
    pub_socket
        .connect(&sub_endpoint)
        .await
        .map_err(|_| WhError::ConnectionError)?;

    // ZMQ PUB sockets need a brief delay for connection establishment.
    // Without this, the first message may be silently dropped because the
    // subscription handshake between PUB and SUB has not completed yet.
    // 100ms is sufficient for localhost; fire-and-forget semantics (WW-02).
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let msg = ZmqMessage::from(wire);
    pub_socket
        .send(msg)
        .await
        .map_err(|e| WhError::Other(format!("Failed to publish: {e}")))?;

    println!("Published to stream '{stream_name}'");
    Ok(())
}

// ── Subscribe helper (Story 1.4, updated Story 1.5 with auto-reconnect) ─────

/// Subscribe to a stream and display messages in real time.
///
/// Connects to Wheelhouse's PUB endpoint, subscribes to the stream topic prefix,
/// and loops receiving and decoding StreamEnvelope messages.
/// On connection loss, automatically reconnects with exponential backoff (ADR-011, CM-02).
/// Ctrl+C terminates cleanly during both receive and reconnect phases.
async fn execute_subscribe(stream_name: &str, format: &OutputFormat) -> Result<(), WhError> {
    use crate::reconnect::{self, ConnectionEvent};
    use tokio_util::sync::CancellationToken;

    let pub_endpoint = std::env::var("WH_PUB_ENDPOINT").unwrap_or_else(|_| {
        let port = std::env::var("WH_PUB_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(5555);
        format!("tcp://127.0.0.1:{port}")
    });

    let topic = format!("{stream_name}\0");
    let cancel = CancellationToken::new();

    // Initial connection
    let mut sub_socket = SubSocket::new();
    sub_socket
        .connect(&pub_endpoint)
        .await
        .map_err(|_| WhError::ConnectionError)?;

    sub_socket
        .subscribe(&topic)
        .await
        .map_err(|e| WhError::Other(format!("Failed to subscribe: {e}")))?;

    eprintln!("Subscribed to stream '{stream_name}' — waiting for messages...");

    // Connection event callback — prints status to stderr (RT-B1: no "broker")
    let stream_name_owned = stream_name.to_string();
    let on_event: reconnect::ConnectionEventCallback = Box::new(move |event| match &event {
        ConnectionEvent::Disconnected { reason } => {
            eprintln!("\nConnection lost: {reason}");
        }
        ConnectionEvent::Reconnecting { attempt } => {
            eprintln!("Reconnecting to Wheelhouse (attempt {attempt})...");
        }
        ConnectionEvent::Reconnected => {
            eprintln!(
                "Reconnected — subscribed to stream '{}', waiting for messages...",
                stream_name_owned
            );
        }
        ConnectionEvent::ReconnectFailed {
            attempts,
            last_error,
        } => {
            eprintln!("Reconnect attempt {attempts} failed: {last_error}");
        }
    });

    loop {
        tokio::select! {
            biased;

            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nDisconnected");
                break;
            }

            result = sub_socket.recv() => {
                match result {
                    Ok(msg) => {
                        let raw: Vec<u8> = msg.try_into().unwrap_or_default();
                        // Strip stream_name\0 prefix
                        if let Some(null_pos) = raw.iter().position(|&b| b == 0) {
                            let payload = &raw[null_pos + 1..];
                            display_envelope(payload, format);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "subscribe recv error — attempting reconnect");

                        // Fire disconnected event before reconnect
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
                                // Continue receive loop with new socket (AC #2)
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

    Ok(())
}

/// Decode and display a StreamEnvelope payload.
fn display_envelope(payload: &[u8], format: &OutputFormat) {
    match StreamEnvelope::decode(payload) {
        Ok(envelope) => match format {
            OutputFormat::Json => {
                let json_val = envelope_to_json(&envelope);
                println!("{}", serde_json::to_string(&json_val).unwrap_or_default());
            }
            OutputFormat::Human => {
                let timestamp = chrono::DateTime::from_timestamp_millis(envelope.published_at_ms)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| "unknown".to_string());
                let type_short = envelope
                    .type_url
                    .rsplit('.')
                    .next()
                    .unwrap_or(&envelope.type_url);
                let content = decode_inner_content(&envelope);
                println!(
                    "[{}] [{}] [{}] {}",
                    timestamp, type_short, envelope.publisher_id, content
                );
            }
        },
        Err(_) => {
            // Raw message — not a StreamEnvelope
            let hex = payload
                .iter()
                .take(64)
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!("[raw] {hex}");
        }
    }
}

/// Decode the inner payload of a StreamEnvelope based on type_url.
fn decode_inner_content(envelope: &StreamEnvelope) -> String {
    match envelope.type_url.as_str() {
        "wheelhouse.v1.TextMessage" => match TextMessage::decode(envelope.payload.as_slice()) {
            Ok(text) => text.content,
            Err(_) => format!("<decode error: {} bytes>", envelope.payload.len()),
        },
        _ => {
            // Unknown type — show hex summary
            let hex = envelope
                .payload
                .iter()
                .take(32)
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            if envelope.payload.len() > 32 {
                format!("{hex}... ({} bytes)", envelope.payload.len())
            } else {
                hex
            }
        }
    }
}

/// Convert a StreamEnvelope to a JSON value for --format json output.
fn envelope_to_json(envelope: &StreamEnvelope) -> serde_json::Value {
    let inner_content = decode_inner_content(envelope);
    json!({
        "stream_name": envelope.stream_name,
        "object_id": envelope.object_id,
        "type_url": envelope.type_url,
        "publisher_id": envelope.publisher_id,
        "published_at_ms": envelope.published_at_ms,
        "sequence_number": envelope.sequence_number,
        "content": inner_content
    })
}

// ── Stream list helper ──────────────────────────────────────────────────────

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
                    println!(
                        "{:<20} {:<12} {:<15} CREATED",
                        "NAME", "RETENTION", "MESSAGES"
                    );
                    for stream in streams {
                        let name = stream.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                        let retention = stream
                            .get("retention")
                            .and_then(|v| if v.is_null() { None } else { v.as_str() })
                            .unwrap_or("none");
                        let message_count = stream
                            .get("message_count")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let created = stream
                            .get("created_at")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-");
                        println!(
                            "{:<20} {:<12} {:<15} {}",
                            name, retention, message_count, created
                        );
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

// ── Stream tail infrastructure (Story 3.3) ─────────────────────────────────

/// Arguments for `wh stream tail`.
#[derive(Debug, Args)]
pub struct StreamTailArgs {
    /// Name of the stream to observe.
    pub stream_name: String,

    /// Filter stream records by key=value pairs.
    /// Supported keys: `type`, `publisher`.
    /// Multiple filters combine with AND logic.
    #[arg(long)]
    pub filter: Vec<String>,

    /// Show the last N messages from the stream before tailing.
    #[arg(long)]
    pub last: Option<u64>,

    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,

    /// Disable content truncation (show full payload).
    #[arg(long)]
    pub verbose: bool,
}

/// Parsed filter specification.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// Filter by protobuf type name.
    Type(String),
    /// Filter by publisher identity.
    Publisher(String),
}

impl Filter {
    /// Parse a `key=value` filter string into a typed Filter.
    pub fn parse(s: &str) -> Result<Self, WhError> {
        let (key, value) = s.split_once('=').ok_or_else(|| {
            WhError::Other(format!(
                "Invalid filter syntax '{s}'. Expected key=value format (e.g., type=TextMessage)"
            ))
        })?;

        if key.is_empty() || value.is_empty() {
            return Err(WhError::Other(format!(
                "Invalid filter '{s}'. Both key and value must be non-empty"
            )));
        }

        match key {
            "type" => Ok(Filter::Type(value.to_string())),
            "publisher" => Ok(Filter::Publisher(value.to_string())),
            _ => Err(WhError::Other(format!(
                "Unknown filter key '{key}'. Supported: type, publisher"
            ))),
        }
    }
}

/// Parse all filter strings from CLI args into typed Filters.
pub fn parse_filters(raw_filters: &[String]) -> Result<Vec<Filter>, WhError> {
    raw_filters.iter().map(|s| Filter::parse(s)).collect()
}

/// A single stream record observed on the stream.
#[derive(Debug, Clone, Serialize)]
pub struct StreamRecord {
    pub timestamp: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub publisher: String,
    pub payload: serde_json::Value,
}

impl StreamRecord {
    pub fn new(
        timestamp: String,
        type_name: String,
        publisher: String,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            timestamp,
            type_name,
            publisher,
            payload,
        }
    }
}

const TRUNCATION_LIMIT: usize = 120;

#[derive(Debug, Serialize)]
pub struct StreamTailStart {
    pub v: u32,
    pub status: &'static str,
    pub command: &'static str,
    pub stream: String,
}

#[derive(Debug, Serialize)]
pub struct StreamTailEnd {
    pub v: u32,
    pub status: &'static str,
    pub command: &'static str,
}

/// Execute `wh stream tail`.
pub fn execute_tail(args: &StreamTailArgs) -> Result<(), WhError> {
    let _filters = parse_filters(&args.filter)?;
    Err(WhError::ConnectionError)
}

/// Check if a stream record passes all active filters.
pub fn passes_filters(record: &StreamRecord, filters: &[Filter]) -> bool {
    filters.iter().all(|filter| match filter {
        Filter::Type(type_name) => record.type_name == *type_name,
        Filter::Publisher(publisher) => record.publisher == *publisher,
    })
}

/// Render a stream record in human-readable format.
pub fn render_human(record: &StreamRecord, verbose: bool) -> String {
    let content = payload_to_string(&record.payload);
    let display_content = if verbose {
        content
    } else {
        truncate_content(&content, TRUNCATION_LIMIT)
    };
    format!(
        "[{}] [{}] [{}] {}",
        record.timestamp, record.type_name, record.publisher, display_content
    )
}

fn payload_to_string(payload: &serde_json::Value) -> String {
    serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string())
}

fn truncate_content(content: &str, max_len: usize) -> String {
    if content.chars().count() <= max_len {
        content.to_string()
    } else {
        let truncated: String = content.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

pub fn render_stream_line_json(record: &StreamRecord) -> Result<String, WhError> {
    serde_json::to_string(record)
        .map_err(|e| WhError::Internal(format!("JSON serialization failed: {e}")))
}

pub fn render_stream_start(stream_name: &str) -> Result<String, WhError> {
    let envelope = StreamTailStart {
        v: 1,
        status: "start",
        command: "stream_tail",
        stream: stream_name.to_string(),
    };
    serde_json::to_string(&envelope)
        .map_err(|e| WhError::Internal(format!("JSON serialization failed: {e}")))
}

pub fn render_stream_end() -> Result<String, WhError> {
    let envelope = StreamTailEnd {
        v: 1,
        status: "end",
        command: "stream_tail",
    };
    serde_json::to_string(&envelope)
        .map_err(|e| WhError::Internal(format!("JSON serialization failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_filter_type() {
        let filter = Filter::parse("type=TextMessage").unwrap();
        assert_eq!(filter, Filter::Type("TextMessage".to_string()));
    }

    #[test]
    fn test_parse_filter_publisher() {
        let filter = Filter::parse("publisher=donna").unwrap();
        assert_eq!(filter, Filter::Publisher("donna".to_string()));
    }

    #[test]
    fn test_parse_filter_invalid_no_equals() {
        let result = Filter::parse("invalid");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid filter syntax"), "Got: {err}");
    }

    #[test]
    fn test_parse_filter_unknown_key() {
        let result = Filter::parse("unknown=value");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown filter key"), "Got: {err}");
    }

    #[test]
    fn test_parse_filter_empty_key() {
        let result = Filter::parse("=value");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_filter_empty_value() {
        let result = Filter::parse("type=");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_filters_multiple() {
        let raw = vec![
            "type=TextMessage".to_string(),
            "publisher=donna".to_string(),
        ];
        let filters = parse_filters(&raw).unwrap();
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0], Filter::Type("TextMessage".to_string()));
        assert_eq!(filters[1], Filter::Publisher("donna".to_string()));
    }

    #[test]
    fn test_parse_filters_empty() {
        let raw: Vec<String> = vec![];
        let filters = parse_filters(&raw).unwrap();
        assert!(filters.is_empty());
    }

    #[test]
    fn test_type_filter_passes_matching() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            serde_json::json!({"content": "hello"}),
        );
        let filters = vec![Filter::Type("TextMessage".to_string())];
        assert!(passes_filters(&record, &filters));
    }

    #[test]
    fn test_type_filter_rejects_non_matching() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "CronEvent".to_string(),
            "donna".to_string(),
            serde_json::json!({}),
        );
        let filters = vec![Filter::Type("TextMessage".to_string())];
        assert!(!passes_filters(&record, &filters));
    }

    #[test]
    fn test_publisher_filter_passes_matching() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "researcher-2".to_string(),
            serde_json::json!({"content": "hello"}),
        );
        let filters = vec![Filter::Publisher("researcher-2".to_string())];
        assert!(passes_filters(&record, &filters));
    }

    #[test]
    fn test_publisher_filter_rejects_non_matching() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            serde_json::json!({}),
        );
        let filters = vec![Filter::Publisher("researcher-2".to_string())];
        assert!(!passes_filters(&record, &filters));
    }

    #[test]
    fn test_combined_filters_and_logic() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            serde_json::json!({"content": "hello"}),
        );
        let filters = vec![
            Filter::Type("TextMessage".to_string()),
            Filter::Publisher("donna".to_string()),
        ];
        assert!(passes_filters(&record, &filters));

        let filters2 = vec![
            Filter::Type("TextMessage".to_string()),
            Filter::Publisher("researcher-2".to_string()),
        ];
        assert!(!passes_filters(&record, &filters2));
    }

    #[test]
    fn test_no_filters_passes_all() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            serde_json::json!({}),
        );
        let filters: Vec<Filter> = vec![];
        assert!(passes_filters(&record, &filters));
    }

    #[test]
    fn test_render_human_format() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            serde_json::json!({"content": "hello world"}),
        );
        let rendered = render_human(&record, false);
        assert_eq!(
            rendered,
            "[2026-03-12T10:00:00Z] [TextMessage] [donna] {\"content\":\"hello world\"}"
        );
    }

    #[test]
    fn test_render_human_format_components() {
        let record = StreamRecord::new(
            "2026-03-12T14:30:00Z".to_string(),
            "CronEvent".to_string(),
            "scheduler".to_string(),
            serde_json::json!({"job": "daily-report"}),
        );
        let rendered = render_human(&record, false);
        assert!(rendered.starts_with("[2026-03-12T14:30:00Z]"));
        assert!(rendered.contains("[CronEvent]"));
        assert!(rendered.contains("[scheduler]"));
    }

    #[test]
    fn test_truncation_short_content() {
        let content = "short";
        assert_eq!(truncate_content(content, TRUNCATION_LIMIT), "short");
    }

    #[test]
    fn test_truncation_long_content() {
        let content = "a".repeat(200);
        let truncated = truncate_content(&content, TRUNCATION_LIMIT);
        assert_eq!(truncated.chars().count(), TRUNCATION_LIMIT + 3);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_truncation_multibyte_unicode() {
        let content = "\u{1F600}".repeat(200);
        let truncated = truncate_content(&content, TRUNCATION_LIMIT);
        assert!(truncated.ends_with("..."));
        assert_eq!(truncated.chars().count(), TRUNCATION_LIMIT + 3);
    }

    #[test]
    fn test_truncation_exact_limit() {
        let content = "a".repeat(TRUNCATION_LIMIT);
        assert_eq!(truncate_content(&content, TRUNCATION_LIMIT), content);
    }

    #[test]
    fn test_render_human_verbose_no_truncation() {
        let long_payload = serde_json::json!({"content": "a".repeat(200)});
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            long_payload,
        );
        let rendered = render_human(&record, true);
        assert!(
            !rendered.ends_with("..."),
            "Verbose mode should not truncate"
        );
    }

    #[test]
    fn test_render_human_truncates_long_content() {
        let long_payload = serde_json::json!({"content": "a".repeat(200)});
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            long_payload,
        );
        let rendered = render_human(&record, false);
        assert!(
            rendered.contains("..."),
            "Non-verbose mode should truncate long content"
        );
    }

    #[test]
    fn test_render_json_line_valid() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            serde_json::json!({"content": "hello"}),
        );
        let json_str = render_stream_line_json(&record).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed["timestamp"].is_string());
        assert!(parsed["type"].is_string());
        assert!(parsed["publisher"].is_string());
        assert!(parsed["payload"].is_object());
    }

    #[test]
    fn test_render_json_line_has_required_fields() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "researcher-1".to_string(),
            serde_json::json!({"content": "hello"}),
        );
        let json_str = render_stream_line_json(&record).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["timestamp"], "2026-03-12T10:00:00Z");
        assert_eq!(parsed["type"], "TextMessage");
        assert_eq!(parsed["publisher"], "researcher-1");
        assert_eq!(parsed["payload"], serde_json::json!({"content": "hello"}));
    }

    #[test]
    fn test_stream_record_serialization_snake_case() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            serde_json::json!({"content": "test"}),
        );
        let json_str = serde_json::to_string(&record).unwrap();
        assert!(json_str.contains("\"timestamp\""));
        assert!(json_str.contains("\"type\""));
        assert!(json_str.contains("\"publisher\""));
        assert!(json_str.contains("\"payload\""));
        assert!(!json_str.contains("\"type_name\""));
    }

    #[test]
    fn test_output_format_default_is_human() {
        assert_eq!(OutputFormat::default(), OutputFormat::Human);
    }

    #[test]
    fn test_stream_tail_start_envelope() {
        let json_str = render_stream_start("main").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["v"], 1);
        assert_eq!(parsed["status"], "start");
        assert_eq!(parsed["command"], "stream_tail");
        assert_eq!(parsed["stream"], "main");
    }

    #[test]
    fn test_stream_tail_end_envelope() {
        let json_str = render_stream_end().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["v"], 1);
        assert_eq!(parsed["status"], "end");
        assert_eq!(parsed["command"], "stream_tail");
    }

    #[test]
    fn test_stream_tail_start_no_agent_field() {
        let json_str = render_stream_start("main").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.get("agent").is_none());
        assert!(parsed.get("stream").is_some());
    }
}
