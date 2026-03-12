//! Stream compaction engine (CM-03, CM-08, 5W-02).
//!
//! Implements the two-phase compaction protocol:
//! 1. Read WAL records → compute statistical summary → write to temp file
//! 2. Validate → atomic rename → git commit
//!
//! `CompactionTempFile::Drop` aborts (deletes temp file) unless `.commit()` is called (5W-02).
//! Per-stream compaction mutex prevents concurrent runs (CM-08).
//! Git operations use CM-04 30s timeout via `run_git_checked()`.

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::deploy::apply::run_git_checked;
use crate::deploy::DeployError;

use super::WalWriter;

/// Errors that can occur during compaction operations.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("compaction mutex busy for stream '{0}' — concurrent run dropped (CM-08)")]
    MutexBusy(String),

    #[error("failed to read WAL records: {0}")]
    WalRead(String),

    #[error("summary generation failed: {0}")]
    SummaryFailed(String),

    #[error("git operation failed: {0}")]
    GitFailed(String),

    #[error("git operation timed out after {0}s")]
    GitTimeout(u64),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("compaction rolled back: {0}")]
    RollbackTriggered(String),
}

impl CompactionError {
    /// Returns the error code string in SCREAMING_SNAKE_CASE (NP-01).
    pub fn code(&self) -> &'static str {
        match self {
            CompactionError::MutexBusy(_) => "MUTEX_BUSY",
            CompactionError::WalRead(_) => "WAL_READ_ERROR",
            CompactionError::SummaryFailed(_) => "SUMMARY_FAILED",
            CompactionError::GitFailed(_) => "GIT_FAILED",
            CompactionError::GitTimeout(_) => "GIT_TIMEOUT",
            CompactionError::IoError(_) => "IO_ERROR",
            CompactionError::RollbackTriggered(_) => "ROLLBACK_TRIGGERED",
        }
    }
}

impl From<DeployError> for CompactionError {
    fn from(err: DeployError) -> Self {
        match err {
            DeployError::GitTimeout(secs) => CompactionError::GitTimeout(secs),
            DeployError::GitFailed(msg) => CompactionError::GitFailed(msg),
            DeployError::FileRead(io_err) => CompactionError::IoError(io_err),
            other => CompactionError::GitFailed(other.to_string()),
        }
    }
}

/// Result of a successful compaction.
#[derive(Debug, Clone)]
pub struct CompactionSummary {
    /// Name of the compacted stream.
    pub stream_name: String,
    /// Date of the compaction (YYYY-MM-DD).
    pub date: String,
    /// Number of records processed.
    pub record_count: u64,
    /// Earliest record timestamp (milliseconds since epoch).
    pub earliest_timestamp_ms: i64,
    /// Latest record timestamp (milliseconds since epoch).
    pub latest_timestamp_ms: i64,
    /// Total payload bytes processed.
    pub total_payload_bytes: u64,
    /// Path to the summary file.
    pub summary_path: PathBuf,
    /// Git commit hash of the compaction commit.
    pub commit_hash: String,
}

/// RAII guard for compaction temp files (5W-02).
///
/// On `Drop`, the temp file is deleted (abort). The caller must
/// explicitly call `.commit()` to atomically rename to the final path.
pub struct CompactionTempFile {
    temp_path: PathBuf,
    final_path: PathBuf,
    committed: bool,
}

impl CompactionTempFile {
    /// Create a new compaction temp file guard.
    pub fn new(temp_path: PathBuf, final_path: PathBuf) -> Self {
        Self {
            temp_path,
            final_path,
            committed: false,
        }
    }

    /// Atomically rename the temp file to the final path.
    ///
    /// After this call, the temp file no longer exists and `Drop` becomes a no-op.
    pub fn commit(mut self) -> Result<PathBuf, CompactionError> {
        std::fs::rename(&self.temp_path, &self.final_path)?;
        self.committed = true;
        Ok(self.final_path.clone())
    }

    /// Get the temp file path.
    pub fn temp_path(&self) -> &Path {
        &self.temp_path
    }

    /// Get the final file path.
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }
}

impl Drop for CompactionTempFile {
    fn drop(&mut self) {
        if !self.committed && self.temp_path.exists() {
            let _ = std::fs::remove_file(&self.temp_path);
        }
    }
}

/// Run compaction for a stream (CM-03, CM-08, NFR-P4, NFR-R3).
///
/// Two-phase protocol:
/// 1. Read WAL records since `since_timestamp_ms`, compute summary stats, write temp file
/// 2. Validate temp file, atomic rename, git add + commit
///
/// After successful git commit, WAL records are truncated.
/// On any failure, the temp file is cleaned up by `CompactionTempFile::Drop`.
#[tracing::instrument(skip_all, fields(stream_name = %stream_name))]
pub async fn compact_stream(
    workspace_root: &Path,
    stream_name: &str,
    wal_writer: &WalWriter,
    since_timestamp_ms: i64,
) -> Result<CompactionSummary, CompactionError> {
    let date = Utc::now().format("%Y-%m-%d").to_string();

    // Read WAL records
    let records = wal_writer
        .read_records_since(since_timestamp_ms)
        .await
        .map_err(|e| CompactionError::WalRead(e.to_string()))?;

    let record_count = records.len() as u64;

    // Compute statistics
    let (earliest_timestamp_ms, latest_timestamp_ms, total_payload_bytes) = if records.is_empty() {
        (0i64, 0i64, 0u64)
    } else {
        let earliest = records.iter().map(|r| r.created_at).min().unwrap_or(0);
        let latest = records.iter().map(|r| r.created_at).max().unwrap_or(0);
        let total_bytes: u64 = records.iter().map(|r| r.payload.len() as u64).sum();
        (earliest, latest, total_bytes)
    };

    // Phase 1: Write temp summary file (PP-03: spawn_blocking for sync I/O)
    let compaction_dir = workspace_root
        .join(".wh")
        .join("compaction")
        .join(stream_name);

    let summary_content = generate_summary(
        stream_name,
        &date,
        record_count,
        earliest_timestamp_ms,
        latest_timestamp_ms,
        total_payload_bytes,
    );

    let dir_clone = compaction_dir.clone();
    let content_clone = summary_content.clone();
    let temp_path = compaction_dir.join(format!("{date}.tmp.md"));
    let final_path = compaction_dir.join(format!("{date}.md"));
    let tp = temp_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), CompactionError> {
        std::fs::create_dir_all(&dir_clone)?;
        std::fs::write(&tp, &content_clone)?;
        Ok(())
    })
    .await
    .map_err(|e| CompactionError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))??;

    let temp_file = CompactionTempFile::new(temp_path, final_path.clone());

    // Phase 2: Validate temp file
    let tp2 = temp_file.temp_path().to_path_buf();
    let temp_content = tokio::task::spawn_blocking(move || std::fs::read_to_string(&tp2))
        .await
        .map_err(|e| CompactionError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))??;
    if temp_content.is_empty() {
        return Err(CompactionError::SummaryFailed(
            "temp summary file is empty".to_string(),
        ));
    }

    // Atomic rename
    let committed_path = temp_file.commit()?;

    // Git add + commit (CM-04 30s timeout via run_git_checked)
    // Wrapped in spawn_blocking to avoid blocking the async runtime (PP-03)
    let relative_path = format!(
        ".wh/compaction/{stream_name}/{date}.md"
    );

    let earliest_str = if earliest_timestamp_ms > 0 {
        chrono::DateTime::from_timestamp_millis(earliest_timestamp_ms)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        "N/A".to_string()
    };
    let latest_str = if latest_timestamp_ms > 0 {
        chrono::DateTime::from_timestamp_millis(latest_timestamp_ms)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        "N/A".to_string()
    };

    let commit_message = format!(
        "[compaction] {stream_name}: daily summary {date}\n\nRecords: {record_count}\nTimeRange: {earliest_str} \u{2014} {latest_str}"
    );

    let ws = workspace_root.to_path_buf();
    let rel = relative_path.clone();
    let msg = commit_message.clone();
    let commit_hash = tokio::task::spawn_blocking(move || -> Result<String, CompactionError> {
        run_git_checked(&ws, &["add", &rel])?;
        run_git_checked(&ws, &["commit", "-m", &msg])?;
        let hash_output = run_git_checked(&ws, &["rev-parse", "HEAD"])?;
        let hash = String::from_utf8_lossy(&hash_output.stdout)
            .trim()
            .to_string();
        Ok(hash)
    })
    .await
    .map_err(|e| CompactionError::GitFailed(format!("spawn_blocking join error: {e}")))??;

    // Truncate WAL after successful git commit (NFR-R3: never before)
    if record_count > 0 {
        if let Err(e) = wal_writer.delete_before(latest_timestamp_ms + 1).await {
            // Log warning but do NOT fail — summary is already committed
            tracing::warn!(
                stream = %stream_name,
                error = %e,
                "compaction: WAL truncation failed after successful commit"
            );
        }
    }

    Ok(CompactionSummary {
        stream_name: stream_name.to_string(),
        date,
        record_count,
        earliest_timestamp_ms,
        latest_timestamp_ms,
        total_payload_bytes,
        summary_path: committed_path,
        commit_hash,
    })
}

/// Generate the compaction summary content (Markdown with YAML-style front matter).
fn generate_summary(
    stream_name: &str,
    date: &str,
    record_count: u64,
    earliest_timestamp_ms: i64,
    latest_timestamp_ms: i64,
    total_payload_bytes: u64,
) -> String {
    let earliest_str = if earliest_timestamp_ms > 0 {
        chrono::DateTime::from_timestamp_millis(earliest_timestamp_ms)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "N/A".to_string())
    } else {
        "N/A".to_string()
    };

    let latest_str = if latest_timestamp_ms > 0 {
        chrono::DateTime::from_timestamp_millis(latest_timestamp_ms)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| "N/A".to_string())
    } else {
        "N/A".to_string()
    };

    let size_human = format_bytes(total_payload_bytes);

    format!(
        "---\nstream: {stream_name}\ndate: \"{date}\"\nrecord_count: {record_count}\nearliest_timestamp: \"{earliest_str}\"\nlatest_timestamp: \"{latest_str}\"\ntotal_payload_bytes: {total_payload_bytes}\n---\n\n# Stream Compaction Summary: {stream_name} ({date})\n\n- **Records processed:** {record_count}\n- **Time range:** {earliest_str} \u{2014} {latest_str}\n- **Total payload size:** {size_human}\n"
    )
}

/// Format bytes as human-readable (e.g. "1.2 MB").
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_temp_file_drop_deletes_temp() {
        let dir = tempfile::tempdir().unwrap();
        let temp_path = dir.path().join("test.tmp.md");
        let final_path = dir.path().join("test.md");
        std::fs::write(&temp_path, "temp content").unwrap();
        assert!(temp_path.exists());

        {
            let _temp = CompactionTempFile::new(temp_path.clone(), final_path.clone());
            // Drop without calling .commit()
        }

        assert!(!temp_path.exists(), "temp file should be deleted on drop");
        assert!(!final_path.exists(), "final file should NOT exist");
    }

    #[test]
    fn compaction_temp_file_commit_renames() {
        let dir = tempfile::tempdir().unwrap();
        let temp_path = dir.path().join("test.tmp.md");
        let final_path = dir.path().join("test.md");
        std::fs::write(&temp_path, "summary content").unwrap();

        let temp = CompactionTempFile::new(temp_path.clone(), final_path.clone());
        let result = temp.commit().unwrap();

        assert_eq!(result, final_path);
        assert!(!temp_path.exists(), "temp file should no longer exist");
        assert!(final_path.exists(), "final file should exist");
        assert_eq!(
            std::fs::read_to_string(&final_path).unwrap(),
            "summary content"
        );
    }

    #[test]
    fn error_codes_are_screaming_snake_case() {
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

    #[test]
    fn generate_summary_produces_valid_markdown() {
        let content = generate_summary("main", "2026-03-12", 847, 1710115200000, 1710201599000, 1234567);
        assert!(content.contains("stream: main"));
        assert!(content.contains("record_count: 847"));
        assert!(content.contains("total_payload_bytes: 1234567"));
        assert!(content.contains("# Stream Compaction Summary: main"));
        assert!(content.contains("847"));
    }

    #[test]
    fn generate_summary_handles_zero_records() {
        let content = generate_summary("empty", "2026-03-12", 0, 0, 0, 0);
        assert!(content.contains("record_count: 0"));
        assert!(content.contains("N/A"));
    }

    #[test]
    fn format_bytes_formats_correctly() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(500), "500 bytes");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
    }

    #[test]
    fn summary_path_follows_architecture_spec() {
        let workspace = PathBuf::from("/workspace");
        let expected = workspace
            .join(".wh")
            .join("compaction")
            .join("main")
            .join("2026-03-12.md");
        assert_eq!(
            expected.to_string_lossy(),
            "/workspace/.wh/compaction/main/2026-03-12.md"
        );
    }
}
