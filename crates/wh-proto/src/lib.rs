//! wh-proto: Shared Protobuf-compatible types for Wheelhouse.
//!
//! These are stub types used until real Protobuf codegen is available (Epic 1).
//! All types use serde derives for JSON serialization in the interim.

use serde::{Deserialize, Serialize};

/// A text message exchanged between surfaces and agents on a stream.
///
/// This is a stub type matching the Python SDK's `TextMessage` — real Protobuf
/// codegen will replace this when `proto/wheelhouse/v1/core.proto` is generated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextMessage {
    /// The message text content.
    pub content: String,
    /// The sender name (e.g., "cli-surface", agent name).
    pub publisher: String,
    /// RFC 3339 full datetime with timezone (e.g., "2026-03-12T10:30:00Z").
    pub timestamp: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_message_serialize_roundtrip() {
        let msg = TextMessage {
            content: "hello".to_string(),
            publisher: "cli-surface".to_string(),
            timestamp: "2026-03-12T10:30:00Z".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: TextMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_text_message_json_fields_snake_case() {
        let msg = TextMessage {
            content: "test".to_string(),
            publisher: "agent-1".to_string(),
            timestamp: "2026-03-12T10:30:00Z".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"content\""));
        assert!(json.contains("\"publisher\""));
        assert!(json.contains("\"timestamp\""));
        // Ensure no camelCase
        assert!(!json.contains("Content"));
        assert!(!json.contains("Publisher"));
        assert!(!json.contains("Timestamp"));
    }
}
