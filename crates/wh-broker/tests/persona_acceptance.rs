//! Acceptance tests for story 2-5: Agent Persona Load at Startup (FR61).
//!
//! These tests verify that persona files (SOUL.md, IDENTITY.md, MEMORY.md)
//! are loaded correctly at startup and that missing MEMORY.md is gracefully
//! handled.
//!
//! TDD RED PHASE: All tests should fail until persona module is implemented.

use wh_broker::deploy::persona::load_persona;
use wh_broker::deploy::podman::build_run_args;
use wh_broker::deploy::{parse_topology, Agent};

// ─── AC #1: Persona field in .wh topology ───────────────────────────────────

#[test]
fn wh_file_with_persona_field_parses_correctly() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    streams: [main]
    persona: agents/donna/
streams:
  - name: main
"#;
    let topo = parse_topology(yaml).unwrap();
    assert_eq!(topo.agents[0].persona, Some("agents/donna/".to_string()));
}

#[test]
fn wh_file_without_persona_field_defaults_to_none() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
    streams: [main]
streams:
  - name: main
"#;
    let topo = parse_topology(yaml).unwrap();
    assert_eq!(topo.agents[0].persona, None);
}

// ─── AC #1: Persona files loaded from disk ──────────────────────────────────

#[test]
fn load_persona_with_all_files_present_returns_content() {
    let dir = tempfile::tempdir().unwrap();
    let persona_dir = dir.path().join("agents/donna");
    std::fs::create_dir_all(&persona_dir).unwrap();
    std::fs::write(persona_dir.join("SOUL.md"), "I am Donna.").unwrap();
    std::fs::write(persona_dir.join("IDENTITY.md"), "Chief of staff.").unwrap();
    std::fs::write(
        persona_dir.join("MEMORY.md"),
        "Last action: scaled researcher.",
    )
    .unwrap();

    let persona = load_persona(dir.path(), "agents/donna/").unwrap();
    assert_eq!(persona.soul, Some("I am Donna.".to_string()));
    assert_eq!(persona.identity, Some("Chief of staff.".to_string()));
    assert_eq!(
        persona.memory,
        Some("Last action: scaled researcher.".to_string())
    );
}

// ─── AC #2: Missing MEMORY.md initialized as empty ─────────────────────────

#[test]
fn load_persona_with_missing_memory_initializes_empty() {
    let dir = tempfile::tempdir().unwrap();
    let persona_dir = dir.path().join("agents/donna");
    std::fs::create_dir_all(&persona_dir).unwrap();
    std::fs::write(persona_dir.join("SOUL.md"), "I am Donna.").unwrap();
    std::fs::write(persona_dir.join("IDENTITY.md"), "Chief of staff.").unwrap();
    // MEMORY.md intentionally NOT created

    let persona = load_persona(dir.path(), "agents/donna/").unwrap();
    // MEMORY.md should be initialized as empty, not cause an error
    assert_eq!(persona.memory, Some(String::new()));

    // The file should now exist on disk
    assert!(persona_dir.join("MEMORY.md").exists());
}

#[test]
fn load_persona_with_missing_soul_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let persona_dir = dir.path().join("agents/donna");
    std::fs::create_dir_all(&persona_dir).unwrap();
    // SOUL.md intentionally NOT created
    std::fs::write(persona_dir.join("IDENTITY.md"), "Chief of staff.").unwrap();
    std::fs::write(persona_dir.join("MEMORY.md"), "Some memory.").unwrap();

    let result = load_persona(dir.path(), "agents/donna/");
    assert!(result.is_err(), "Missing SOUL.md should cause an error");
}

#[test]
fn load_persona_with_missing_identity_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let persona_dir = dir.path().join("agents/donna");
    std::fs::create_dir_all(&persona_dir).unwrap();
    std::fs::write(persona_dir.join("SOUL.md"), "I am Donna.").unwrap();
    // IDENTITY.md intentionally NOT created
    std::fs::write(persona_dir.join("MEMORY.md"), "Some memory.").unwrap();

    let result = load_persona(dir.path(), "agents/donna/");
    assert!(result.is_err(), "Missing IDENTITY.md should cause an error");
}

// ─── AC #1: Container gets persona volume mount ─────────────────────────────

#[test]
fn build_run_args_includes_persona_volume_when_set() {
    let args = build_run_args(
        "dev",
        "donna",
        "agent-claude:latest",
        &["main".to_string()],
        None,
        Some("/workspace/agents/donna/"),
        &[],
    );
    // Should include -v for persona mount
    let has_volume = args
        .windows(2)
        .any(|w| w[0] == "-v" && w[1].contains("/persona"));
    assert!(
        has_volume,
        "Expected persona volume mount in args: {:?}",
        args
    );

    // Should include WH_PERSONA_PATH env var
    let has_env = args.iter().any(|a| a == "WH_PERSONA_PATH=/persona");
    assert!(
        has_env,
        "Expected WH_PERSONA_PATH env var in args: {:?}",
        args
    );
}

#[test]
fn build_run_args_excludes_persona_when_not_set() {
    let args = build_run_args(
        "dev",
        "researcher",
        "researcher:latest",
        &["main".to_string()],
        None,
        None,
        &[],
    );
    // Should NOT include persona-related args
    let has_persona = args
        .iter()
        .any(|a| a.contains("persona") || a.contains("PERSONA"));
    assert!(
        !has_persona,
        "Should not include persona args when not set: {:?}",
        args
    );
}

// ─── AC #2: Path traversal validation ───────────────────────────────────────

#[test]
fn load_persona_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let result = load_persona(dir.path(), "../evil/");
    assert!(result.is_err(), "Path traversal should be rejected");
}

// ─── AC #3: Agent identity visible in publisher field ───────────────────────

#[test]
fn agent_struct_persona_field_included_in_serialization() {
    let agent = Agent {
        name: "donna".to_string(),
        image: "agent-claude:latest".to_string(),
        replicas: 1,
        streams: vec!["main".to_string()],
        persona: Some("agents/donna/".to_string()),
    };
    let yaml = serde_yaml::to_string(&agent).unwrap();
    assert!(
        yaml.contains("persona"),
        "persona field should be serialized"
    );
    assert!(
        yaml.contains("agents/donna/"),
        "persona value should be in serialized output"
    );
}
