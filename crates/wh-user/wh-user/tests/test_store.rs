use tempfile::TempDir;
use wh_user::{generate_user_id, UserStore};

#[test]
fn test_register_creates_profile_file() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let profile = store.register("cli", "alice", "Alice").unwrap();

    assert!(profile.user_id.starts_with("usr_"));
    assert_eq!(profile.platform, "cli");
    assert_eq!(profile.display_name, "Alice");
    assert!(!profile.created_at.is_empty());

    // Verify file exists on disk
    let expected_path = tmp
        .path()
        .join(".wh")
        .join("users")
        .join(format!("{}.yaml", profile.user_id));
    assert!(expected_path.exists());
}

#[test]
fn test_register_deduplication_returns_existing() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let first = store.register("cli", "alice", "Alice").unwrap();
    let second = store.register("cli", "alice", "Alice Different Name").unwrap();

    // Should return the original profile, not create a new one
    assert_eq!(first.user_id, second.user_id);
    assert_eq!(first.display_name, second.display_name);
    assert_eq!(first.created_at, second.created_at);
}

#[test]
fn test_register_different_users_produce_distinct_ids() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let alice = store.register("cli", "alice", "Alice").unwrap();
    let bob = store.register("cli", "bob", "Bob").unwrap();

    assert_ne!(alice.user_id, bob.user_id);
}

#[test]
fn test_register_same_user_different_platforms_produce_distinct_ids() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let cli_alice = store.register("cli", "alice", "Alice CLI").unwrap();
    let tg_alice = store.register("telegram", "alice", "Alice TG").unwrap();

    assert_ne!(cli_alice.user_id, tg_alice.user_id);
}

#[test]
fn test_lookup_nonexistent_returns_none() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let result = store.lookup("usr_nonexistent12345").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_lookup_existing_returns_profile() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let registered = store.register("cli", "alice", "Alice").unwrap();
    let looked_up = store.lookup(&registered.user_id).unwrap();

    assert!(looked_up.is_some());
    let looked_up = looked_up.unwrap();
    assert_eq!(registered.user_id, looked_up.user_id);
    assert_eq!(registered.platform, looked_up.platform);
    assert_eq!(registered.display_name, looked_up.display_name);
}

#[test]
fn test_user_id_deterministic() {
    let id1 = generate_user_id("cli", "alice");
    let id2 = generate_user_id("cli", "alice");
    assert_eq!(id1, id2);
}

#[test]
fn test_user_id_different_inputs_different_ids() {
    let id1 = generate_user_id("cli", "alice");
    let id2 = generate_user_id("cli", "bob");
    let id3 = generate_user_id("telegram", "alice");
    assert_ne!(id1, id2);
    assert_ne!(id1, id3);
}

#[test]
fn test_user_id_format() {
    let id = generate_user_id("cli", "alice");
    assert!(id.starts_with("usr_"));
    assert_eq!(id.len(), 20); // "usr_" (4) + 16 hex chars
    // Verify hex chars only after prefix
    assert!(id[4..].chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_platform_validation_rejects_empty() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let result = store.register("", "alice", "Alice");
    assert!(result.is_err());
}

#[test]
fn test_platform_validation_rejects_uppercase() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let result = store.register("CLI", "alice", "Alice");
    assert!(result.is_err());
}

#[test]
fn test_platform_validation_rejects_starting_with_digit() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let result = store.register("1cli", "alice", "Alice");
    assert!(result.is_err());
}

#[test]
fn test_display_name_validation_rejects_empty() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let result = store.register("cli", "alice", "");
    assert!(result.is_err());
}

#[test]
fn test_platform_user_id_validation_rejects_empty() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let result = store.register("cli", "", "Alice");
    assert!(result.is_err());
}

#[test]
fn test_field_too_long_platform() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let long_platform = "a".repeat(65);
    let result = store.register(&long_platform, "alice", "Alice");
    assert!(result.is_err());
}

#[test]
fn test_field_too_long_display_name() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let long_name = "A".repeat(257);
    let result = store.register("cli", "alice", &long_name);
    assert!(result.is_err());
}

#[test]
fn test_profile_yaml_content() {
    let tmp = TempDir::new().unwrap();
    let store = UserStore::new(tmp.path());

    let profile = store.register("cli", "alice", "Alice").unwrap();

    let path = tmp
        .path()
        .join(".wh")
        .join("users")
        .join(format!("{}.yaml", profile.user_id));
    let content = std::fs::read_to_string(&path).unwrap();

    assert!(content.contains(&profile.user_id));
    assert!(content.contains("cli"));
    assert!(content.contains("Alice"));
    assert!(content.contains(&profile.created_at));
}
