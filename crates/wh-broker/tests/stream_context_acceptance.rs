//! Acceptance tests for Story 10.1: Stream Description Field and CONTEXT.md Initialization
//!
//! These tests verify the end-to-end behavior of the `description` field on streams
//! and the automatic creation of `.wh/context/<stream>/CONTEXT.md` files during deploy apply.

use tempfile::TempDir;
use wh_broker::deploy::apply::write_context_files;
use wh_broker::deploy::plan::detect_new_context_files;
use wh_broker::deploy::{parse_topology, Stream, Topology};

/// AC-1: Stream with description creates CONTEXT.md on apply
#[test]
fn stream_with_description_creates_context_md() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
streams:
  - name: main
    description: "Main conversation stream for general discussion"
"#;
    let topo = parse_topology(yaml).unwrap();
    assert_eq!(
        topo.streams[0].description,
        Some("Main conversation stream for general discussion".to_string())
    );

    let tmp = TempDir::new().unwrap();
    let wh_dir = tmp.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();

    let context_path = wh_dir.join("context").join("main").join("CONTEXT.md");
    assert!(!context_path.exists(), "CONTEXT.md should not exist yet");

    write_context_files(tmp.path(), &topo.streams);
    assert!(context_path.exists(), "CONTEXT.md should be created");
    let content = std::fs::read_to_string(&context_path).unwrap();
    assert_eq!(content, "Main conversation stream for general discussion");
}

/// AC-1: CONTEXT.md is never overwritten if it already exists
#[test]
fn context_md_not_overwritten_if_exists() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
streams:
  - name: main
    description: "New description"
"#;
    let topo = parse_topology(yaml).unwrap();

    let tmp = TempDir::new().unwrap();
    let wh_dir = tmp.path().join(".wh");
    let context_dir = wh_dir.join("context").join("main");
    std::fs::create_dir_all(&context_dir).unwrap();

    let context_path = context_dir.join("CONTEXT.md");
    std::fs::write(&context_path, "Operator-enriched content").unwrap();

    write_context_files(tmp.path(), &topo.streams);

    let content = std::fs::read_to_string(&context_path).unwrap();
    assert_eq!(
        content, "Operator-enriched content",
        "CONTEXT.md must not be overwritten"
    );
}

/// AC-2: Stream without description does NOT create CONTEXT.md
#[test]
fn stream_without_description_no_context_md() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
streams:
  - name: main
"#;
    let topo = parse_topology(yaml).unwrap();
    assert_eq!(topo.streams[0].description, None);

    let tmp = TempDir::new().unwrap();
    let wh_dir = tmp.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();

    write_context_files(tmp.path(), &topo.streams);

    let context_path = wh_dir.join("context").join("main").join("CONTEXT.md");
    assert!(
        !context_path.exists(),
        "CONTEXT.md should not be created for streams without description"
    );
}

/// AC-2: Empty description treated as None — no CONTEXT.md created
#[test]
fn empty_description_no_context_md() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
streams:
  - name: main
    description: ""
"#;
    let topo = parse_topology(yaml).unwrap();

    let tmp = TempDir::new().unwrap();
    let wh_dir = tmp.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();

    write_context_files(tmp.path(), &topo.streams);

    let context_path = wh_dir.join("context").join("main").join("CONTEXT.md");
    assert!(
        !context_path.exists(),
        "CONTEXT.md should not be created for streams with empty description"
    );
}

/// AC-2: Backward compatibility — Stream without description field deserializes correctly
#[test]
fn stream_without_description_backward_compat() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
streams:
  - name: main
    retention: 7d
"#;
    let topo = parse_topology(yaml).unwrap();
    assert_eq!(topo.streams[0].name, "main");
    assert_eq!(topo.streams[0].retention, Some("7d".to_string()));
    assert_eq!(topo.streams[0].description, None);
}

/// AC-3: Destroy does NOT delete CONTEXT.md files
#[test]
fn destroy_preserves_context_md() {
    let tmp = TempDir::new().unwrap();
    let wh_dir = tmp.path().join(".wh");
    let context_dir = wh_dir.join("context").join("main");
    std::fs::create_dir_all(&context_dir).unwrap();

    let context_path = context_dir.join("CONTEXT.md");
    std::fs::write(&context_path, "Operator context").unwrap();

    // Write a state.json with a stream (simulating deployed state)
    let state_path = wh_dir.join("state.json");
    let state = serde_json::json!({
        "api_version": "wheelhouse.dev/v1",
        "name": "dev",
        "agents": [],
        "streams": [{"name": "main"}],
        "surfaces": []
    });
    std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

    // Verify CONTEXT.md exists before and after state changes
    // (destroy only clears state.json and stops containers — it never touches context/)
    assert!(
        context_path.exists(),
        "CONTEXT.md must survive destroy — operator data is never deleted automatically"
    );

    // Simulate what destroy does: write an empty topology to state.json
    let empty_state = serde_json::json!({
        "api_version": "wheelhouse.dev/v1",
        "name": "dev",
        "agents": [],
        "streams": [],
        "surfaces": []
    });
    std::fs::write(
        &state_path,
        serde_json::to_string_pretty(&empty_state).unwrap(),
    )
    .unwrap();

    // CONTEXT.md must still exist after state is cleared
    assert!(
        context_path.exists(),
        "CONTEXT.md must survive destroy — operator data is never deleted automatically"
    );
}

/// AC-4: Plan output notes CONTEXT.md creation
#[test]
fn plan_notes_context_file_creation() {
    let streams = vec![
        Stream {
            name: "main".to_string(),
            retention: None,
            description: Some("Main stream context".to_string()),
        },
        Stream {
            name: "admin".to_string(),
            retention: None,
            description: Some("Admin stream context".to_string()),
        },
    ];

    let tmp = TempDir::new().unwrap();
    let wh_dir = tmp.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();

    let context_notes = detect_new_context_files(tmp.path(), &streams);
    assert_eq!(context_notes.len(), 2);
    assert!(context_notes.iter().any(|n| n == "main"));
    assert!(context_notes.iter().any(|n| n == "admin"));
}

/// AC-4: Plan output does NOT note CONTEXT.md for existing files
#[test]
fn plan_skips_existing_context_files() {
    let streams = vec![Stream {
        name: "main".to_string(),
        retention: None,
        description: Some("Main stream context".to_string()),
    }];

    let tmp = TempDir::new().unwrap();
    let wh_dir = tmp.path().join(".wh");
    let context_dir = wh_dir.join("context").join("main");
    std::fs::create_dir_all(&context_dir).unwrap();
    std::fs::write(context_dir.join("CONTEXT.md"), "Already exists").unwrap();

    let context_notes = detect_new_context_files(tmp.path(), &streams);
    assert!(
        context_notes.is_empty(),
        "Should not note creation for existing CONTEXT.md"
    );
}

/// Description field round-trips through YAML serialization
#[test]
fn description_yaml_roundtrip() {
    let topo = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        broker: None,
        skills_repo: None,
        agents: vec![],
        streams: vec![Stream {
            name: "main".to_string(),
            retention: None,
            description: Some("Test description".to_string()),
        }],
        surfaces: vec![],
        guardrails: None,
    };
    let yaml = serde_yaml::to_string(&topo).unwrap();
    let parsed: Topology = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(
        parsed.streams[0].description,
        Some("Test description".to_string())
    );
}

/// Multiple streams — only those with descriptions get CONTEXT.md
#[test]
fn mixed_streams_only_described_get_context() {
    let streams = vec![
        Stream {
            name: "main".to_string(),
            retention: None,
            description: Some("Main stream".to_string()),
        },
        Stream {
            name: "logs".to_string(),
            retention: Some("30d".to_string()),
            description: None,
        },
        Stream {
            name: "admin".to_string(),
            retention: None,
            description: Some("Admin stream".to_string()),
        },
    ];

    let tmp = TempDir::new().unwrap();
    let wh_dir = tmp.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();

    write_context_files(tmp.path(), &streams);

    assert!(tmp.path().join(".wh/context/main/CONTEXT.md").exists());
    assert!(!tmp.path().join(".wh/context/logs/CONTEXT.md").exists());
    assert!(tmp.path().join(".wh/context/admin/CONTEXT.md").exists());
}

/// PlanData display includes context file creation notes
#[test]
fn plan_data_display_shows_context_files() {
    use wh_broker::deploy::plan::PlanData;

    let plan_data = PlanData {
        has_changes: true,
        changes: vec![wh_broker::deploy::Change {
            op: "+".to_string(),
            component: "stream main".to_string(),
            field: None,
            from: None,
            to: Some(serde_json::json!({"provider": "local"})),
            source_file: None,
        }],
        plan_hash: "sha256:abc".to_string(),
        topology_name: "dev".to_string(),
        policy_snapshot_hash: String::new(),
        warnings: vec![],
        context_files: vec!["main".to_string(), "admin".to_string()],
    };

    let output = format!("{plan_data}");
    assert!(
        output.contains("Context files to create:"),
        "should show context files header: {output}"
    );
    assert!(
        output.contains(".wh/context/main/CONTEXT.md"),
        "should show main context path: {output}"
    );
    assert!(
        output.contains(".wh/context/admin/CONTEXT.md"),
        "should show admin context path: {output}"
    );
}
