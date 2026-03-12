//! Acceptance tests for Story 7.4: Agent MEMORY.md Updates via Git.
//!
//! These tests verify:
//! - AC #1: Agent updates MEMORY.md with attributed git commit (agent name + timestamp)
//! - AC #2: Updated MEMORY.md is available after restart (read_memory returns persisted content)
//! - AC #3: Sequential writes both succeed without corruption (CM-04 timeout compliance)

/// Helper: create a temp directory with a git repo initialized.
fn setup_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();

    // Initialize git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    // Create an initial commit so HEAD exists
    std::fs::write(dir.path().join(".gitkeep"), "").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    dir
}

/// Helper: read the last git log message from the workspace.
fn last_commit_message(workspace: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%B"])
        .current_dir(workspace)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

// =============================================================================
// AC #1: Agent updates MEMORY.md with attributed git commit
// =============================================================================

#[test]
fn write_memory_creates_git_commit_with_agent_name_and_timestamp() {
    use wh_broker::deploy::memory::write_memory;

    let dir = setup_workspace();
    let result = write_memory(
        dir.path(),
        "donna",
        "Detected recurring timeout pattern on researcher agent",
        "topology change decision",
    )
    .unwrap();

    // Commit should exist
    assert!(
        !result.commit_hash.is_empty(),
        "commit hash should not be empty"
    );

    // Commit message should contain agent name
    let msg = last_commit_message(dir.path());
    assert!(
        msg.contains("[donna]"),
        "commit message should contain agent name attribution: {msg}"
    );
    assert!(
        msg.contains("memory:"),
        "commit message should contain 'memory:' prefix: {msg}"
    );
    assert!(
        msg.contains("topology change decision"),
        "commit message should contain the reason: {msg}"
    );
    // Commit body should contain timestamp
    assert!(
        msg.contains("Timestamp:"),
        "commit body should contain timestamp: {msg}"
    );
    assert!(
        msg.contains("Agent: donna"),
        "commit body should contain agent attribution: {msg}"
    );
}

#[test]
fn write_memory_creates_file_at_correct_path() {
    use wh_broker::deploy::memory::write_memory;

    let dir = setup_workspace();
    write_memory(dir.path(), "donna", "Test memory content", "test reason").unwrap();

    let memory_path = dir.path().join(".wh/agents/donna/MEMORY.md");
    assert!(
        memory_path.exists(),
        "MEMORY.md should exist at .wh/agents/donna/MEMORY.md"
    );
    let content = std::fs::read_to_string(&memory_path).unwrap();
    assert_eq!(content, "Test memory content");
}

#[test]
fn write_memory_creates_directory_if_missing() {
    use wh_broker::deploy::memory::write_memory;

    let dir = setup_workspace();

    // Directory does not exist yet
    assert!(!dir.path().join(".wh/agents/researcher").exists());

    write_memory(
        dir.path(),
        "researcher",
        "First memory entry",
        "initial learning",
    )
    .unwrap();

    assert!(dir.path().join(".wh/agents/researcher/MEMORY.md").exists());
}

// =============================================================================
// AC #2: Updated MEMORY.md is available after restart (read_memory)
// =============================================================================

#[test]
fn read_memory_returns_none_for_nonexistent_agent() {
    use wh_broker::deploy::memory::read_memory;

    let dir = setup_workspace();
    let result = read_memory(dir.path(), "nonexistent").unwrap();
    assert!(
        result.is_none(),
        "read_memory should return None for non-existent agent"
    );
}

#[test]
fn read_memory_returns_content_after_write() {
    use wh_broker::deploy::memory::{read_memory, write_memory};

    let dir = setup_workspace();
    let content = "Important decision: scaled researcher from 1 to 2 replicas";
    write_memory(dir.path(), "donna", content, "topology decision").unwrap();

    let read_result = read_memory(dir.path(), "donna").unwrap();
    assert_eq!(
        read_result.as_deref(),
        Some(content),
        "read_memory should return the written content"
    );
}

#[test]
fn write_memory_overwrites_existing_content() {
    use wh_broker::deploy::memory::{read_memory, write_memory};

    let dir = setup_workspace();
    write_memory(dir.path(), "donna", "First entry", "first reason").unwrap();
    write_memory(dir.path(), "donna", "Second entry", "second reason").unwrap();

    let read_result = read_memory(dir.path(), "donna").unwrap();
    assert_eq!(
        read_result.as_deref(),
        Some("Second entry"),
        "write_memory should overwrite existing content"
    );
}

// =============================================================================
// AC #1 (continued): append_memory convenience function
// =============================================================================

#[test]
fn append_memory_adds_entry_with_separator() {
    use wh_broker::deploy::memory::{append_memory, read_memory, write_memory};

    let dir = setup_workspace();
    write_memory(dir.path(), "donna", "First entry", "first reason").unwrap();
    append_memory(dir.path(), "donna", "Second entry", "second reason").unwrap();

    let content = read_memory(dir.path(), "donna").unwrap().unwrap();
    assert!(
        content.contains("First entry"),
        "append should preserve first entry: {content}"
    );
    assert!(
        content.contains("Second entry"),
        "append should include new entry: {content}"
    );
    assert!(
        content.contains("---"),
        "append should include separator: {content}"
    );
}

#[test]
fn append_memory_writes_directly_when_no_existing_content() {
    use wh_broker::deploy::memory::{append_memory, read_memory};

    let dir = setup_workspace();
    append_memory(dir.path(), "researcher", "First entry ever", "initial").unwrap();

    let content = read_memory(dir.path(), "researcher").unwrap().unwrap();
    assert_eq!(content, "First entry ever");
}

// =============================================================================
// AC #3: Sequential writes both succeed (CM-04 compliance)
// =============================================================================

#[test]
fn sequential_writes_both_succeed_without_corruption() {
    use wh_broker::deploy::memory::{read_memory, write_memory};

    let dir = setup_workspace();

    // First write
    let result1 = write_memory(dir.path(), "donna", "Entry 1", "reason 1");
    assert!(result1.is_ok(), "first write should succeed");

    // Second write immediately after
    let result2 = write_memory(dir.path(), "donna", "Entry 2", "reason 2");
    assert!(result2.is_ok(), "second write should succeed");

    // Both commits should exist in git log
    let output = std::process::Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&output.stdout);
    let commit_count = log.lines().count();
    assert!(
        commit_count >= 3, // initial + 2 memory writes
        "should have at least 3 commits (initial + 2 writes), got {commit_count}: {log}"
    );

    // Latest content should be from the second write
    let content = read_memory(dir.path(), "donna").unwrap().unwrap();
    assert_eq!(content, "Entry 2");
}

#[test]
fn sequential_writes_to_different_agents_both_succeed() {
    use wh_broker::deploy::memory::write_memory;

    let dir = setup_workspace();

    let result1 = write_memory(dir.path(), "donna", "Donna's memory", "donna reason");
    assert!(result1.is_ok(), "donna write should succeed");

    let result2 = write_memory(
        dir.path(),
        "researcher",
        "Researcher's memory",
        "researcher reason",
    );
    assert!(result2.is_ok(), "researcher write should succeed");

    // Both files should exist
    assert!(dir.path().join(".wh/agents/donna/MEMORY.md").exists());
    assert!(dir.path().join(".wh/agents/researcher/MEMORY.md").exists());
}

// =============================================================================
// AC #1: Commit message format verification
// =============================================================================

#[test]
fn commit_message_follows_attribution_format() {
    use wh_broker::deploy::memory::write_memory;

    let dir = setup_workspace();
    write_memory(
        dir.path(),
        "donna",
        "Memory content here",
        "detected recurring pattern",
    )
    .unwrap();

    let msg = last_commit_message(dir.path());

    // First line: [agent_name] memory: reason
    let first_line = msg.lines().next().unwrap();
    assert!(
        first_line.starts_with("[donna] memory:"),
        "first line should start with '[donna] memory:': got '{first_line}'"
    );
    assert!(
        first_line.contains("detected recurring pattern"),
        "first line should contain reason: got '{first_line}'"
    );

    // Body should contain Timestamp and Agent fields
    assert!(
        msg.contains("Timestamp: "),
        "body should contain 'Timestamp: ': {msg}"
    );
    assert!(
        msg.contains("Agent: donna"),
        "body should contain 'Agent: donna': {msg}"
    );
}

// =============================================================================
// Error handling
// =============================================================================

#[test]
fn memory_error_codes_are_screaming_snake_case() {
    use wh_broker::deploy::memory::MemoryError;

    // Verify error code format
    let err = MemoryError::InvalidPath("test".to_string());
    let code = err.code();
    assert!(
        code.chars().all(|c| c.is_uppercase() || c == '_'),
        "error code should be SCREAMING_SNAKE_CASE: {code}"
    );
}
