//! Proto backward-compatibility tests (NFR-E1).
//!
//! These tests verify that messages serialized with previous schema versions
//! can still be deserialized by the current version without data loss.

use prost::Message;

/// Path to the fixture directory relative to the workspace root.
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/proto");

#[test]
fn v1_text_message_fixture_exists() {
    let path = format!("{}/v1_text_message.bin", FIXTURE_DIR);
    assert!(
        std::path::Path::new(&path).exists(),
        "Proto fixture file must exist at {path} for backward compatibility (NFR-E1). \
         Run the fixture generator to create it."
    );
}

#[test]
fn v1_text_message_roundtrip() {
    let path = format!("{}/v1_text_message.bin", FIXTURE_DIR);
    let data = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("Failed to read proto fixture at {path}: {e}. Run fixture generator first.")
    });

    let msg = wh_proto::TextMessage::decode(data.as_slice())
        .expect("Deserialization of v1 TextMessage fixture must succeed (NFR-E1)");

    assert_eq!(msg.content, "Hello from Wheelhouse v0.1.0");
    assert_eq!(msg.publisher_id, "fixture-generator");
    assert!(msg.timestamp_ms > 0, "timestamp_ms must be set");

    // Re-encode and verify round-trip
    let re_encoded = msg.encode_to_vec();
    let re_decoded = wh_proto::TextMessage::decode(re_encoded.as_slice())
        .expect("Re-deserialization must succeed");
    assert_eq!(msg.content, re_decoded.content);
    assert_eq!(msg.publisher_id, re_decoded.publisher_id);
    assert_eq!(msg.timestamp_ms, re_decoded.timestamp_ms);
}

#[test]
fn v1_skill_invocation_fixture_roundtrip() {
    let path = format!("{}/v1_skill_invocation.bin", FIXTURE_DIR);
    let data = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("Failed to read proto fixture at {path}: {e}. Run fixture generator first.")
    });

    let msg = wh_proto::SkillInvocation::decode(data.as_slice())
        .expect("Deserialization of v1 SkillInvocation fixture must succeed (NFR-E1)");

    assert_eq!(msg.skill_name, "echo");
    assert_eq!(msg.agent_id, "fixture-agent");
    assert!(!msg.invocation_id.is_empty());
}

#[test]
fn v1_cron_event_fixture_roundtrip() {
    let path = format!("{}/v1_cron_event.bin", FIXTURE_DIR);
    let data = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("Failed to read proto fixture at {path}: {e}. Run fixture generator first.")
    });

    let msg = wh_proto::CronEvent::decode(data.as_slice())
        .expect("Deserialization of v1 CronEvent fixture must succeed (NFR-E1)");

    assert_eq!(msg.job_name, "daily-compaction");
}
