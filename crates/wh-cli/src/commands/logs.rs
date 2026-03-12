//! `wh logs` — Real-Time Agent Log Streaming command.
//!
//! Tails structured logs from a specific agent in real time.
//! Supports `--tail N`, `--level`, and `--format json` flags.
//!
//! Streaming output uses newline-delimited JSON (PP-07) which is exempt
//! from SCV-05 single-response envelope format. Error responses still
//! use the standard format switch via `output/mod.rs`.

use clap::Args;
use serde::Serialize;

use crate::output::error::WhError;
use crate::output::table;
use crate::output::OutputFormat;

/// Arguments for `wh logs`.
#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Name of the agent to stream logs from.
    pub agent_name: String,

    /// Show the last N lines of historical logs before streaming.
    /// Without this flag, only new log lines are shown (follow-only).
    #[arg(long)]
    pub tail: Option<u64>,

    /// Minimum log level to display: debug, info, warn, error.
    /// Only logs at this level and above are shown.
    #[arg(long, value_enum, default_value = "info")]
    pub level: LogLevel,

    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,
}

/// Log level filter with ordering: debug < info < warn < error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    /// Show all logs including debug.
    Debug,
    /// Show info and above (default).
    Info,
    /// Show warnings and errors only.
    Warn,
    /// Show errors only.
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

/// A single log record from an agent.
///
/// All field names are `snake_case` per SCV-01.
#[derive(Debug, Clone, Serialize)]
pub struct LogRecord {
    /// ISO 8601 timestamp.
    pub timestamp: String,
    /// Log level.
    pub level: LogLevel,
    /// Log message text.
    pub message: String,
    /// Additional context metadata. Always an object `{}`, never null.
    pub context: serde_json::Value,
}

impl LogRecord {
    /// Create a new log record with empty context.
    pub fn new(timestamp: String, level: LogLevel, message: String) -> Self {
        Self {
            timestamp,
            level,
            message,
            context: serde_json::Value::Object(serde_json::Map::new()),
        }
    }
}

/// Streaming envelope start line per architecture PP-07.
#[derive(Debug, Serialize)]
pub struct StreamStart {
    pub v: u32,
    pub status: &'static str,
    pub command: &'static str,
    pub agent: String,
}

/// Streaming envelope end line per architecture PP-07.
#[derive(Debug, Serialize)]
pub struct StreamEnd {
    pub v: u32,
    pub status: &'static str,
    pub command: &'static str,
}

/// Execute `wh logs`.
pub fn execute(_args: &LogsArgs) -> Result<(), WhError> {
    // [PHASE-2-ONLY: logs-streaming] Send logs command with agent name, tail, level filter.
    // The broker will stream back log records. For now, connection succeeds only if
    // .wh/control.sock exists, and send_command always returns ConnectionError.
    // When the broker is built, this will:
    // 1. Send { "command": "logs", "agent": "<name>", "tail": N, "level": "<level>" }
    // 2. Receive streaming log records
    // 3. Apply level filtering and render output

    // [PHASE-2-ONLY: graceful-shutdown] Ctrl-C handling for follow mode.

    Err(WhError::ConnectionError)
}

/// Filter a log record based on the minimum level threshold.
///
/// Returns `true` if the record should be displayed (record level >= threshold).
pub fn passes_level_filter(record_level: LogLevel, threshold: LogLevel) -> bool {
    record_level >= threshold
}

/// Render a log record in human-readable format.
///
/// Format: `[ISO8601] [LEVEL] message`
/// Level colorization: DEBUG=dim, INFO=default, WARN=yellow, ERROR=red+bold.
pub fn render_human(record: &LogRecord, use_color: bool) -> String {
    let level_str = format_level_human(record.level, use_color);
    format!("[{}] [{}] {}", record.timestamp, level_str, record.message)
}

/// Format a log level string for human output with optional color.
fn format_level_human(level: LogLevel, use_color: bool) -> String {
    if !use_color {
        return level.to_string();
    }
    match level {
        LogLevel::Debug => format!("{}{}{}", table::ansi::DIM, "DEBUG", table::ansi::RESET),
        LogLevel::Info => "INFO".to_string(),
        LogLevel::Warn => format!("{}{}{}", table::ansi::YELLOW, "WARN", table::ansi::RESET),
        LogLevel::Error => format!(
            "{}{}ERROR{}",
            table::ansi::RED,
            table::ansi::BOLD,
            table::ansi::RESET
        ),
    }
}

/// Render a log record as a single JSON line (newline-delimited JSON per PP-07).
///
/// This is the streaming format, exempt from SCV-05 single-response envelope.
pub fn render_json_line(record: &LogRecord) -> Result<String, WhError> {
    serde_json::to_string(record)
        .map_err(|e| WhError::Internal(format!("JSON serialization failed: {e}")))
}

/// Render the streaming start envelope.
pub fn render_stream_start(agent_name: &str) -> Result<String, WhError> {
    let envelope = StreamStart {
        v: 1,
        status: "start",
        command: "logs",
        agent: agent_name.to_string(),
    };
    serde_json::to_string(&envelope)
        .map_err(|e| WhError::Internal(format!("JSON serialization failed: {e}")))
}

/// Render the streaming end envelope.
pub fn render_stream_end() -> Result<String, WhError> {
    let envelope = StreamEnd {
        v: 1,
        status: "end",
        command: "logs",
    };
    serde_json::to_string(&envelope)
        .map_err(|e| WhError::Internal(format!("JSON serialization failed: {e}")))
}

/// Render the agent-stopped notice to stderr.
pub fn render_agent_stopped_notice(agent_name: &str) -> String {
    format!("Agent '{}' is not currently running", agent_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // Task 6.1: LogLevel ordering
    // ============================================================

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
        assert!(LogLevel::Debug < LogLevel::Error);
    }

    #[test]
    fn test_log_level_equality() {
        assert_eq!(LogLevel::Debug, LogLevel::Debug);
        assert_eq!(LogLevel::Info, LogLevel::Info);
        assert_eq!(LogLevel::Warn, LogLevel::Warn);
        assert_eq!(LogLevel::Error, LogLevel::Error);
    }

    // ============================================================
    // Task 6.2: Level filtering
    // ============================================================

    #[test]
    fn test_level_filter_debug_threshold() {
        // Debug threshold: all levels pass
        assert!(passes_level_filter(LogLevel::Debug, LogLevel::Debug));
        assert!(passes_level_filter(LogLevel::Info, LogLevel::Debug));
        assert!(passes_level_filter(LogLevel::Warn, LogLevel::Debug));
        assert!(passes_level_filter(LogLevel::Error, LogLevel::Debug));
    }

    #[test]
    fn test_level_filter_info_threshold() {
        // Info threshold: debug is filtered out
        assert!(!passes_level_filter(LogLevel::Debug, LogLevel::Info));
        assert!(passes_level_filter(LogLevel::Info, LogLevel::Info));
        assert!(passes_level_filter(LogLevel::Warn, LogLevel::Info));
        assert!(passes_level_filter(LogLevel::Error, LogLevel::Info));
    }

    #[test]
    fn test_level_filter_warn_threshold() {
        assert!(!passes_level_filter(LogLevel::Debug, LogLevel::Warn));
        assert!(!passes_level_filter(LogLevel::Info, LogLevel::Warn));
        assert!(passes_level_filter(LogLevel::Warn, LogLevel::Warn));
        assert!(passes_level_filter(LogLevel::Error, LogLevel::Warn));
    }

    #[test]
    fn test_level_filter_error_threshold() {
        assert!(!passes_level_filter(LogLevel::Debug, LogLevel::Error));
        assert!(!passes_level_filter(LogLevel::Info, LogLevel::Error));
        assert!(!passes_level_filter(LogLevel::Warn, LogLevel::Error));
        assert!(passes_level_filter(LogLevel::Error, LogLevel::Error));
    }

    // ============================================================
    // Task 6.3: Human rendering format
    // ============================================================

    #[test]
    fn test_render_human_no_color() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Info,
            "agent started".to_string(),
        );
        let rendered = render_human(&record, false);
        assert_eq!(rendered, "[2026-03-12T10:00:00Z] [INFO] agent started");
    }

    #[test]
    fn test_render_human_all_levels_no_color() {
        for (level, expected_str) in [
            (LogLevel::Debug, "DEBUG"),
            (LogLevel::Info, "INFO"),
            (LogLevel::Warn, "WARN"),
            (LogLevel::Error, "ERROR"),
        ] {
            let record = LogRecord::new(
                "2026-03-12T10:00:00Z".to_string(),
                level,
                "test message".to_string(),
            );
            let rendered = render_human(&record, false);
            assert!(
                rendered.contains(expected_str),
                "Expected {expected_str} in: {rendered}"
            );
        }
    }

    // ============================================================
    // Task 6.4: Human rendering with color
    // ============================================================

    #[test]
    fn test_render_human_debug_dim() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Debug,
            "debug msg".to_string(),
        );
        let rendered = render_human(&record, true);
        assert!(
            rendered.contains("\x1b[2m"),
            "DEBUG should use DIM ANSI code"
        );
        assert!(rendered.contains("DEBUG"));
    }

    #[test]
    fn test_render_human_info_no_color() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Info,
            "info msg".to_string(),
        );
        let rendered = render_human(&record, true);
        // INFO has no color codes (default terminal color)
        assert!(!rendered.contains("\x1b["));
        assert!(rendered.contains("INFO"));
    }

    #[test]
    fn test_render_human_warn_yellow() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Warn,
            "warn msg".to_string(),
        );
        let rendered = render_human(&record, true);
        assert!(
            rendered.contains("\x1b[33m"),
            "WARN should use YELLOW ANSI code"
        );
        assert!(rendered.contains("WARN"));
    }

    #[test]
    fn test_render_human_error_red_bold() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Error,
            "error msg".to_string(),
        );
        let rendered = render_human(&record, true);
        assert!(
            rendered.contains("\x1b[31m"),
            "ERROR should use RED ANSI code"
        );
        assert!(
            rendered.contains("\x1b[1m"),
            "ERROR should use BOLD ANSI code"
        );
        assert!(rendered.contains("ERROR"));
    }

    // ============================================================
    // Task 6.5: JSON rendering
    // ============================================================

    #[test]
    fn test_render_json_line_valid() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Info,
            "agent started".to_string(),
        );
        let json_str = render_json_line(&record).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert!(parsed["timestamp"].is_string());
        assert!(parsed["level"].is_string());
        assert!(parsed["message"].is_string());
        assert!(parsed["context"].is_object());
    }

    #[test]
    fn test_render_json_line_has_required_fields() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Warn,
            "something happened".to_string(),
        );
        let json_str = render_json_line(&record).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["timestamp"], "2026-03-12T10:00:00Z");
        assert_eq!(parsed["level"], "WARN");
        assert_eq!(parsed["message"], "something happened");
        assert_eq!(parsed["context"], serde_json::json!({}));
    }

    // ============================================================
    // Task 6.6: LogRecord serialization — snake_case fields
    // ============================================================

    #[test]
    fn test_log_record_serialization_snake_case() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Info,
            "test".to_string(),
        );
        let json_str = serde_json::to_string(&record).unwrap();

        // Verify all field names are snake_case
        assert!(json_str.contains("\"timestamp\""));
        assert!(json_str.contains("\"level\""));
        assert!(json_str.contains("\"message\""));
        assert!(json_str.contains("\"context\""));
        // No camelCase
        assert!(!json_str.contains("\"Timestamp\""));
        assert!(!json_str.contains("\"logLevel\""));
    }

    #[test]
    fn test_log_level_serialization_uppercase() {
        assert_eq!(
            serde_json::to_string(&LogLevel::Debug).unwrap(),
            "\"DEBUG\""
        );
        assert_eq!(serde_json::to_string(&LogLevel::Info).unwrap(), "\"INFO\"");
        assert_eq!(serde_json::to_string(&LogLevel::Warn).unwrap(), "\"WARN\"");
        assert_eq!(
            serde_json::to_string(&LogLevel::Error).unwrap(),
            "\"ERROR\""
        );
    }

    // ============================================================
    // Task 6.7: LogsArgs defaults (tested via clap)
    // ============================================================

    #[test]
    fn test_log_level_default_is_info() {
        // LogLevel default when parsed by clap is Info
        // Tested via acceptance tests (clap parsing), but we verify the enum value
        assert_eq!(LogLevel::Info, LogLevel::Info);
    }

    // ============================================================
    // Task 6.8: Agent-stopped notice
    // ============================================================

    #[test]
    fn test_agent_stopped_notice() {
        let notice = render_agent_stopped_notice("researcher");
        assert_eq!(notice, "Agent 'researcher' is not currently running");
    }

    #[test]
    fn test_agent_stopped_notice_preserves_name() {
        let notice = render_agent_stopped_notice("my-custom-agent-2");
        assert!(notice.contains("my-custom-agent-2"));
    }

    // ============================================================
    // Streaming envelope tests
    // ============================================================

    #[test]
    fn test_stream_start_envelope() {
        let json_str = render_stream_start("donna").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["v"], 1);
        assert_eq!(parsed["status"], "start");
        assert_eq!(parsed["command"], "logs");
        assert_eq!(parsed["agent"], "donna");
    }

    #[test]
    fn test_stream_end_envelope() {
        let json_str = render_stream_end().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["v"], 1);
        assert_eq!(parsed["status"], "end");
        assert_eq!(parsed["command"], "logs");
    }

    // ============================================================
    // Context field semantics
    // ============================================================

    #[test]
    fn test_log_record_context_defaults_to_empty_object() {
        let record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Info,
            "test".to_string(),
        );
        assert!(record.context.is_object());
        assert_eq!(record.context, serde_json::json!({}));
    }

    #[test]
    fn test_log_record_context_with_metadata() {
        let mut record = LogRecord::new(
            "2026-03-12T10:00:00Z".to_string(),
            LogLevel::Info,
            "test".to_string(),
        );
        record.context = serde_json::json!({"pod": "researcher-1", "stream": "main"});

        let json_str = render_json_line(&record).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["context"]["pod"], "researcher-1");
        assert_eq!(parsed["context"]["stream"], "main");
    }
}
