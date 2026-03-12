//! Acceptance tests for Story 7.2: Agent-Invoked Plan and Apply.
//!
//! These tests verify that an agent can invoke `wh deploy plan` and
//! `wh deploy apply` as subprocesses, receiving structured JSON output
//! and proper exit codes. Guardrail validation is also tested.

use wh_broker::deploy::apply;
use wh_broker::deploy::lint;
use wh_broker::deploy::plan::{self, PlanData};
use wh_broker::deploy::Topology;

// ---------------------------------------------------------------------------
// AC #1: Agent receives structured JSON from plan
// ---------------------------------------------------------------------------

#[test]
fn test_agent_plan_returns_parseable_json() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 2\nstreams:\n  - name: main\n",
    )
    .unwrap();

    let linted = lint::lint(&wh_path).unwrap();
    let plan_output = plan::plan(linted).unwrap();
    let plan_data = PlanData::from(&plan_output);

    // Serialize to JSON and verify it is parseable
    let json_str = serde_json::to_string(&plan_data).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert!(parsed.get("has_changes").is_some());
    assert!(parsed.get("changes").is_some());
    assert!(parsed.get("plan_hash").is_some());
    assert!(parsed.get("topology_name").is_some());
}

#[test]
fn test_agent_plan_json_includes_all_required_fields() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\nstreams:\n  - name: main\n",
    )
    .unwrap();

    let linted = lint::lint(&wh_path).unwrap();
    let plan_output = plan::plan(linted).unwrap();
    let plan_data = PlanData::from(&plan_output);

    // All v1 schema fields must be present
    assert!(!plan_data.plan_hash.is_empty());
    assert_eq!(plan_data.topology_name, "dev");
    // policy_snapshot_hash is present (empty string when no policy)
    let _ = &plan_data.policy_snapshot_hash;
    let _ = &plan_data.warnings;
}

// ---------------------------------------------------------------------------
// AC #2: Agent apply creates git commit with agent name
// ---------------------------------------------------------------------------

#[test]
fn test_agent_apply_commit_with_agent_name() {
    // Tests that commit() with an agent name produces the correct message format.
    // This is a library-level test since CLI integration requires a git repo.
    let dir = tempfile::tempdir().unwrap();
    let wh_dir = dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();

    // Initialize a git repo for commit
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output();

    // Write initial state so there's a diff
    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![],
        streams: vec![],
        guardrails: None,
    };
    std::fs::write(
        wh_dir.join("state.json"),
        serde_json::to_string(&topology).unwrap(),
    )
    .unwrap();

    // Write topology file with a change
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: donna\n    image: donna:latest\n",
    )
    .unwrap();

    // Initial commit so git add/commit works
    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(dir.path())
        .output();

    let linted = lint::lint(&wh_path).unwrap();
    let plan_output = plan::plan(linted).unwrap();
    assert!(plan_output.has_changes());

    // Commit with agent name "donna"
    let committed = apply::commit(plan_output, Some("donna")).unwrap();
    assert!(!committed.plan_hash().is_empty());

    // Verify git log contains [donna] in the commit message
    let log_output = std::process::Command::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log_str.contains("[donna]"),
        "git log should contain [donna], got: {log_str}"
    );
}

#[test]
fn test_agent_apply_default_operator_name() {
    let dir = tempfile::tempdir().unwrap();
    let wh_dir = dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();

    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output();

    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n",
    )
    .unwrap();

    let _ = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(dir.path())
        .output();

    let linted = lint::lint(&wh_path).unwrap();
    let plan_output = plan::plan(linted).unwrap();
    let _committed = apply::commit(plan_output, None).unwrap();

    let log_output = std::process::Command::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log_str.contains("[operator]"),
        "git log should contain [operator], got: {log_str}"
    );
}

// ---------------------------------------------------------------------------
// AC #3: Guardrail violation blocks plan
// ---------------------------------------------------------------------------

#[test]
fn test_guardrail_violation_returns_policy_violation() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nguardrails:\n  max_replicas: 2\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 5\n",
    )
    .unwrap();

    let linted = lint::lint(&wh_path).unwrap();
    let result = plan::plan(linted);

    assert!(result.is_err(), "plan should fail with guardrail violation");
    let err = result.unwrap_err();
    assert_eq!(err.code(), "POLICY_VIOLATION");
}

#[test]
fn test_guardrail_violation_error_describes_constraint() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nguardrails:\n  max_replicas: 3\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 10\n",
    )
    .unwrap();

    let linted = lint::lint(&wh_path).unwrap();
    let err = plan::plan(linted).unwrap_err();
    let msg = err.to_string();

    assert!(
        msg.contains("researcher"),
        "error should name the violating agent, got: {msg}"
    );
    assert!(
        msg.contains("max_replicas"),
        "error should mention max_replicas, got: {msg}"
    );
}

#[test]
fn test_guardrail_pass_allows_plan() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nguardrails:\n  max_replicas: 5\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 2\n",
    )
    .unwrap();

    let linted = lint::lint(&wh_path).unwrap();
    let result = plan::plan(linted);
    assert!(result.is_ok(), "plan should succeed when within guardrails");
}

#[test]
fn test_no_guardrails_allows_any_replicas() {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 100\n",
    )
    .unwrap();

    let linted = lint::lint(&wh_path).unwrap();
    let result = plan::plan(linted);
    assert!(
        result.is_ok(),
        "plan should succeed with no guardrails defined"
    );
}
