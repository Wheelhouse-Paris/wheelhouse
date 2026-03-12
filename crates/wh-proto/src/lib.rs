//! Wheelhouse protocol types.
//!
//! Stub types for MVP — real Protobuf codegen comes later.
//! These are Rust-native structs with serde support.

use serde::{Deserialize, Serialize};

/// A text message exchanged on a stream.
///
/// Used by surfaces (CLI, Telegram) and agents to communicate.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TextMessage {
    /// The message content.
    pub content: String,

    /// Publisher identity (surface name or agent name).
    pub publisher: String,

    /// RFC 3339 timestamp with timezone, e.g. "2026-03-12T10:30:00Z".
    pub timestamp: String,

    /// User ID referencing a registered UserProfile (FR59).
    /// Present when message is from a human user via a surface.
    /// None when message is from an agent.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<String>,

    /// Target user ID for response routing.
    /// Set by agents when responding to a specific user.
    /// Surfaces use this to route responses to the correct chat/session.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reply_to_user_id: Option<String>,
}

/// A registered user profile (FR58).
///
/// Stored as YAML in `.wh/users/{user_id}.yaml` and committed to git.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UserProfile {
    /// Deterministic user ID: `usr_` + first 16 hex chars of SHA-256("{platform}:{platform_user_id}").
    pub user_id: String,

    /// Platform identifier, e.g. "cli", "telegram".
    pub platform: String,

    /// Human-readable display name.
    pub display_name: String,

    /// RFC 3339 timestamp of profile creation.
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_message_serializes_with_all_fields() {
        let msg = TextMessage {
            content: "Hello".to_string(),
            publisher: "telegram-surface".to_string(),
            timestamp: "2026-03-12T10:00:00Z".to_string(),
            user_id: Some("usr_abc123".to_string()),
            reply_to_user_id: Some("usr_def456".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("reply_to_user_id"));
        assert!(json.contains("user_id"));
    }

    #[test]
    fn text_message_omits_none_fields() {
        let msg = TextMessage {
            content: "Hello".to_string(),
            publisher: "agent".to_string(),
            timestamp: "2026-03-12T10:00:00Z".to_string(),
            user_id: None,
            reply_to_user_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("user_id"));
        assert!(!json.contains("reply_to_user_id"));
    }

    #[test]
    fn text_message_deserializes_without_optional_fields() {
        let json = r#"{"content":"Hello","publisher":"agent","timestamp":"2026-03-12T10:00:00Z"}"#;
        let msg: TextMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.user_id, None);
        assert_eq!(msg.reply_to_user_id, None);
    }

    #[test]
    fn text_message_roundtrip() {
        let msg = TextMessage {
            content: "test".to_string(),
            publisher: "pub".to_string(),
            timestamp: "2026-03-12T10:00:00Z".to_string(),
            user_id: Some("usr_abc".to_string()),
            reply_to_user_id: Some("usr_def".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: TextMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn user_profile_yaml_roundtrip() {
        let profile = UserProfile {
            user_id: "usr_abc123".to_string(),
            platform: "telegram".to_string(),
            display_name: "Alice".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
        };
        let yaml = serde_yaml::to_string(&profile).unwrap();
        let deserialized: UserProfile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(profile, deserialized);
    }

    #[test]
    fn user_profile_json_roundtrip() {
        let profile = UserProfile {
            user_id: "usr_abc123".to_string(),
            platform: "telegram".to_string(),
            display_name: "Alice".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&profile).unwrap();
        let deserialized: UserProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, deserialized);
    }
}
