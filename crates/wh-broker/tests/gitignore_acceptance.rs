//! Acceptance tests for Story 2.6: Git Backend — Versioning, Recovery, and Secrets Exclusion
//!
//! AC #1: Deploy apply stages .wh/ directory contents in git commit (FR28)
//! AC #2: Recovery via git clone + deploy apply restores infrastructure (NFR-I3)
//! AC #3: Secrets never appear in committed files (NFR-S2)
//! AC #4: WAL, secrets, and lock files excluded from git

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
fn setup_git_repo() -> tempfile::TempDir {
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

    temp_dir
}

// ── AC #1: Deploy apply stages .wh/ directory ──

/// AC #1: After deploy apply, .wh/state.json is committed.
#[test]
fn deploy_apply_commits_state_json() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    let linted = wh_broker::deploy::lint::lint(&wh_path).expect("lint should succeed");
    let plan_output = wh_broker::deploy::plan::plan(linted).expect("plan should succeed");
    assert!(plan_output.has_changes());

    let committed =
        wh_broker::deploy::apply::commit(plan_output, None).expect("commit should succeed");
    let _ = wh_broker::deploy::apply::apply(committed);

    // Verify .wh/state.json is in the commit
    let output = git_cmd()
        .args(["show", "--stat", "HEAD"])
        .current_dir(temp_path)
        .output()
        .expect("git show failed");
    let stat = String::from_utf8_lossy(&output.stdout);
    assert!(
        stat.contains(".wh/state.json"),
        "commit must include .wh/state.json. Got: {stat}"
    );
}

/// AC #1: After deploy apply, .wh/.gitignore is committed.
#[test]
fn deploy_apply_creates_gitignore() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    let linted = wh_broker::deploy::lint::lint(&wh_path).expect("lint should succeed");
    let plan_output = wh_broker::deploy::plan::plan(linted).expect("plan should succeed");
    let committed =
        wh_broker::deploy::apply::commit(plan_output, None).expect("commit should succeed");
    let _ = wh_broker::deploy::apply::apply(committed);

    // Verify .wh/.gitignore exists and is committed
    let gitignore_path = temp_path.join(".wh").join(".gitignore");
    assert!(
        gitignore_path.exists(),
        ".wh/.gitignore must exist after deploy apply"
    );

    let output = git_cmd()
        .args(["show", "--stat", "HEAD"])
        .current_dir(temp_path)
        .output()
        .expect("git show failed");
    let stat = String::from_utf8_lossy(&output.stdout);
    assert!(
        stat.contains(".wh/.gitignore"),
        "commit must include .wh/.gitignore. Got: {stat}"
    );
}

// ── AC #3: Secrets exclusion ──

/// AC #3: .wh/.gitignore excludes WAL files.
#[test]
fn gitignore_excludes_wal_files() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();

    // Create .wh dir and gitignore
    wh_broker::deploy::gitignore::ensure_gitignore(temp_path)
        .expect("ensure_gitignore should succeed");

    let gitignore =
        std::fs::read_to_string(temp_path.join(".wh/.gitignore")).expect("should read .gitignore");

    assert!(
        gitignore.contains("*.db"),
        ".gitignore must exclude *.db WAL files"
    );
    assert!(
        gitignore.contains("*.db-wal"),
        ".gitignore must exclude *.db-wal"
    );
    assert!(
        gitignore.contains("*.db-shm"),
        ".gitignore must exclude *.db-shm"
    );
}

/// AC #3: .wh/.gitignore excludes secret files.
#[test]
fn gitignore_excludes_secrets() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();

    wh_broker::deploy::gitignore::ensure_gitignore(temp_path)
        .expect("ensure_gitignore should succeed");

    let gitignore =
        std::fs::read_to_string(temp_path.join(".wh/.gitignore")).expect("should read .gitignore");

    assert!(
        gitignore.contains(".env"),
        ".gitignore must exclude .env files"
    );
    assert!(
        gitignore.contains("secrets/"),
        ".gitignore must exclude secrets/ directory"
    );
    assert!(
        gitignore.contains("*.token"),
        ".gitignore must exclude *.token files"
    );
}

/// AC #3: .wh/.gitignore excludes lock files.
#[test]
fn gitignore_excludes_lock_files() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();

    wh_broker::deploy::gitignore::ensure_gitignore(temp_path)
        .expect("ensure_gitignore should succeed");

    let gitignore =
        std::fs::read_to_string(temp_path.join(".wh/.gitignore")).expect("should read .gitignore");

    assert!(
        gitignore.contains("workspace.lock"),
        ".gitignore must exclude workspace.lock"
    );
}

/// AC #3: ensure_gitignore preserves existing user patterns.
#[test]
fn gitignore_preserves_user_patterns() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();

    // Pre-create .wh/.gitignore with user content
    let wh_dir = temp_path.join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join(".gitignore"),
        "# my custom rule\ncustom_exclude/\n",
    )
    .unwrap();

    wh_broker::deploy::gitignore::ensure_gitignore(temp_path)
        .expect("ensure_gitignore should succeed");

    let gitignore =
        std::fs::read_to_string(wh_dir.join(".gitignore")).expect("should read .gitignore");

    assert!(
        gitignore.contains("custom_exclude/"),
        "must preserve user patterns"
    );
    assert!(
        gitignore.contains("*.db"),
        "must also add required patterns"
    );
}

/// AC #3: Staging a file that matches secret patterns is blocked.
#[test]
fn secrets_scan_detects_suspicious_files() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();

    // Create a suspicious file in .wh/
    let wh_dir = temp_path.join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(wh_dir.join("api.token"), "sk-secret-value").unwrap();

    // Stage it
    git_cmd()
        .args(["add", ".wh/api.token"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    let result = wh_broker::deploy::gitignore::scan_staged_for_secrets(temp_path);
    assert!(result.is_ok(), "scan should succeed");
    let suspicious = result.unwrap();
    assert!(
        !suspicious.is_empty(),
        "should detect suspicious staged file"
    );
}

/// AC #3: Normal files are not flagged by secrets scan.
#[test]
fn secrets_scan_allows_normal_files() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();

    // Create a normal file in .wh/
    let wh_dir = temp_path.join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(wh_dir.join("state.json"), "{}").unwrap();

    // Stage it
    git_cmd()
        .args(["add", ".wh/state.json"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    let result = wh_broker::deploy::gitignore::scan_staged_for_secrets(temp_path);
    assert!(result.is_ok());
    let suspicious = result.unwrap();
    assert!(
        suspicious.is_empty(),
        "should not flag normal files. Got: {suspicious:?}"
    );
}

// ── AC #2, #4: Recovery flow ──

/// AC #2: After deploy and state deletion (simulating fresh clone), plan shows all additions.
#[test]
fn recovery_plan_detects_all_as_additions() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();
    let wh_path = temp_path.join("topology.wh");

    // First deploy
    let linted = wh_broker::deploy::lint::lint(&wh_path).expect("lint");
    let plan = wh_broker::deploy::plan::plan(linted).expect("plan");
    assert!(plan.has_changes());
    let committed = wh_broker::deploy::apply::commit(plan, None).expect("commit");
    let _ = wh_broker::deploy::apply::apply(committed);

    // Simulate fresh clone: remove .wh/state.json
    let state_path = temp_path.join(".wh/state.json");
    if state_path.exists() {
        std::fs::remove_file(&state_path).unwrap();
    }

    // Re-plan: should see all components as additions (same as first deploy)
    let linted2 = wh_broker::deploy::lint::lint(&wh_path).expect("lint2");
    let plan2 = wh_broker::deploy::plan::plan(linted2).expect("plan2");
    assert!(
        plan2.has_changes(),
        "after removing state.json, plan should detect additions (recovery scenario)"
    );
}

/// AC #4: .wh/.gitignore content ensures WAL and secrets are not restored via git.
#[test]
fn gitignore_ensures_non_restorable_files_excluded() {
    let temp_dir = setup_git_repo();
    let temp_path = temp_dir.path();

    wh_broker::deploy::gitignore::ensure_gitignore(temp_path)
        .expect("ensure_gitignore should succeed");

    // Create files that should be ignored
    let wh_dir = temp_path.join(".wh");
    std::fs::write(wh_dir.join("stream.db"), "wal data").unwrap();
    std::fs::write(wh_dir.join("stream.db-wal"), "wal data").unwrap();
    std::fs::write(wh_dir.join("workspace.lock"), "lock").unwrap();

    // Stage .wh/ and check what gets staged
    git_cmd()
        .args(["add", ".wh/"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    let output = git_cmd()
        .args(["diff", "--cached", "--name-only"])
        .current_dir(temp_path)
        .output()
        .expect("git diff failed");
    let staged = String::from_utf8_lossy(&output.stdout);

    assert!(
        !staged.contains("stream.db"),
        "WAL file should not be staged"
    );
    assert!(
        !staged.contains("workspace.lock"),
        "lock file should not be staged"
    );
    // .gitignore itself should be staged
    assert!(
        staged.contains(".wh/.gitignore"),
        ".gitignore should be staged"
    );
}
