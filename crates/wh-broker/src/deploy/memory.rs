//! Agent MEMORY.md persistence via git.
//!
//! Provides functions to read, write, and append to an agent's MEMORY.md file
//! within the `.wh/agents/{agent_name}/` persona directory, with each write
//! committed to git with agent attribution (FR62, ADR-003).
//!
//! The git subprocess calls use the same 30s timeout as the deploy pipeline (CM-04).

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::deploy::apply::run_git_checked;

/// Errors that can occur during memory operations.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("failed to read memory file: {0}")]
    FileRead(#[from] std::io::Error),

    #[error("git operation failed: {0}")]
    GitFailed(String),

    #[error("git operation timed out after {0}s")]
    GitTimeout(u64),

    #[error("invalid path or agent name: {0}")]
    InvalidPath(String),
}

impl MemoryError {
    /// Returns the error code string in SCREAMING_SNAKE_CASE (NP-01).
    pub fn code(&self) -> &'static str {
        match self {
            MemoryError::FileRead(_) => "FILE_READ_ERROR",
            MemoryError::GitFailed(_) => "GIT_FAILED",
            MemoryError::GitTimeout(_) => "GIT_TIMEOUT",
            MemoryError::InvalidPath(_) => "INVALID_PATH",
        }
    }
}

/// Convert deploy errors from git helpers into MemoryError.
impl From<crate::deploy::DeployError> for MemoryError {
    fn from(err: crate::deploy::DeployError) -> Self {
        match err {
            crate::deploy::DeployError::GitTimeout(secs) => MemoryError::GitTimeout(secs),
            crate::deploy::DeployError::GitFailed(msg) => MemoryError::GitFailed(msg),
            crate::deploy::DeployError::FileRead(io_err) => MemoryError::FileRead(io_err),
            other => MemoryError::GitFailed(other.to_string()),
        }
    }
}

/// The result of a successful memory write operation.
#[derive(Debug, Clone)]
pub struct MemoryUpdateResult {
    /// The git commit hash of the memory update.
    pub commit_hash: String,
    /// The file path of the MEMORY.md file.
    pub file_path: PathBuf,
    /// ISO-8601 timestamp of when the update was committed.
    pub timestamp: String,
}

/// Validate that an agent name is safe for use as a directory name.
///
/// Agent names must be non-empty and consist only of alphanumeric characters
/// and hyphens. Path traversal characters are rejected.
fn validate_agent_name(agent_name: &str) -> Result<(), MemoryError> {
    if agent_name.is_empty() {
        return Err(MemoryError::InvalidPath(
            "agent name must not be empty".to_string(),
        ));
    }

    if !agent_name.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return Err(MemoryError::InvalidPath(format!(
            "agent name '{}' contains invalid characters — only alphanumeric and hyphens allowed",
            agent_name
        )));
    }

    if agent_name.starts_with('-') || agent_name.ends_with('-') {
        return Err(MemoryError::InvalidPath(format!(
            "agent name '{}' must not start or end with a hyphen",
            agent_name
        )));
    }

    Ok(())
}

/// Resolve the MEMORY.md file path for an agent.
fn memory_path(workspace_root: &Path, agent_name: &str) -> PathBuf {
    workspace_root
        .join(".wh")
        .join("agents")
        .join(agent_name)
        .join("MEMORY.md")
}

/// Write (overwrite) an agent's MEMORY.md and commit to git.
///
/// Creates the `.wh/agents/{agent_name}/` directory if it does not exist.
/// The content overwrites any existing MEMORY.md — the caller is responsible
/// for merging with existing content if needed.
///
/// The git commit message follows ADR-003 attribution:
/// `[{agent_name}] memory: {reason}\n\nTimestamp: {ISO-8601}\nAgent: {agent_name}`
#[tracing::instrument(skip_all, fields(agent_name = %agent_name))]
pub fn write_memory(
    workspace_root: &Path,
    agent_name: &str,
    content: &str,
    reason: &str,
) -> Result<MemoryUpdateResult, MemoryError> {
    validate_agent_name(agent_name)?;

    let file_path = memory_path(workspace_root, agent_name);

    // Create directory structure if missing
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Write the content
    std::fs::write(&file_path, content)?;

    // Build the relative path for git add
    let relative_path = format!(".wh/agents/{}/MEMORY.md", agent_name);

    // Stage the file
    run_git_checked(workspace_root, &["add", &relative_path])?;

    // Generate timestamp
    let timestamp = Utc::now().to_rfc3339();

    // Commit with attribution (ADR-003 pattern)
    let commit_message = format!(
        "[{}] memory: {}\n\nTimestamp: {}\nAgent: {}",
        agent_name, reason, timestamp, agent_name
    );
    run_git_checked(workspace_root, &["commit", "-m", &commit_message])?;

    // Get the commit hash
    let hash_output = run_git_checked(workspace_root, &["rev-parse", "HEAD"])?;
    let commit_hash = String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string();

    Ok(MemoryUpdateResult {
        commit_hash,
        file_path,
        timestamp,
    })
}

/// Read an agent's MEMORY.md content.
///
/// Returns `Ok(None)` if the file does not exist (FR61: missing MEMORY.md
/// is treated as empty rather than an error).
#[tracing::instrument(skip_all, fields(agent_name = %agent_name))]
pub fn read_memory(workspace_root: &Path, agent_name: &str) -> Result<Option<String>, MemoryError> {
    validate_agent_name(agent_name)?;

    let file_path = memory_path(workspace_root, agent_name);

    if !file_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&file_path)?;
    Ok(Some(content))
}

/// Append an entry to an agent's MEMORY.md and commit to git.
///
/// Reads existing content, appends the new entry with a `---` separator,
/// and commits the result. If no existing content, writes the entry directly
/// without a leading separator.
#[tracing::instrument(skip_all, fields(agent_name = %agent_name))]
pub fn append_memory(
    workspace_root: &Path,
    agent_name: &str,
    entry: &str,
    reason: &str,
) -> Result<MemoryUpdateResult, MemoryError> {
    let existing = read_memory(workspace_root, agent_name)?;

    let new_content = match existing {
        Some(existing_content) if !existing_content.is_empty() => {
            format!("{}\n\n---\n\n{}", existing_content, entry)
        }
        _ => entry.to_string(),
    };

    write_memory(workspace_root, agent_name, &new_content, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_agent_name_rejects_empty() {
        let result = validate_agent_name("");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, MemoryError::InvalidPath(_)));
    }

    #[test]
    fn validate_agent_name_rejects_path_traversal() {
        assert!(validate_agent_name("../evil").is_err());
        assert!(validate_agent_name("foo/bar").is_err());
        assert!(validate_agent_name("foo\\bar").is_err());
        assert!(validate_agent_name("..").is_err());
    }

    #[test]
    fn validate_agent_name_accepts_valid_names() {
        assert!(validate_agent_name("donna").is_ok());
        assert!(validate_agent_name("researcher").is_ok());
        assert!(validate_agent_name("my-agent-1").is_ok());
        assert!(validate_agent_name("Agent2").is_ok());
    }

    #[test]
    fn validate_agent_name_rejects_spaces() {
        assert!(validate_agent_name("my agent").is_err());
    }

    #[test]
    fn validate_agent_name_rejects_leading_trailing_hyphens() {
        assert!(validate_agent_name("-leading").is_err());
        assert!(validate_agent_name("trailing-").is_err());
        assert!(validate_agent_name("-both-").is_err());
    }

    #[test]
    fn memory_path_is_correct() {
        let path = memory_path(Path::new("/workspace"), "donna");
        assert_eq!(path, PathBuf::from("/workspace/.wh/agents/donna/MEMORY.md"));
    }

    #[test]
    fn error_codes_are_screaming_snake_case() {
        let errors = [
            MemoryError::InvalidPath("test".to_string()),
            MemoryError::GitFailed("test".to_string()),
            MemoryError::GitTimeout(30),
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
    fn read_memory_returns_none_for_nonexistent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = read_memory(dir.path(), "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_memory_returns_content_for_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join(".wh/agents/donna");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("MEMORY.md"), "test content").unwrap();

        let result = read_memory(dir.path(), "donna").unwrap();
        assert_eq!(result.as_deref(), Some("test content"));
    }
}
