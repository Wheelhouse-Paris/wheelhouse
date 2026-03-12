//! `wh stream tail` — Real-Time Stream Observation command.
//!
//! Tails stream objects in real time with optional type and publisher filtering.
//! Supports `--filter type=<TypeName>`, `--filter publisher=<name>`, `--format json`,
//! `--last N`, and `--verbose` flags.
//!
//! Streaming output uses newline-delimited JSON (PP-07) which is exempt
//! from SCV-05 single-response envelope format. Error responses still
//! use the standard format switch via `output/mod.rs`.

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::output::error::WhError;
use crate::output::OutputFormat;

/// Stream management subcommands.
#[derive(Debug, Subcommand)]
pub enum StreamCommand {
    /// Observe stream objects in real time.
    Tail(StreamTailArgs),
}

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
    ///
    /// Returns an error for missing `=`, empty key/value, or unknown filter keys.
    pub fn parse(s: &str) -> Result<Self, WhError> {
        let (key, value) = s
            .split_once('=')
            .ok_or_else(|| WhError::Other(format!("Invalid filter syntax '{s}'. Expected key=value format (e.g., type=TextMessage)")))?;

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
///
/// All field names are `snake_case` per SCV-01.
/// The `type_name` field serializes as `type` in JSON output via serde rename.
#[derive(Debug, Clone, Serialize)]
pub struct StreamRecord {
    /// ISO 8601 timestamp when the record was observed.
    pub timestamp: String,
    /// Protobuf type short name (e.g., "TextMessage").
    /// Serializes as `type` in JSON per acceptance criteria.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Publisher identity (e.g., "researcher-1").
    pub publisher: String,
    /// Compact JSON payload of the message content.
    pub payload: serde_json::Value,
}

impl StreamRecord {
    /// Create a new stream record.
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

/// Maximum content length before truncation in human format (SCV-07).
const TRUNCATION_LIMIT: usize = 120;

/// Streaming envelope start line per architecture PP-07.
#[derive(Debug, Serialize)]
pub struct StreamTailStart {
    pub v: u32,
    pub status: &'static str,
    pub command: &'static str,
    pub stream: String,
}

/// Streaming envelope end line per architecture PP-07.
#[derive(Debug, Serialize)]
pub struct StreamTailEnd {
    pub v: u32,
    pub status: &'static str,
    pub command: &'static str,
}

/// Execute `wh stream tail`.
pub fn execute(args: &StreamTailArgs) -> Result<(), WhError> {
    // Validate filters before attempting broker connection
    let _filters = parse_filters(&args.filter)?;

    // [PHASE-2-ONLY: --last direct-read] When WAL storage exists,
    // --last N will read SQLite WAL directly via rusqlite (WW-03).

    // [PHASE-2-ONLY: stream-tail-streaming] Send stream_tail command with
    // stream name and filters to the broker via control socket.
    // The broker will stream back records. For now, connection always
    // returns ConnectionError since the broker protocol doesn't exist yet.
    // When the broker is built, this will:
    // 1. Send { "command": "stream_tail", "stream": "<name>", "filters": [...] }
    // 2. Receive streaming stream records
    // 3. Apply filters and render output

    // [PHASE-2-ONLY: graceful-shutdown] Ctrl-C handling for follow mode.
    // When broker exists and streaming is active, Ctrl-C must exit
    // cleanly with code 0 (no error message).

    Err(WhError::ConnectionError)
}

/// Check if a stream record passes all active filters.
///
/// Filters combine with AND logic — all filters must match.
pub fn passes_filters(record: &StreamRecord, filters: &[Filter]) -> bool {
    filters.iter().all(|filter| match filter {
        Filter::Type(type_name) => record.type_name == *type_name,
        Filter::Publisher(publisher) => record.publisher == *publisher,
    })
}

/// Render a stream record in human-readable format.
///
/// Format: `[ISO8601] [TypeName] [publisher] content`
/// Content is truncated at 120 chars with `...` unless verbose is true (SCV-07).
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

/// Convert a JSON payload to a compact string representation.
fn payload_to_string(payload: &serde_json::Value) -> String {
    serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string())
}

/// Truncate content to max_len characters, appending `...` if truncated.
///
/// Uses char boundaries to avoid panicking on multi-byte UTF-8 strings.
fn truncate_content(content: &str, max_len: usize) -> String {
    if content.chars().count() <= max_len {
        content.to_string()
    } else {
        let truncated: String = content.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

/// Render a stream record as a single JSON line (newline-delimited JSON per PP-07).
///
/// This is the streaming format, exempt from SCV-05 single-response envelope.
pub fn render_stream_line_json(record: &StreamRecord) -> Result<String, WhError> {
    serde_json::to_string(record)
        .map_err(|e| WhError::Internal(format!("JSON serialization failed: {e}")))
}

/// Render the streaming start envelope.
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

/// Render the streaming end envelope.
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

    // ============================================================
    // Task 6.1: Filter parsing
    // ============================================================

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

    // ============================================================
    // Task 6.2: Type filtering
    // ============================================================

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

    // ============================================================
    // Task 6.3: Publisher filtering
    // ============================================================

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
        // Both must match
        let filters = vec![
            Filter::Type("TextMessage".to_string()),
            Filter::Publisher("donna".to_string()),
        ];
        assert!(passes_filters(&record, &filters));

        // Type matches but publisher doesn't
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

    // ============================================================
    // Task 6.4: Human rendering format
    // ============================================================

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

    // ============================================================
    // Task 6.5: Content truncation at 120 chars
    // ============================================================

    #[test]
    fn test_truncation_short_content() {
        let content = "short";
        assert_eq!(truncate_content(content, TRUNCATION_LIMIT), "short");
    }

    #[test]
    fn test_truncation_long_content() {
        let content = "a".repeat(200);
        let truncated = truncate_content(&content, TRUNCATION_LIMIT);
        // 120 chars of content + "..." = 123 chars total
        assert_eq!(truncated.chars().count(), TRUNCATION_LIMIT + 3);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_truncation_multibyte_unicode() {
        // Multi-byte UTF-8 chars must not panic on truncation
        let content = "\u{1F600}".repeat(200); // emoji chars (4 bytes each)
        let truncated = truncate_content(&content, TRUNCATION_LIMIT);
        assert!(truncated.ends_with("..."));
        // Should have 120 emoji chars + "..."
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
        assert!(!rendered.ends_with("..."), "Verbose mode should not truncate");
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
        assert!(rendered.contains("..."), "Non-verbose mode should truncate long content");
    }

    // ============================================================
    // Task 6.6: JSON rendering
    // ============================================================

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

    // ============================================================
    // Task 6.7: StreamRecord serialization — snake_case + type rename
    // ============================================================

    #[test]
    fn test_stream_record_serialization_snake_case() {
        let record = StreamRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            "TextMessage".to_string(),
            "donna".to_string(),
            serde_json::json!({"content": "test"}),
        );
        let json_str = serde_json::to_string(&record).unwrap();

        // Verify field names
        assert!(json_str.contains("\"timestamp\""));
        assert!(json_str.contains("\"type\"")); // renamed from type_name
        assert!(json_str.contains("\"publisher\""));
        assert!(json_str.contains("\"payload\""));
        // Must NOT contain the struct field name
        assert!(!json_str.contains("\"type_name\""));
    }

    // ============================================================
    // Task 6.8: StreamTailArgs defaults
    // ============================================================

    // Defaults are tested via acceptance tests (clap parsing).
    // Unit verification of the default format enum value.
    #[test]
    fn test_output_format_default_is_human() {
        assert_eq!(OutputFormat::default(), OutputFormat::Human);
    }

    // ============================================================
    // Task 6.9: Streaming envelope start/end
    // ============================================================

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
        // stream_tail uses "stream", not "agent" (unlike logs)
        let json_str = render_stream_start("main").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.get("agent").is_none(), "stream_tail should not have 'agent' field");
        assert!(parsed.get("stream").is_some(), "stream_tail should have 'stream' field");
    }
}
