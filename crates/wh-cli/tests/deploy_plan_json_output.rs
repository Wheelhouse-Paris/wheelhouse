//! Acceptance tests for Story 7.1: Operator-Driven Plan/Apply Loop (Donna Mode)
//! AC #1: `wh deploy plan --format json` returns structured JSON with has_changes, changes, plan_hash
//! AC #3: JSON output matches v1.0 fixture schema — all required fields present

use std::process::Command;

fn wh_binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wh"))
}

/// AC #1: Given a first deploy (no prior state),
/// When I run `wh deploy plan topology.wh --format json`,
/// Then the JSON output includes `has_changes: true`, changes, and a `plan_hash` field,
/// And the exit code is `2` (change detected).
#[test]
fn plan_json_includes_has_changes_and_plan_hash_when_changes_exist() {
    let output = wh_binary()
        .args([
            "deploy",
            "plan",
            "tests/fixtures/modified.wh",
            "--format",
            "json",
        ])
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output should be valid JSON: {e}\nstdout: {stdout}"));

    // Required fields per architecture spec
    assert_eq!(json["v"], 1, "schema version must be 1");
    assert_eq!(json["status"], "ok", "status must be ok");
    assert_eq!(
        json["data"]["has_changes"], true,
        "has_changes must be true when topology differs"
    );
    assert!(
        json["data"]["changes"].is_array(),
        "changes must be an array"
    );
    assert!(
        !json["data"]["changes"].as_array().unwrap().is_empty(),
        "changes array must not be empty"
    );
    assert!(
        json["data"]["plan_hash"].is_string(),
        "plan_hash must be present as string"
    );
    assert!(
        json["data"]["topology_name"].is_string(),
        "topology_name must be present"
    );

    // Exit code 2 = change detected (ADR-014)
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code must be 2 when changes detected"
    );
}

/// AC #1 (negative): When state matches desired, exit code 0 and has_changes false.
#[test]
fn plan_json_returns_no_changes_when_topology_unchanged() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let temp_path = temp_dir.path();

    std::fs::write(
        temp_path.join("topology.wh"),
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: researcher:latest\n    replicas: 1\n    streams:\n      - main\nstreams:\n  - name: main\n    retention: 7d\n",
    ).unwrap();

    let wh_dir = temp_path.join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"researcher","image":"researcher:latest","replicas":1,"streams":["main"]}],"streams":[{"name":"main","retention":"7d"}]}"#,
    ).unwrap();

    let output = wh_binary()
        .args(["deploy", "plan", "topology.wh", "--format", "json"])
        .current_dir(temp_path)
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output should be valid JSON: {e}\nstdout: {stdout}"));

    assert_eq!(
        json["data"]["has_changes"], false,
        "has_changes must be false when no changes"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code must be 0 when no changes"
    );
}

/// AC #3: All required v1 fields present in JSON output.
#[test]
fn plan_json_schema_contains_all_v1_required_fields() {
    let output = wh_binary()
        .args([
            "deploy",
            "plan",
            "tests/fixtures/modified.wh",
            "--format",
            "json",
        ])
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output should be valid JSON: {e}\nstdout: {stdout}"));

    assert!(json.get("v").is_some(), "missing required field: v");
    assert!(
        json.get("status").is_some(),
        "missing required field: status"
    );
    assert!(json.get("data").is_some(), "missing required field: data");

    let data = &json["data"];
    let required_fields = [
        "has_changes",
        "changes",
        "plan_hash",
        "topology_name",
        "policy_snapshot_hash",
        "warnings",
    ];
    for field in &required_fields {
        assert!(
            data.get(field).is_some(),
            "missing required data field: {field}"
        );
    }

    if let Some(changes) = data["changes"].as_array() {
        if let Some(change) = changes.first() {
            assert!(change.get("op").is_some(), "change item missing 'op' field");
            assert!(
                change.get("component").is_some(),
                "change item missing 'component' field"
            );
        }
    }
}

/// Error path: malformed .wh file returns exit code 1.
#[test]
fn plan_malformed_wh_file_returns_error() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let bad_file = temp_dir.path().join("bad.wh");
    std::fs::write(&bad_file, "not valid yaml: {{{").unwrap();

    let output = wh_binary()
        .args([
            "deploy",
            "plan",
            bad_file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to execute wh binary");

    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code must be 1 for errors"
    );
}

/// Error path: missing .wh file returns exit code 1.
#[test]
fn plan_missing_file_returns_error() {
    let output = wh_binary()
        .args(["deploy", "plan", "/nonexistent/path.wh", "--format", "json"])
        .output()
        .expect("failed to execute wh binary");

    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code must be 1 for missing file"
    );
}
