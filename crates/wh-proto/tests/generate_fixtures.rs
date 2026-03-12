//! Fixture generator for proto backward-compatibility tests.
//!
//! Run with: cargo test -p wh-proto --test generate_fixtures -- --ignored
//!
//! This generates binary proto fixtures at tests/fixtures/proto/.
//! These fixtures are committed to the repo and used by CI to verify
//! backward compatibility (NFR-E1).

use prost::Message;

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/proto");

#[test]
#[ignore] // Run explicitly when fixtures need regeneration
fn generate_v1_fixtures() {
    std::fs::create_dir_all(FIXTURE_DIR).expect("Failed to create fixture directory");

    // TextMessage fixture
    let text_msg = wh_proto::TextMessage {
        content: "Hello from Wheelhouse v0.1.0".to_string(),
        publisher_id: "fixture-generator".to_string(),
        timestamp_ms: 1710000000000, // Fixed timestamp for reproducibility
    };
    let path = format!("{FIXTURE_DIR}/v1_text_message.bin");
    std::fs::write(&path, text_msg.encode_to_vec()).expect("Failed to write TextMessage fixture");
    println!("Wrote {path}");

    // SkillInvocation fixture
    let skill_msg = wh_proto::SkillInvocation {
        skill_name: "echo".to_string(),
        agent_id: "fixture-agent".to_string(),
        invocation_id: "inv-001".to_string(),
        parameters: [("input".to_string(), "test".to_string())]
            .into_iter()
            .collect(),
        timestamp_ms: 1710000000000,
    };
    let path = format!("{FIXTURE_DIR}/v1_skill_invocation.bin");
    std::fs::write(&path, skill_msg.encode_to_vec())
        .expect("Failed to write SkillInvocation fixture");
    println!("Wrote {path}");

    // CronEvent fixture
    let cron_msg = wh_proto::CronEvent {
        job_name: "daily-compaction".to_string(),
        stream_name: "system-events".to_string(),
        cron_expression: "0 0 * * *".to_string(),
        fired_at_ms: 1710000000000,
    };
    let path = format!("{FIXTURE_DIR}/v1_cron_event.bin");
    std::fs::write(&path, cron_msg.encode_to_vec()).expect("Failed to write CronEvent fixture");
    println!("Wrote {path}");

    println!("All v1 fixtures generated successfully.");
}
