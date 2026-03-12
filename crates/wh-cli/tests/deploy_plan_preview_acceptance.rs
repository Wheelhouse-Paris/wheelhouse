//! Acceptance tests for Story 2-3: `wh deploy plan` — Preview Changes
//!
//! TDD Red Phase: These tests verify the acceptance criteria for story 2-3.
//! Tests cover human-readable output format, self-destruct detection (CM-05),
//! and JSON output compliance.

use std::process::Command;

fn wh_binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wh"))
}

// ===========================================================================
// AC #1: Human output shows "+ agent researcher (new)" and summary line
// ===========================================================================

/// AC #1: Given a `.wh` file that would create 1 agent and 1 stream (first deploy),
/// When I run `wh deploy plan topology.wh`,
/// Then the output shows `+ agent researcher (new)` and `+ stream main (new)`
/// And the summary line reads `2 to create · 0 to update · 0 to destroy`.
#[test]
fn plan_human_output_shows_new_annotations_and_summary() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let wh_path = temp_dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: researcher:latest\nstreams:\n  - name: main\n",
    )
    .unwrap();

    let output = wh_binary()
        .args(["deploy", "plan", wh_path.to_str().unwrap()])
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("+ agent researcher (new)"),
        "should show '+ agent researcher (new)' in output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("+ stream main (new"),
        "should show '+ stream main (new...' in output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("2 to create"),
        "should show '2 to create' in summary, got:\n{stdout}"
    );
    assert!(
        stdout.contains("0 to update"),
        "should show '0 to update' in summary, got:\n{stdout}"
    );
    assert!(
        stdout.contains("0 to destroy"),
        "should show '0 to destroy' in summary, got:\n{stdout}"
    );
}

/// AC #1: Stream additions include provider info.
#[test]
fn plan_human_output_shows_provider_info_for_streams() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let wh_path = temp_dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\nstreams:\n  - name: main\n",
    )
    .unwrap();

    let output = wh_binary()
        .args(["deploy", "plan", wh_path.to_str().unwrap()])
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Provider defaults to "local" when absent
    assert!(
        stdout.contains("provider: local") || stdout.contains("local"),
        "should show provider info for new streams, got:\n{stdout}"
    );
}

// ===========================================================================
// AC #2: No changes message
// ===========================================================================

/// AC #2: Given a plan is run on an already-deployed topology with no changes,
/// When I run `wh deploy plan topology.wh`,
/// Then the output reads `No changes. Infrastructure is up-to-date.`
#[test]
fn plan_human_output_shows_no_changes_message() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let wh_path = temp_dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: researcher:latest\n    replicas: 1\n    streams:\n      - main\nstreams:\n  - name: main\n    retention: 7d\n",
    )
    .unwrap();

    let wh_dir = temp_dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"researcher","image":"researcher:latest","replicas":1,"streams":["main"]}],"streams":[{"name":"main","retention":"7d"}]}"#,
    )
    .unwrap();

    let output = wh_binary()
        .args(["deploy", "plan", wh_path.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No changes") && stdout.contains("up-to-date"),
        "should show no-changes message, got:\n{stdout}"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "exit code should be 0 when no changes"
    );
}

// ===========================================================================
// AC #3: JSON output compliance (extends existing tests from 7-1)
// ===========================================================================

/// AC #3: Plan output goes to stdout only, errors to stderr.
#[test]
fn plan_json_output_goes_to_stdout_only() {
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
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Plan output should be valid JSON on stdout
    let _json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout should be valid JSON: {e}\nstdout: {stdout}"));

    // stderr should be empty for successful plan
    assert!(
        stderr.trim().is_empty(),
        "stderr should be empty for successful plan, got:\n{stderr}"
    );
}

/// AC #3: Exit code 2 when changes detected.
#[test]
fn plan_exits_with_code_2_on_changes() {
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

    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code should be 2 when changes detected"
    );
}

// ===========================================================================
// AC #5: Self-destruct detection (CM-05)
// ===========================================================================

/// AC #5: Given a plan would destroy all agents (self-destruct),
/// When `wh deploy plan` evaluates the diff,
/// Then the plan exits with error and a prominent warning.
#[test]
fn plan_blocks_self_destruct_without_force_flag() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");

    // Current state has 2 agents
    let wh_dir = temp_dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"researcher","image":"r:latest","replicas":1,"streams":[]},{"name":"writer","image":"w:latest","replicas":1,"streams":[]}],"streams":[]}"#,
    )
    .unwrap();

    // Desired state has 0 agents (self-destruct)
    let wh_path = temp_dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\nstreams: []\n",
    )
    .unwrap();

    let output = wh_binary()
        .args(["deploy", "plan", wh_path.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .expect("failed to execute wh binary");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code should be 1 when self-destruct is blocked"
    );
    assert!(
        stderr.contains("destroy")
            || stderr.contains("self-destruct")
            || stderr.contains("--force-destroy-all"),
        "error should mention self-destruct or --force-destroy-all, got:\n{stderr}"
    );
}

/// AC #5: With --force-destroy-all flag, self-destruct plan proceeds with warning.
#[test]
fn plan_allows_self_destruct_with_force_flag() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");

    let wh_dir = temp_dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"researcher","image":"r:latest","replicas":1,"streams":[]}],"streams":[]}"#,
    )
    .unwrap();

    let wh_path = temp_dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\nstreams: []\n",
    )
    .unwrap();

    let output = wh_binary()
        .args([
            "deploy",
            "plan",
            wh_path.to_str().unwrap(),
            "--force-destroy-all",
        ])
        .current_dir(temp_dir.path())
        .output()
        .expect("failed to execute wh binary");

    // Should succeed (exit 2 = changes detected)
    assert_eq!(
        output.status.code(),
        Some(2),
        "exit code should be 2 (changes detected) when force flag is provided"
    );
}

/// AC #5: Self-destruct JSON output includes warning.
#[test]
fn plan_self_destruct_with_force_json_includes_warning() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");

    let wh_dir = temp_dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"researcher","image":"r:latest","replicas":1,"streams":[]}],"streams":[]}"#,
    )
    .unwrap();

    let wh_path = temp_dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\nstreams: []\n",
    )
    .unwrap();

    let output = wh_binary()
        .args([
            "deploy",
            "plan",
            wh_path.to_str().unwrap(),
            "--force-destroy-all",
            "--format",
            "json",
        ])
        .current_dir(temp_dir.path())
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("should be valid JSON: {e}\nstdout: {stdout}"));

    // Warnings should include self-destruct notice
    let warnings = json["data"]["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(
        warnings.iter().any(|w| {
            w.as_str()
                .map(|s| s.contains("destroy") || s.contains("self-destruct"))
                .unwrap_or(false)
        }),
        "warnings should mention destroy/self-destruct, got: {:?}",
        warnings
    );
}

// ===========================================================================
// Human output: modification and summary
// ===========================================================================

/// Test that a modification shows the update details.
#[test]
fn plan_human_output_shows_modification_details() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");

    let wh_dir = temp_dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"researcher","image":"r:latest","replicas":1,"streams":[]}],"streams":[]}"#,
    )
    .unwrap();

    let wh_path = temp_dir.path().join("topology.wh");
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 2\n",
    )
    .unwrap();

    let output = wh_binary()
        .args(["deploy", "plan", wh_path.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("~") && stdout.contains("researcher"),
        "should show modification marker for changed agent, got:\n{stdout}"
    );
    assert!(
        stdout.contains("0 to create"),
        "summary should show 0 to create, got:\n{stdout}"
    );
    assert!(
        stdout.contains("1 to update"),
        "summary should show 1 to update, got:\n{stdout}"
    );
}
