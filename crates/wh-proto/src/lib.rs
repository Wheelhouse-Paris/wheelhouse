use serde::{Deserialize, Serialize};

/// A text message exchanged on a stream between surfaces and agents.
///
/// Stub type matching the Python SDK's approach — real Protobuf codegen comes later.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TextMessage {
    /// The message text content.
    pub content: String,
    /// Publisher name (e.g., "cli-surface", agent name).
    pub publisher: String,
    /// RFC 3339 full datetime with timezone (e.g., `2026-03-12T10:30:00Z`).
    pub timestamp: String,
    /// Optional user_id referencing a registered User profile (FR59).
    /// Messages from surfaces include this; messages from agents leave it as None.
    /// Note: user_id is attribution metadata only, NOT an authentication credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// A user profile representing a human participant in the system.
///
/// Users are registered entities: their profiles are git-versioned,
/// their messages are attributed by `user_id` in stream objects,
/// and their data is GDPR-purgeable (FR58).
///
/// Stub type — real Protobuf codegen comes later.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UserProfile {
    /// Deterministic user identifier (e.g., `usr_a1b2c3d4e5f6g7h8`).
    pub user_id: String,
    /// Platform where the user was first registered (e.g., "cli", "telegram").
    pub platform: String,
    /// Human-readable display name.
    pub display_name: String,
    /// RFC 3339 datetime when the profile was created (e.g., `2026-03-12T10:30:00Z`).
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_message_with_user_id_serializes() {
        let msg = TextMessage {
            content: "hello".to_string(),
            publisher: "cli-surface".to_string(),
            timestamp: "2026-03-12T10:30:00Z".to_string(),
            user_id: Some("usr_abc123".to_string()),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"user_id\":\"usr_abc123\""));

        let deserialized: TextMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.user_id, Some("usr_abc123".to_string()));
    }

    #[test]
    fn test_text_message_without_user_id_deserializes() {
        // JSON without user_id field — backward compatibility
        let json = r#"{"content":"hello","publisher":"agent-donna","timestamp":"2026-03-12T10:30:00Z"}"#;
        let msg: TextMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.user_id, None);
        assert_eq!(msg.content, "hello");
    }

    #[test]
    fn test_text_message_with_null_user_id_deserializes() {
        let json = r#"{"content":"hello","publisher":"agent","timestamp":"2026-03-12T10:30:00Z","user_id":null}"#;
        let msg: TextMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.user_id, None);
    }

    #[test]
    fn test_text_message_without_user_id_skips_in_serialization() {
        let msg = TextMessage {
            content: "hello".to_string(),
            publisher: "agent".to_string(),
            timestamp: "2026-03-12T10:30:00Z".to_string(),
            user_id: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("user_id"));
    }

    #[test]
    fn test_user_profile_yaml_roundtrip() {
        let profile = UserProfile {
            user_id: "usr_abc123def456".to_string(),
            platform: "cli".to_string(),
            display_name: "Alice".to_string(),
            created_at: "2026-03-12T10:30:00Z".to_string(),
        };

        let yaml = serde_yaml::to_string(&profile).unwrap();
        let deserialized: UserProfile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(profile, deserialized);
    }

    #[test]
    fn test_user_profile_json_roundtrip() {
        let profile = UserProfile {
            user_id: "usr_abc123def456".to_string(),
            platform: "telegram".to_string(),
            display_name: "Bob".to_string(),
            created_at: "2026-03-12T10:30:00Z".to_string(),
        };

        let json = serde_json::to_string(&profile).unwrap();
        let deserialized: UserProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, deserialized);
    }
}
