use std::process::Command;
use tempfile::TempDir;
use wh_user::{GitBackend, UserStore};

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

    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial commit"])
        .current_dir(path)
        .output()
        .expect("failed to create initial commit");
}

/// End-to-end test: register a user and commit to git.
#[test]
fn test_register_and_commit_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    init_git_repo(workspace);

    let store = UserStore::new(workspace);

    // Register user
    let profile = store.register("cli", "alice", "Alice").unwrap();
    assert!(profile.user_id.starts_with("usr_"));

    // Commit to git
    let profile_path = workspace
        .join(".wh")
        .join("users")
        .join(format!("{}.yaml", profile.user_id));
    let result = GitBackend::commit_user_profile(workspace, &profile_path, &profile);
    assert!(result.is_ok(), "commit failed: {:?}", result.err());

    // Verify git log
    let output = Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(workspace)
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&output.stdout);
    assert!(log.contains("feat(user): register Alice (cli)"));

    // Verify profile can be looked up
    let looked_up = store.lookup(&profile.user_id).unwrap();
    assert!(looked_up.is_some());
    assert_eq!(looked_up.unwrap().display_name, "Alice");
}

/// End-to-end test: two users produce distinct IDs.
#[test]
fn test_two_users_distinct_attribution() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    let store = UserStore::new(workspace);

    let alice = store.register("cli", "alice", "Alice").unwrap();
    let bob = store.register("cli", "bob", "Bob").unwrap();

    assert_ne!(alice.user_id, bob.user_id);

    // Both profiles exist on disk
    let alice_path = workspace
        .join(".wh")
        .join("users")
        .join(format!("{}.yaml", alice.user_id));
    let bob_path = workspace
        .join(".wh")
        .join("users")
        .join(format!("{}.yaml", bob.user_id));
    assert!(alice_path.exists());
    assert!(bob_path.exists());
}
