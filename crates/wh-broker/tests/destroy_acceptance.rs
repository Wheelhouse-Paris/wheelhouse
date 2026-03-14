//! Acceptance tests for Story 2.7: `wh deploy destroy` and Guardrails
//!
//! AC #1: Deploy destroy stops all containers and records a git commit
//! AC #2: Guardrails max_replicas blocks apply when exceeded (FR4)
//! AC #3: Autonomous plan respects max_replicas (FR4, CM-05)

use std::process::Command;

fn git_cmd() -> Command {
    for path in &[
        "/usr/bin/git",
        "/usr/local/bin/git",
        "/opt/homebrew/bin/git",
    ] {
        if std::path::Path::new(path).exists() {
            return Command::new(*path);
        }
    }
    Command::new("git")
}

/// Helper: initialize a temp git repo with a deployed topology in .wh/state.json.
fn setup_deployed_repo() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let temp_path = temp_dir.path();

    git_cmd()
        .args(["init"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["config", "user.email", "test@test.com"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["config", "user.name", "Test"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    std::fs::write(temp_path.join(".gitkeep"), "").unwrap();
    git_cmd()
        .args(["add", "."])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["commit", "-m", "initial commit"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    // Write topology file
    std::fs::write(
        temp_path.join("topology.wh"),
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: researcher:latest\n    replicas: 1\n    streams:\n      - main\nstreams:\n  - name: main\n    retention: 7d\n",
    ).unwrap();

    // Simulate a deployed state by writing state.json
    let wh_dir = temp_path.join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    let topology = wh_broker::deploy::Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![wh_broker::deploy::Agent {
            name: "researcher".to_string(),
            image: "researcher:latest".to_string(),
            replicas: 1,
            streams: vec!["main".to_string()],
            persona: None,
        }],
        streams: vec![wh_broker::deploy::Stream {
            name: "main".to_string(),
            retention: Some("7d".to_string()),
        }],
        surfaces: vec![],
        guardrails: None,
    };
    std::fs::write(
        wh_dir.join("state.json"),
        serde_json::to_string_pretty(&topology).unwrap(),
    )
    .unwrap();

    // Commit the deployed state
    git_cmd()
        .args(["add", "."])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["commit", "-m", "[operator] apply: add agent researcher"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    temp_dir
}

// ── AC #1: Deploy destroy clears state and records git commit ──

/// AC #1: destroy() clears state.json and creates a git commit.
#[test]
fn destroy_clears_state_and_commits() {
    let temp_dir = setup_deployed_repo();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    // Call destroy — container stop will fail (no Podman in CI) but destroy
    // should still succeed (partial failure pattern: log and continue)
    let result = wh_broker::deploy::apply::destroy(&wh_path, None);
    assert!(result.is_ok(), "destroy should succeed: {:?}", result.err());

    let destroy_result = result.unwrap();
    // Agent count should be 1 (intent recorded even if container stop fails)
    assert_eq!(
        destroy_result.destroyed, 1,
        "should record 1 agent destroyed"
    );
    assert_eq!(
        destroy_result.streams_removed, 1,
        "should record 1 stream removed"
    );

    // Verify state.json has empty agents/streams
    let state_path = temp_path.join(".wh").join("state.json");
    assert!(state_path.exists(), "state.json should still exist");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let topo: wh_broker::deploy::Topology = serde_json::from_str(&content).unwrap();
    assert!(
        topo.agents.is_empty(),
        "state.json should have no agents after destroy"
    );
    assert!(
        topo.streams.is_empty(),
        "state.json should have no streams after destroy"
    );

    // Verify a destroy commit was created
    let log_output = git_cmd()
        .args(["log", "--oneline", "-1"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    let log_msg = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log_msg.contains("destroy"),
        "commit message should contain 'destroy': {log_msg}"
    );
}

/// AC #1: destroy on empty state (no state.json) is a no-op.
#[test]
fn destroy_on_empty_state_is_noop() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    git_cmd()
        .args(["init"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["config", "user.email", "test@test.com"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["config", "user.name", "Test"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    std::fs::write(temp_path.join(".gitkeep"), "").unwrap();
    git_cmd()
        .args(["add", "."])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["commit", "-m", "initial"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    std::fs::write(
        temp_path.join("topology.wh"),
        "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\nstreams: []\n",
    )
    .unwrap();

    let wh_path = temp_path.join("topology.wh");
    let result = wh_broker::deploy::apply::destroy(&wh_path, None);
    assert!(result.is_ok(), "destroy on empty state should succeed");

    let destroy_result = result.unwrap();
    assert_eq!(destroy_result.destroyed, 0, "nothing to destroy");
    assert_eq!(destroy_result.streams_removed, 0, "no streams to remove");
}

/// AC #1: destroy with empty state.json (agents:[], streams:[]) is a no-op.
#[test]
fn destroy_on_empty_deployed_state_is_noop() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    git_cmd()
        .args(["init"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["config", "user.email", "test@test.com"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["config", "user.name", "Test"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    std::fs::write(temp_path.join(".gitkeep"), "").unwrap();
    git_cmd()
        .args(["add", "."])
        .current_dir(temp_path)
        .output()
        .unwrap();
    git_cmd()
        .args(["commit", "-m", "initial"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    std::fs::write(
        temp_path.join("topology.wh"),
        "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\nstreams: []\n",
    )
    .unwrap();

    // Write empty state.json
    let wh_dir = temp_path.join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    let empty_topo = wh_broker::deploy::Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![],
        streams: vec![],
        surfaces: vec![],
        guardrails: None,
    };
    std::fs::write(
        wh_dir.join("state.json"),
        serde_json::to_string(&empty_topo).unwrap(),
    )
    .unwrap();

    let wh_path = temp_path.join("topology.wh");
    let result = wh_broker::deploy::apply::destroy(&wh_path, None);
    assert!(
        result.is_ok(),
        "destroy on empty deployed state should succeed"
    );

    let destroy_result = result.unwrap();
    assert_eq!(destroy_result.destroyed, 0, "nothing to destroy");
}

/// AC #1: destroy commit uses ADR-003 format with agent name attribution.
#[test]
fn destroy_commit_uses_adr003_format() {
    let temp_dir = setup_deployed_repo();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    let _result = wh_broker::deploy::apply::destroy(&wh_path, Some("donna")).unwrap();

    let log_output = git_cmd()
        .args(["log", "--format=%s", "-1"])
        .current_dir(temp_path)
        .output()
        .unwrap();
    let commit_subject = String::from_utf8_lossy(&log_output.stdout)
        .trim()
        .to_string();
    assert!(
        commit_subject.starts_with("[donna] destroy:"),
        "commit should start with '[donna] destroy:': {commit_subject}"
    );
}

/// AC #1: DestroyResult Display format.
#[test]
fn destroy_result_display() {
    let result = wh_broker::deploy::apply::DestroyResult {
        destroyed: 2,
        streams_removed: 1,
        surfaces_destroyed: 0,
    };
    let display = result.to_string();
    assert!(
        display.contains("2 destroyed"),
        "should show destroyed count: {display}"
    );
    assert!(
        display.contains("1 streams removed"),
        "should show streams removed: {display}"
    );
}

// ── AC #2: Guardrails max_replicas blocks deployment ──

/// AC #2: max_replicas guardrail blocks plan when replicas exceed limit.
#[test]
fn guardrails_max_replicas_blocks_exceeding_replicas() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nguardrails:\n  max_replicas: 3\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 5\nstreams: []\n",
    ).unwrap();

    let linted = wh_broker::deploy::lint::lint(&wh_path).unwrap();
    let err = wh_broker::deploy::plan::plan(linted).unwrap_err();
    assert!(matches!(
        err,
        wh_broker::deploy::DeployError::PolicyViolation(_)
    ));
    let msg = err.to_string();
    assert!(
        msg.contains("researcher"),
        "error should mention agent name: {msg}"
    );
    assert!(
        msg.contains("5"),
        "error should mention requested replicas: {msg}"
    );
    assert!(
        msg.contains("3"),
        "error should mention max_replicas: {msg}"
    );
}

/// AC #2: max_replicas guardrail allows valid replicas.
#[test]
fn guardrails_max_replicas_allows_valid_replicas() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nguardrails:\n  max_replicas: 3\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 2\nstreams: []\n",
    ).unwrap();

    let linted = wh_broker::deploy::lint::lint(&wh_path).unwrap();
    let plan_output = wh_broker::deploy::plan::plan(linted).unwrap();
    assert!(plan_output.has_changes(), "should detect additions");
}

/// AC #2: max_replicas guardrail allows exact max.
#[test]
fn guardrails_max_replicas_allows_exact_max() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nguardrails:\n  max_replicas: 3\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 3\nstreams: []\n",
    ).unwrap();

    let linted = wh_broker::deploy::lint::lint(&wh_path).unwrap();
    let plan_output = wh_broker::deploy::plan::plan(linted).unwrap();
    assert!(
        plan_output.has_changes(),
        "should detect additions for exact max"
    );
}

// ── AC #3: Autonomous agent respects max_replicas ──

/// AC #3: autonomous plan_with_self_check still enforces max_replicas.
#[test]
fn autonomous_plan_respects_max_replicas() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nguardrails:\n  max_replicas: 3\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 5\nstreams: []\n",
    ).unwrap();

    let linted = wh_broker::deploy::lint::lint(&wh_path).unwrap();
    let err =
        wh_broker::deploy::plan::plan_with_self_check(linted, Some("researcher")).unwrap_err();
    assert!(matches!(
        err,
        wh_broker::deploy::DeployError::PolicyViolation(_)
    ));
}
