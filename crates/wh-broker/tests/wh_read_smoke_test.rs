//! Acceptance tests for Story 2.8: Agent Reads Its Own `.wh` — Autonomous Loop Smoke Test.
//!
//! These tests verify:
//! - AC #1: Agent reads its `.wh` file and produces a topology summary
//! - AC #2: Agent attribution in the publish event (log publisher field)
//!
//! TDD Red Phase: These tests are written BEFORE implementation.
//! They MUST fail initially and pass after implementation.

/// Helper: create a temp directory with a .wh file.
fn setup_wh_workspace(wh_content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(&wh_path, wh_content).unwrap();
    (dir, wh_path)
}

const VALID_WH: &str = r#"api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: donna
    image: agent-claude:latest
    streams: [main]
    persona: agents/donna/
  - name: researcher
    image: researcher:latest
    streams: [main]
streams:
  - name: main
    retention: 7d
"#;

// =============================================================================
// AC #1: Agent reads .wh file and returns topology summary
// =============================================================================

#[test]
fn agent_reads_own_wh_and_returns_summary() {
    use wh_broker::deploy::autonomous::smoke_test_read_loop;

    let (_dir, wh_path) = setup_wh_workspace(VALID_WH);

    let event = smoke_test_read_loop(&wh_path, "donna").unwrap();

    // The event must contain the correct topology metadata
    assert_eq!(event.agent_name, "donna");
    assert!(event.summary.contains("dev"), "summary should contain topology name");
    assert!(event.summary.contains("2 agents"), "summary should contain agent count");
    assert!(event.summary.contains("1 stream"), "summary should contain stream count");
    assert!(!event.content.is_empty(), "content should contain raw YAML");
}

#[test]
fn agent_reads_own_wh_missing_file_errors() {
    use wh_broker::deploy::autonomous::read_own_topology;
    use wh_broker::deploy::DeployError;

    let result = read_own_topology(std::path::Path::new("/nonexistent/topology.wh"));
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), DeployError::FileRead(_)));
}

#[test]
fn agent_reads_own_wh_invalid_yaml_errors() {
    use wh_broker::deploy::autonomous::read_own_topology;
    use wh_broker::deploy::DeployError;

    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("bad.wh");
    std::fs::write(&wh_path, "not: [valid: yaml: {{{").unwrap();

    let result = read_own_topology(&wh_path);
    assert!(result.is_err());
    // Invalid YAML should result in a parse error
    match result.unwrap_err() {
        DeployError::YamlParse(_) => {}
        other => panic!("Expected YamlParse, got: {:?}", other),
    }
}

#[test]
fn topology_summary_format_is_human_readable() {
    use wh_broker::deploy::autonomous::read_own_topology;

    let (_dir, wh_path) = setup_wh_workspace(VALID_WH);

    let summary = read_own_topology(&wh_path).unwrap();
    // Format: "Topology 'dev': 2 agents, 1 stream"
    assert_eq!(summary.summary, "Topology 'dev': 2 agents, 1 stream");
    assert_eq!(summary.topology_name, "dev");
    assert_eq!(summary.agent_count, 2);
    assert_eq!(summary.stream_count, 1);
}

// =============================================================================
// AC #2: Agent identity attributed in publish event
// =============================================================================

#[test]
fn publish_event_attributes_agent_identity() {
    use wh_broker::deploy::autonomous::{read_own_topology, publish_topology_summary};

    let (_dir, wh_path) = setup_wh_workspace(VALID_WH);
    let summary = read_own_topology(&wh_path).unwrap();

    let event = publish_topology_summary(&summary, "donna");
    assert_eq!(event.agent_name, "donna");
    assert!(!event.timestamp.is_empty(), "timestamp must be set");
    assert!(event.content.contains("wheelhouse.dev/v1"), "content should contain raw YAML");
}

// =============================================================================
// Edge cases
// =============================================================================

#[test]
fn topology_summary_singular_agent_singular_stream() {
    use wh_broker::deploy::autonomous::read_own_topology;

    let wh_content = r#"api_version: wheelhouse.dev/v1
name: prod
agents:
  - name: donna
    image: agent-claude:latest
streams:
  - name: main
"#;
    let (_dir, wh_path) = setup_wh_workspace(wh_content);
    let summary = read_own_topology(&wh_path).unwrap();
    assert_eq!(summary.summary, "Topology 'prod': 1 agent, 1 stream");
}

#[test]
fn topology_summary_no_streams() {
    use wh_broker::deploy::autonomous::read_own_topology;

    let wh_content = r#"api_version: wheelhouse.dev/v1
name: minimal
agents:
  - name: donna
    image: agent-claude:latest
"#;
    let (_dir, wh_path) = setup_wh_workspace(wh_content);
    let summary = read_own_topology(&wh_path).unwrap();
    assert_eq!(summary.summary, "Topology 'minimal': 1 agent, 0 streams");
}
