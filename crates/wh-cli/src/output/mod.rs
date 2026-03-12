//! Output formatting module (SCV-05).
//!
//! All CLI output is routed through this module's format switch.
//! Never serialize JSON directly in command handlers.

pub mod error;

use serde::Serialize;
use wh_proto::TextMessage;

/// Output format selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    /// Parse format string from CLI flag.
    pub fn from_str_value(s: &str) -> Result<Self, String> {
        match s {
            "human" => Ok(OutputFormat::Human),
            "json" => Ok(OutputFormat::Json),
            other => Err(format!("invalid format '{other}': expected 'human' or 'json'")),
        }
    }
}

/// JSON envelope for message output (ADR-014 JSON contract).
/// All fields snake_case (SCV-01), schema version always present.
#[derive(Debug, Serialize)]
pub struct JsonMessageEnvelope {
    pub v: u32,
    pub status: String,
    pub data: JsonMessageData,
}

/// Inner data for JSON message output.
#[derive(Debug, Serialize)]
pub struct JsonMessageData {
    pub publisher: String,
    pub timestamp: String,
    pub content: String,
}

/// Format a TextMessage for display according to the selected output format.
///
/// Human format: `[timestamp] publisher: content`
/// JSON format: `{ "v": 1, "status": "ok", "data": { ... } }`
pub fn format_message(msg: &TextMessage, format: OutputFormat) -> String {
    match format {
        OutputFormat::Human => {
            format!("[{}] {}: {}", msg.timestamp, msg.publisher, msg.content)
        }
        OutputFormat::Json => {
            let envelope = JsonMessageEnvelope {
                v: 1,
                status: "ok".to_string(),
                data: JsonMessageData {
                    publisher: msg.publisher.clone(),
                    timestamp: msg.timestamp.clone(),
                    content: msg.content.clone(),
                },
            };
            // Safe: our struct only contains strings and u32
            serde_json::to_string(&envelope).expect("JSON serialization should not fail")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message() -> TextMessage {
        TextMessage {
            content: "hello world".to_string(),
            publisher: "agent-1".to_string(),
            timestamp: "2026-03-12T10:30:00Z".to_string(),
        }
    }

    #[test]
    fn test_human_format() {
        let msg = sample_message();
        let output = format_message(&msg, OutputFormat::Human);
        assert_eq!(output, "[2026-03-12T10:30:00Z] agent-1: hello world");
    }

    #[test]
    fn test_json_format_valid_json() {
        let msg = sample_message();
        let output = format_message(&msg, OutputFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.is_object());
    }

    #[test]
    fn test_json_format_has_required_fields() {
        let msg = sample_message();
        let output = format_message(&msg, OutputFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(parsed["v"], 1);
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["publisher"], "agent-1");
        assert_eq!(parsed["data"]["timestamp"], "2026-03-12T10:30:00Z");
        assert_eq!(parsed["data"]["content"], "hello world");
    }

    #[test]
    fn test_json_format_snake_case_fields() {
        let msg = sample_message();
        let output = format_message(&msg, OutputFormat::Json);
        // Ensure no camelCase (SCV-01)
        assert!(!output.contains("Publisher"));
        assert!(!output.contains("Timestamp"));
        assert!(!output.contains("Content"));
        assert!(output.contains("\"publisher\""));
        assert!(output.contains("\"timestamp\""));
        assert!(output.contains("\"content\""));
    }

    #[test]
    fn test_output_format_from_str() {
        assert_eq!(OutputFormat::from_str_value("human").unwrap(), OutputFormat::Human);
        assert_eq!(OutputFormat::from_str_value("json").unwrap(), OutputFormat::Json);
        assert!(OutputFormat::from_str_value("xml").is_err());
        assert!(OutputFormat::from_str_value("").is_err());
    }
}
