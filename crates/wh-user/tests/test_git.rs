use std::process::Command;
use tempfile::TempDir;
use wh_proto::UserProfile;
use wh_user::GitBackend;

fn init_git_repo(path: &std::path::Path) {
    Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("failed to init git repo");

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(path)
        .output()
        .expect("failed to set git email");

    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .expect("failed to set git name");

    // Create an initial commit so HEAD exists
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial commit"])
        .current_dir(path)
        .output()
        .expect("failed to create initial commit");
}

#[test]
fn test_git_commit_user_profile() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    init_git_repo(workspace);

    // Create user profile file
    let users_dir = workspace.join(".wh").join("users");
    std::fs::create_dir_all(&users_dir).unwrap();

    let profile = UserProfile {
        user_id: "usr_abc123def456ab".to_string(),
        platform: "cli".to_string(),
        display_name: "Alice".to_string(),
        created_at: "2026-03-12T10:30:00Z".to_string(),
    };

    let profile_path = users_dir.join("usr_abc123def456ab.yaml");
    let yaml = serde_yaml::to_string(&profile).unwrap();
    std::fs::write(&profile_path, &yaml).unwrap();

    // Commit the profile
    let result = GitBackend::commit_user_profile(workspace, &profile_path, &profile);
    assert!(result.is_ok(), "commit failed: {:?}", result.err());

    // Verify git log contains the commit
    let output = Command::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(workspace)
        .output()
        .expect("failed to read git log");

    let log = String::from_utf8_lossy(&output.stdout);
    assert!(
        log.contains("feat(user): register Alice (cli)"),
        "unexpected commit message: {log}"
    );
}

#[test]
fn test_git_not_initialized_returns_error() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    // Do NOT init git — this should fail
    let profile = UserProfile {
        user_id: "usr_abc123def456ab".to_string(),
        platform: "cli".to_string(),
        display_name: "Alice".to_string(),
        created_at: "2026-03-12T10:30:00Z".to_string(),
    };

    let profile_path = workspace.join("usr_abc123def456ab.yaml");
    let result = GitBackend::commit_user_profile(workspace, &profile_path, &profile);
    assert!(result.is_err());

    // Verify it's the right error variant
    match result.unwrap_err() {
        wh_user::UserError::GitNotInitialized => {} // expected
        other => panic!("expected GitNotInitialized, got: {other:?}"),
    }
}
