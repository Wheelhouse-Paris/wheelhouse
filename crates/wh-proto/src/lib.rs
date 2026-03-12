//! Wheelhouse Protobuf type definitions.
//!
//! This crate is the single `include!` point for all generated Protobuf types (KR-01).
//! Downstream crates use `wh_proto::TextMessage`, never `OUT_DIR` directly.

/// All Wheelhouse v1 Protobuf types.
///
/// Types are organized across four domains:
/// - **core**: TextMessage, FileMessage, Reaction
/// - **skills**: SkillInvocation, SkillResult, SkillProgress
/// - **system**: CronEvent, TopologyShutdown, StreamCapacityWarning
/// - **stream**: StreamEnvelope, TypeRegistration, TypeRegistryEntry
pub mod wheelhouse {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/wheelhouse.v1.rs"));
    }
}

// Re-export prost_types for consumers using Timestamp in CronEvent and other proto messages.
pub use prost_types;

// Re-export all types at crate root for ergonomic imports:
// `use wh_proto::TextMessage;` instead of `use wh_proto::wheelhouse::v1::TextMessage;`
pub use wheelhouse::v1::*;

/// Result of attempting to receive and deserialize a typed message.
///
/// Per AC #2: if the receiver knows the type, it gets deserialized data.
/// If the receiver does not know the type, it gets raw bytes + type name.
/// Never a silent failure or crash.
#[derive(Debug, Clone)]
pub enum TypedMessage {
    /// Type was known and deserialized successfully.
    Known {
        type_name: String,
        /// Deserialized data — generic bytes that the application layer interprets.
        data: Vec<u8>,
    },
    /// Type was unknown — raw bytes returned with type name for inspection.
    Unknown {
        type_name: String,
        raw_bytes: Vec<u8>,
    },
}

impl TypedMessage {
    /// Get the type name regardless of known/unknown status.
    pub fn type_name(&self) -> &str {
        match self {
            TypedMessage::Known { type_name, .. } => type_name,
            TypedMessage::Unknown { type_name, .. } => type_name,
        }
    }

    /// Check if the type was known.
    pub fn is_known(&self) -> bool {
        matches!(self, TypedMessage::Known { .. })
    }
}

use serde::{Deserialize, Serialize};

/// A user profile representing a human participant in the system.
///
/// Users are registered entities: their profiles are git-versioned,
/// their messages are attributed by `user_id` in stream objects,
/// and their data is GDPR-purgeable (FR58).
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
    use prost::Message;

    // ── Core types ──────────────────────────────────────────

    #[test]
    fn text_message_default() {
        let msg = TextMessage::default();
        assert!(msg.content.is_empty());
        assert!(msg.publisher_id.is_empty());
        assert_eq!(msg.timestamp_ms, 0);
    }

    #[test]
    fn text_message_roundtrip() {
        let original = TextMessage {
            content: "hello world".to_string(),
            publisher_id: "test-pub".to_string(),
            timestamp_ms: 1710000000000,
            user_id: String::new(),
            reply_to_user_id: String::new(),
        };
        let encoded = original.encode_to_vec();
        let decoded = TextMessage::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.content, decoded.content);
        assert_eq!(original.publisher_id, decoded.publisher_id);
        assert_eq!(original.timestamp_ms, decoded.timestamp_ms);
    }

    #[test]
    fn file_message_roundtrip() {
        let original = FileMessage {
            filename: "test.txt".to_string(),
            mime_type: "text/plain".to_string(),
            data: b"file content".to_vec(),
            publisher_id: "pub-1".to_string(),
            timestamp_ms: 1710000000000,
        };
        let encoded = original.encode_to_vec();
        let decoded = FileMessage::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.filename, decoded.filename);
        assert_eq!(original.data, decoded.data);
    }

    #[test]
    fn reaction_roundtrip() {
        let original = Reaction {
            target_object_id: "obj-123".to_string(),
            emoji: "thumbs_up".to_string(),
            publisher_id: "user-1".to_string(),
            timestamp_ms: 1710000000000,
        };
        let encoded = original.encode_to_vec();
        let decoded = Reaction::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.emoji, decoded.emoji);
    }

    // ── Skills types ────────────────────────────────────────

    #[test]
    fn skill_invocation_roundtrip() {
        let mut params = std::collections::HashMap::new();
        params.insert("key".to_string(), "value".to_string());

        let original = SkillInvocation {
            skill_name: "my-skill".to_string(),
            agent_id: "agent-1".to_string(),
            invocation_id: "inv-001".to_string(),
            parameters: params,
            timestamp_ms: 1710000000000,
        };
        let encoded = original.encode_to_vec();
        let decoded = SkillInvocation::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.skill_name, decoded.skill_name);
        assert_eq!(
            original.parameters.get("key"),
            decoded.parameters.get("key")
        );
    }

    #[test]
    fn skill_result_roundtrip() {
        let original = SkillResult {
            invocation_id: "inv-001".to_string(),
            skill_name: "my-skill".to_string(),
            success: true,
            output: "done".to_string(),
            error_message: String::new(),
            error_code: String::new(),
            timestamp_ms: 1710000000000,
        };
        let encoded = original.encode_to_vec();
        let decoded = SkillResult::decode(encoded.as_slice()).unwrap();
        assert!(decoded.success);
        assert_eq!(original.output, decoded.output);
    }

    #[test]
    fn skill_progress_roundtrip() {
        let original = SkillProgress {
            invocation_id: "inv-001".to_string(),
            skill_name: "my-skill".to_string(),
            progress_percent: 0.5,
            status_message: "halfway".to_string(),
            timestamp_ms: 1710000000000,
        };
        let encoded = original.encode_to_vec();
        let decoded = SkillProgress::decode(encoded.as_slice()).unwrap();
        assert!((decoded.progress_percent - 0.5).abs() < f32::EPSILON);
    }

    // ── System types ────────────────────────────────────────

    #[test]
    fn cron_event_roundtrip() {
        let original = CronEvent {
            job_name: "daily-job".to_string(),
            action: "event".to_string(),
            schedule: "0 0 * * *".to_string(),
            triggered_at: Some(prost_types::Timestamp {
                seconds: 1710000000,
                nanos: 0,
            }),
            payload: std::collections::HashMap::new(),
        };
        let encoded = original.encode_to_vec();
        let decoded = CronEvent::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.job_name, decoded.job_name);
        assert_eq!(original.schedule, decoded.schedule);
        assert_eq!(
            original.triggered_at.unwrap().seconds,
            decoded.triggered_at.unwrap().seconds
        );
    }

    #[test]
    fn topology_shutdown_roundtrip() {
        let original = TopologyShutdown {
            reason: "operator requested".to_string(),
            timestamp_ms: 1710000000000,
        };
        let encoded = original.encode_to_vec();
        let decoded = TopologyShutdown::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.reason, decoded.reason);
    }

    #[test]
    fn stream_capacity_warning_roundtrip() {
        let original = StreamCapacityWarning {
            stream_name: "events".to_string(),
            current_size_bytes: 900_000_000,
            max_size_bytes: 1_000_000_000,
            usage_percent: 90.0,
            timestamp_ms: 1710000000000,
        };
        let encoded = original.encode_to_vec();
        let decoded = StreamCapacityWarning::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.stream_name, decoded.stream_name);
        assert!((decoded.usage_percent - 90.0).abs() < f32::EPSILON);
    }

    // ── Stream types ────────────────────────────────────────

    #[test]
    fn stream_envelope_roundtrip() {
        let original = StreamEnvelope {
            stream_name: "my-stream".to_string(),
            object_id: "obj-1".to_string(),
            type_url: "wheelhouse.v1.TextMessage".to_string(),
            payload: b"encoded-payload".to_vec(),
            publisher_id: "pub-1".to_string(),
            published_at_ms: 1710000000000,
            sequence_number: 42,
        };
        let encoded = original.encode_to_vec();
        let decoded = StreamEnvelope::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.stream_name, decoded.stream_name);
        assert_eq!(original.sequence_number, decoded.sequence_number);
    }

    #[test]
    fn type_registration_roundtrip() {
        let original = TypeRegistration {
            namespace: "myapp".to_string(),
            type_name: "OrderEvent".to_string(),
            descriptor: "proto descriptor bytes".to_string(),
            registered_at_ms: 1710000000000,
        };
        let encoded = original.encode_to_vec();
        let decoded = TypeRegistration::decode(encoded.as_slice()).unwrap();
        assert_eq!(original.namespace, decoded.namespace);
        assert_eq!(original.type_name, decoded.type_name);
    }

    #[test]
    fn type_registry_entry_roundtrip() {
        let original = TypeRegistryEntry {
            full_name: "wheelhouse.v1.TextMessage".to_string(),
            namespace: "wheelhouse".to_string(),
            type_name: "TextMessage".to_string(),
            descriptor: String::new(),
            is_builtin: true,
        };
        let encoded = original.encode_to_vec();
        let decoded = TypeRegistryEntry::decode(encoded.as_slice()).unwrap();
        assert!(decoded.is_builtin);
        assert_eq!(original.full_name, decoded.full_name);
    }

    // ── Empty message deserialization ───────────────────────

    #[test]
    fn empty_bytes_deserialize_to_defaults() {
        let msg = TextMessage::decode(&[] as &[u8]).unwrap();
        assert!(msg.content.is_empty());
        assert_eq!(msg.timestamp_ms, 0);
    }

    // ── TypedMessage ────────────────────────────────────────

    #[test]
    fn typed_message_unknown_type_returns_raw_bytes() {
        let msg = TypedMessage::Unknown {
            type_name: "biotech.MoleculeObject".to_string(),
            raw_bytes: vec![1, 2, 3],
        };
        assert!(!msg.is_known());
        assert_eq!(msg.type_name(), "biotech.MoleculeObject");
        match msg {
            TypedMessage::Unknown {
                type_name,
                raw_bytes,
            } => {
                assert_eq!(type_name, "biotech.MoleculeObject");
                assert_eq!(raw_bytes, vec![1, 2, 3]);
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    #[test]
    fn typed_message_known_type() {
        let msg = TypedMessage::Known {
            type_name: "biotech.MoleculeObject".to_string(),
            data: vec![1, 2, 3],
        };
        assert!(msg.is_known());
        assert_eq!(msg.type_name(), "biotech.MoleculeObject");
    }

    // ── UserProfile ─────────────────────────────────────────

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

