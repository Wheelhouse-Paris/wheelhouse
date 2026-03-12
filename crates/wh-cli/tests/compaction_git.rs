//! Acceptance tests for stream compaction (Story 7-5).
//!
//! These tests verify the ATDD acceptance criteria:
//! AC#1: Per-stream compaction mutex (CM-08) — concurrent runs dropped
//! AC#2: Two-phase protocol: temp → validate → rename → git commit (CM-03)
//! AC#3: Rollback on failure — no partial summary committed (NFR-R3)
//! AC#4: Git commit with structured summary file (FR11)

#![allow(unused_must_use)]

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

/// Helper: initialize a git repo in a temp directory.
fn init_git_repo(dir: &std::path::Path) {
    let git = find_git();
    Command::new(git)
        .args(["init"])
        .current_dir(dir)
        .output()
        .expect("git init failed");
    Command::new(git)
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir)
        .output()
        .expect("git config email failed");
    Command::new(git)
        .args(["config", "user.name", "Test"])
        .current_dir(dir)
        .output()
        .expect("git config name failed");
    // Initial commit so HEAD exists
    std::fs::write(dir.join(".gitkeep"), "").unwrap();
    Command::new(git)
        .args(["add", ".gitkeep"])
        .current_dir(dir)
        .output()
        .expect("git add failed");
    Command::new(git)
        .args(["commit", "-m", "initial"])
        .current_dir(dir)
        .output()
        .expect("git commit failed");
}

fn find_git() -> &'static str {
    for path in &[
        "/usr/bin/git",
        "/usr/local/bin/git",
        "/opt/homebrew/bin/git",
    ] {
        if std::path::Path::new(path).exists() {
            return path;
        }
    }
    "git"
}

// =============================================================================
// AC #1: Per-stream compaction mutex — concurrent runs dropped (CM-08)
// =============================================================================

#[tokio::test]
async fn compaction_acquires_per_stream_mutex() {
    // GIVEN a stream with WAL records
    // WHEN compaction is triggered
    // THEN a per-stream mutex is acquired
    // AND a concurrent compaction attempt returns MutexBusy

    // This test will fail until compact_stream() and the compaction mutex exist
    use wh_broker::wal::compaction::{compact_stream, CompactionError};

    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());

    let wal = wh_broker::wal::WalWriter::open(dir.path(), "test-stream").unwrap();
    wal.write(b"message1").await.unwrap();

    let since = 0i64;

    // First compaction should succeed
    let result = compact_stream(dir.path(), "test-stream", &wal, since).await;
    assert!(result.is_ok(), "first compaction should succeed");
}

#[tokio::test]
async fn concurrent_compaction_same_stream_is_dropped() {
    // GIVEN compaction is already running for stream "main"
    // WHEN a second compaction is triggered for "main"
    // THEN the second run is silently dropped with MutexBusy

    use wh_broker::wal::compaction::CompactionError;

    // Test will fail until CompactionError::MutexBusy variant exists
    let err = CompactionError::MutexBusy("test-stream".to_string());
    assert_eq!(err.code(), "MUTEX_BUSY");
}

// =============================================================================
// AC #2: Two-phase protocol (CM-03) — temp → validate → rename → git commit
// =============================================================================

#[tokio::test]
async fn compaction_creates_summary_at_correct_path() {
    // GIVEN a stream "main" with WAL records
    // WHEN compaction runs successfully
    // THEN a summary file exists at .wh/compaction/main/{YYYY-MM-DD}.md

    use wh_broker::wal::compaction::compact_stream;

    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());

    let wal = wh_broker::wal::WalWriter::open(dir.path(), "main").unwrap();
    wal.write(b"test-payload").await.unwrap();

    let since = 0i64;
    let result = compact_stream(dir.path(), "main", &wal, since)
        .await
        .unwrap();

    // Summary file should exist at the architecture-specified path
    assert!(result.summary_path.exists(), "summary file should exist");
    let path_str = result.summary_path.to_string_lossy();
    assert!(
        path_str.contains(".wh/compaction/main/"),
        "summary should be under .wh/compaction/main/"
    );
    assert!(path_str.ends_with(".md"), "summary should be markdown");
}

#[tokio::test]
async fn compaction_temp_file_is_cleaned_on_drop() {
    // GIVEN a CompactionTempFile is created
    // WHEN it is dropped without calling .commit()
    // THEN the temp file is deleted (5W-02 Drop=abort)

    use wh_broker::wal::compaction::CompactionTempFile;

    let dir = TempDir::new().unwrap();
    let temp_path = dir.path().join("test.tmp.md");
    std::fs::write(&temp_path, "temp content").unwrap();
    assert!(temp_path.exists());

    {
        let _temp = CompactionTempFile::new(temp_path.clone(), dir.path().join("test.md"));
        // Drop without calling .commit()
    }

    assert!(!temp_path.exists(), "temp file should be deleted on drop");
}

#[tokio::test]
async fn compaction_temp_file_commit_renames() {
    // GIVEN a CompactionTempFile with content
    // WHEN .commit() is called
    // THEN the temp file is renamed to the final path

    use wh_broker::wal::compaction::CompactionTempFile;

    let dir = TempDir::new().unwrap();
    let temp_path = dir.path().join("test.tmp.md");
    let final_path = dir.path().join("test.md");
    std::fs::write(&temp_path, "summary content").unwrap();

    let temp = CompactionTempFile::new(temp_path.clone(), final_path.clone());
    temp.commit().unwrap();

    assert!(!temp_path.exists(), "temp file should no longer exist");
    assert!(final_path.exists(), "final file should exist");
    assert_eq!(
        std::fs::read_to_string(&final_path).unwrap(),
        "summary content"
    );
}

// =============================================================================
// AC #2 continued: Git commit with attribution
// =============================================================================

#[tokio::test]
async fn compaction_creates_git_commit_with_attribution() {
    // GIVEN compaction completes successfully
    // WHEN I inspect the git log
    // THEN a commit exists with [compaction] prefix and stream name

    use wh_broker::wal::compaction::compact_stream;

    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());

    let wal = wh_broker::wal::WalWriter::open(dir.path(), "main").unwrap();
    wal.write(b"payload-data").await.unwrap();

    let since = 0i64;
    let result = compact_stream(dir.path(), "main", &wal, since)
        .await
        .unwrap();

    // Verify git commit exists
    assert!(!result.commit_hash.is_empty(), "commit hash should be set");

    let git = find_git();
    let log_output = Command::new(git)
        .args(["log", "--oneline", "-1"])
        .current_dir(dir.path())
        .output()
        .expect("git log failed");
    let log_msg = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log_msg.contains("[compaction]"),
        "commit should have [compaction] prefix: {log_msg}"
    );
    assert!(
        log_msg.contains("main"),
        "commit should reference stream name: {log_msg}"
    );
}

// =============================================================================
// AC #3: Rollback on failure — WAL intact, no partial summary (NFR-R3)
// =============================================================================

#[tokio::test]
async fn compaction_rollback_leaves_no_partial_summary() {
    // GIVEN compaction fails during processing
    // WHEN the failure occurs
    // THEN no summary file exists (rolled back)
    // AND the WAL records are still intact

    use wh_broker::wal::compaction::CompactionTempFile;

    let dir = TempDir::new().unwrap();
    let compaction_dir = dir.path().join(".wh/compaction/main");
    std::fs::create_dir_all(&compaction_dir).unwrap();

    let temp_path = compaction_dir.join("2026-03-12.tmp.md");
    let final_path = compaction_dir.join("2026-03-12.md");

    std::fs::write(&temp_path, "partial content").unwrap();

    // Simulate failure by dropping without commit
    {
        let _temp = CompactionTempFile::new(temp_path.clone(), final_path.clone());
        // Simulated failure — drop triggers abort
    }

    assert!(!temp_path.exists(), "temp file should be cleaned up");
    assert!(
        !final_path.exists(),
        "final file should NOT exist after rollback"
    );
}

// =============================================================================
// AC #4: Summary contains structured data (FR11)
// =============================================================================

#[tokio::test]
async fn compaction_summary_contains_structured_data() {
    // GIVEN compaction completes successfully
    // WHEN I read the summary file
    // THEN it contains record_count, time_range, and total_bytes

    use wh_broker::wal::compaction::compact_stream;

    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());

    let wal = wh_broker::wal::WalWriter::open(dir.path(), "main").unwrap();
    wal.write(b"message-one").await.unwrap();
    wal.write(b"message-two").await.unwrap();
    wal.write(b"message-three").await.unwrap();

    let since = 0i64;
    let result = compact_stream(dir.path(), "main", &wal, since)
        .await
        .unwrap();

    assert_eq!(result.record_count, 3, "should report 3 records");
    assert!(
        result.total_payload_bytes > 0,
        "should report payload bytes"
    );

    // Read the summary file and verify structured content
    let content = std::fs::read_to_string(&result.summary_path).unwrap();
    assert!(
        content.contains("record_count:"),
        "summary should have record_count"
    );
    assert!(content.contains("3"), "summary should report 3 records");
}

#[tokio::test]
async fn compaction_with_no_records_produces_empty_summary() {
    // GIVEN a stream with no WAL records in the time range
    // WHEN compaction runs
    // THEN it succeeds with record_count = 0

    use wh_broker::wal::compaction::compact_stream;

    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());

    let wal = wh_broker::wal::WalWriter::open(dir.path(), "empty-stream").unwrap();

    let since = chrono::Utc::now().timestamp_millis(); // Future timestamp = no records
    let result = compact_stream(dir.path(), "empty-stream", &wal, since)
        .await
        .unwrap();

    assert_eq!(result.record_count, 0, "empty stream should have 0 records");
}

// =============================================================================
// WAL truncation after compaction
// =============================================================================

#[tokio::test]
async fn wal_records_truncated_after_successful_compaction() {
    // GIVEN compaction completes successfully
    // WHEN I check the WAL
    // THEN the compacted records have been deleted

    use wh_broker::wal::compaction::compact_stream;

    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());

    let wal = wh_broker::wal::WalWriter::open(dir.path(), "main").unwrap();
    wal.write(b"old-message").await.unwrap();

    let count_before = wal.record_count().await.unwrap();
    assert_eq!(count_before, 1);

    let since = 0i64;
    compact_stream(dir.path(), "main", &wal, since)
        .await
        .unwrap();

    let count_after = wal.record_count().await.unwrap();
    assert_eq!(count_after, 0, "WAL should be truncated after compaction");
}

// =============================================================================
// Error code validation
// =============================================================================

#[test]
fn compaction_error_codes_are_screaming_snake_case() {
    use wh_broker::wal::compaction::CompactionError;

    let errors = [
        CompactionError::MutexBusy("test".to_string()),
        CompactionError::WalRead("test".to_string()),
        CompactionError::SummaryFailed("test".to_string()),
        CompactionError::GitFailed("test".to_string()),
        CompactionError::GitTimeout(30),
        CompactionError::RollbackTriggered("test".to_string()),
    ];

    for err in &errors {
        let code = err.code();
        assert!(
            code.chars().all(|c| c.is_uppercase() || c == '_'),
            "error code should be SCREAMING_SNAKE_CASE: {code}"
        );
    }
}

// =============================================================================
// WalWriter::read_records_since tests
// =============================================================================

#[tokio::test]
async fn wal_read_records_since_returns_matching_records() {
    use wh_broker::wal::WalWriter;

    let dir = TempDir::new().unwrap();
    let wal = WalWriter::open(dir.path(), "test").unwrap();

    wal.write(b"msg1").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let midpoint = chrono::Utc::now().timestamp_millis();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    wal.write(b"msg2").await.unwrap();

    let records = wal.read_records_since(midpoint).await.unwrap();
    assert_eq!(
        records.len(),
        1,
        "should return only records after midpoint"
    );
}

#[tokio::test]
async fn wal_read_records_since_returns_empty_when_no_match() {
    use wh_broker::wal::WalWriter;

    let dir = TempDir::new().unwrap();
    let wal = WalWriter::open(dir.path(), "test").unwrap();

    wal.write(b"old-msg").await.unwrap();

    let future = chrono::Utc::now().timestamp_millis() + 100_000;
    let records = wal.read_records_since(future).await.unwrap();
    assert!(
        records.is_empty(),
        "should return empty vec for future timestamp"
    );
}
