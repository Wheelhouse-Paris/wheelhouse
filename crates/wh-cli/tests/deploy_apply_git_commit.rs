//! Acceptance tests for Story 7.1: Operator-Driven Plan/Apply Loop (Donna Mode)
//! AC #2: `wh deploy apply --yes` creates git commit with plan_hash in message body
//!
//! These tests use the library API directly to validate the apply+commit logic,
//! because the test sandbox may prevent double process spawning (test -> wh -> git).

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

/// Helper: initialize a temp git repo with an initial commit and a .wh topology file.
fn setup_git_repo_with_topology() -> tempfile::TempDir {
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
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: researcher:latest\n    replicas: 2\n    streams:\n      - main\nstreams:\n  - name: main\n    retention: 7d\n",
    ).unwrap();

    temp_dir
}

/// AC #2: deploy apply creates git commit with plan_hash in message body (ADR-003).
/// Tests the library API directly: lint -> plan -> commit -> apply.
#[test]
fn apply_creates_git_commit_with_plan_hash() {
    let temp_dir = setup_git_repo_with_topology();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    // Run the deploy pipeline via library API
    let linted = wh_broker::deploy::lint::lint(&wh_path).expect("lint should succeed");
    let plan_output = wh_broker::deploy::plan::plan(linted).expect("plan should succeed");

    assert!(
        plan_output.has_changes(),
        "should have changes for first deploy"
    );

    let committed =
        wh_broker::deploy::apply::commit(plan_output, None).expect("commit should succeed");

    wh_broker::deploy::apply::apply(committed, &[]).expect("apply should succeed");

    // Verify git commit
    let git_log_output = git_cmd()
        .args(["log", "--format=%B", "-1"])
        .current_dir(temp_path)
        .output()
        .expect("git log failed");

    let commit_msg = String::from_utf8_lossy(&git_log_output.stdout);
    assert!(
        commit_msg.contains("Plan:"),
        "commit message must contain 'Plan:' with plan hash (ADR-003). Got: {commit_msg}"
    );
    assert!(
        commit_msg.contains("sha256:"),
        "commit message must contain sha256 plan hash. Got: {commit_msg}"
    );
}

/// AC #2: The commit contains agent name and change summary (ADR-003 format).
#[test]
fn apply_commit_contains_operator_name_and_change_summary() {
    let temp_dir = setup_git_repo_with_topology();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    let linted = wh_broker::deploy::lint::lint(&wh_path).expect("lint should succeed");
    let plan_output = wh_broker::deploy::plan::plan(linted).expect("plan should succeed");

    let committed =
        wh_broker::deploy::apply::commit(plan_output, None).expect("commit should succeed");

    wh_broker::deploy::apply::apply(committed, &[]).expect("apply should succeed");

    let git_log_output = git_cmd()
        .args(["log", "--format=%B", "-1"])
        .current_dir(temp_path)
        .output()
        .expect("git log failed");

    let commit_msg = String::from_utf8_lossy(&git_log_output.stdout);

    // ADR-003 format: [agent-name] apply: <summary>\n\nPlan: <plan-output-or-hash>
    assert!(
        commit_msg.contains("[operator] apply:"),
        "commit message must follow ADR-003 format: [operator] apply: <summary>. Got: {commit_msg}"
    );
}

/// AC #2: When a custom agent name is provided, it appears in the commit.
#[test]
fn apply_commit_uses_custom_agent_name() {
    let temp_dir = setup_git_repo_with_topology();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    let linted = wh_broker::deploy::lint::lint(&wh_path).expect("lint should succeed");
    let plan_output = wh_broker::deploy::plan::plan(linted).expect("plan should succeed");

    let committed = wh_broker::deploy::apply::commit(plan_output, Some("donna"))
        .expect("commit should succeed");

    wh_broker::deploy::apply::apply(committed, &[]).expect("apply should succeed");

    let git_log_output = git_cmd()
        .args(["log", "--format=%B", "-1"])
        .current_dir(temp_path)
        .output()
        .expect("git log failed");

    let commit_msg = String::from_utf8_lossy(&git_log_output.stdout);
    assert!(
        commit_msg.contains("[donna] apply:"),
        "commit message must use provided agent name. Got: {commit_msg}"
    );
}

/// FM-04: Applying the same topology twice produces the same result (idempotent).
#[test]
fn apply_is_idempotent() {
    let temp_dir = setup_git_repo_with_topology();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    // First apply
    let linted = wh_broker::deploy::lint::lint(&wh_path).expect("lint should succeed");
    let plan_output = wh_broker::deploy::plan::plan(linted).expect("plan should succeed");
    assert!(plan_output.has_changes());
    let committed =
        wh_broker::deploy::apply::commit(plan_output, None).expect("first commit should succeed");
    wh_broker::deploy::apply::apply(committed, &[]).expect("first apply should succeed");

    // Second apply — should detect no changes
    let linted2 = wh_broker::deploy::lint::lint(&wh_path).expect("lint should succeed");
    let plan_output2 = wh_broker::deploy::plan::plan(linted2).expect("plan should succeed");
    assert!(
        !plan_output2.has_changes(),
        "second plan should detect no changes (idempotent)"
    );
}
