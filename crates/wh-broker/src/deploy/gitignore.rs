//! Gitignore generation and secrets scanning for the `.wh/` directory.
//!
//! Ensures WAL files, secrets, and lock files are excluded from git.
//! Provides a pre-commit scan to detect accidentally staged secret files.
//!
//! See NFR-S2 (secrets exclusion) and FM-07 (`wh doctor` git health check).

use std::path::Path;

use crate::deploy::apply::run_git;
use crate::deploy::DeployError;

/// Patterns for secret files that must never be committed.
pub const SECRETS_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.token",
    "secrets/",
    "*.key",
    "*.pem",
    "*_secret*",
];

/// Patterns for WAL/database files (broker-owned ephemeral data).
pub const WAL_PATTERNS: &[&str] = &[
    "*.db",
    "*.db-wal",
    "*.db-shm",
];

/// Patterns for lock files (process-local, never committed).
pub const LOCK_PATTERNS: &[&str] = &[
    "workspace.lock",
];

/// All required gitignore patterns combined.
fn all_required_patterns() -> Vec<&'static str> {
    let mut patterns = Vec::new();
    patterns.extend_from_slice(SECRETS_PATTERNS);
    patterns.extend_from_slice(WAL_PATTERNS);
    patterns.extend_from_slice(LOCK_PATTERNS);
    patterns
}

/// Ensure `.wh/.gitignore` exists and contains all required exclusion patterns.
///
/// Creates the file if it does not exist. If it exists, appends any missing
/// required patterns (append-only — never removes user-added lines).
///
/// Returns `true` if the file was created or modified (needs staging).
pub fn ensure_gitignore(workspace_root: &Path) -> Result<bool, DeployError> {
    let wh_dir = workspace_root.join(".wh");
    std::fs::create_dir_all(&wh_dir).map_err(DeployError::FileRead)?;

    let gitignore_path = wh_dir.join(".gitignore");
    let required = all_required_patterns();

    let existing_content = if gitignore_path.exists() {
        std::fs::read_to_string(&gitignore_path).map_err(DeployError::FileRead)?
    } else {
        String::new()
    };

    let existing_lines: Vec<&str> = existing_content.lines().collect();

    // Find patterns not yet present in the file
    let missing: Vec<&&str> = required
        .iter()
        .filter(|pattern| !existing_lines.iter().any(|line| line.trim() == **pattern))
        .collect();

    if missing.is_empty() {
        return Ok(false); // No changes needed
    }

    // Build new content: existing content + header comment + missing patterns
    let mut new_content = existing_content.clone();
    if !new_content.is_empty() && !new_content.ends_with('\n') {
        new_content.push('\n');
    }

    // Only add header if there's no existing Wheelhouse section
    if !existing_content.contains("# Wheelhouse managed") {
        new_content.push_str("\n# Wheelhouse managed — do not remove these patterns\n");
    }

    for pattern in &missing {
        new_content.push_str(pattern);
        new_content.push('\n');
    }

    std::fs::write(&gitignore_path, &new_content).map_err(DeployError::FileRead)?;
    Ok(true)
}

/// Check if all required patterns are present in `.wh/.gitignore`.
///
/// Returns a list of missing patterns. Empty list means the gitignore is complete.
pub fn check_gitignore_completeness(workspace_root: &Path) -> Result<Vec<String>, DeployError> {
    let gitignore_path = workspace_root.join(".wh").join(".gitignore");

    if !gitignore_path.exists() {
        return Ok(all_required_patterns().iter().map(|s| s.to_string()).collect());
    }

    let content = std::fs::read_to_string(&gitignore_path).map_err(DeployError::FileRead)?;
    let lines: Vec<&str> = content.lines().map(|l| l.trim()).collect();

    let missing: Vec<String> = all_required_patterns()
        .iter()
        .filter(|p| !lines.contains(p))
        .map(|p| p.to_string())
        .collect();

    Ok(missing)
}

/// Scan staged files for potential secrets within the `.wh/` directory.
///
/// Runs `git diff --cached --name-only` and checks files under `.wh/` for
/// suspicious patterns. Only files within `.wh/` are scanned to avoid false
/// positives on code files (e.g., `keyboard.rs`, `token_parser.rs`).
///
/// Returns a list of suspicious file paths (empty if clean).
pub fn scan_staged_for_secrets(workspace_root: &Path) -> Result<Vec<String>, DeployError> {
    let output = run_git(workspace_root, &["diff", "--cached", "--name-only"])?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DeployError::GitFailed(format!(
            "git diff --cached failed: {stderr}"
        )));
    }

    let staged_files = String::from_utf8_lossy(&output.stdout);
    let suspicious: Vec<String> = staged_files
        .lines()
        .filter(|file| file.starts_with(".wh/"))
        .filter(|file| is_suspicious_filename(file))
        .map(|s| s.to_string())
        .collect();

    Ok(suspicious)
}

/// Check if a filename looks like it might contain secrets.
///
/// Matches files ending in `.token`, `.key`, `.pem`, `.env`, or containing
/// `secret` or `password` in the filename (case-insensitive).
fn is_suspicious_filename(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);

    // Extension-based checks
    if filename.ends_with(".token")
        || filename.ends_with(".key")
        || filename.ends_with(".pem")
        || filename.ends_with(".env")
        || filename == ".env"
    {
        return true;
    }

    // .env.* pattern
    if filename.starts_with(".env.") {
        return true;
    }

    // Content-suggesting names
    if filename.contains("secret") || filename.contains("password") {
        return true;
    }

    false
}

/// Scan git history (last 100 commits) for secrets accidentally committed
/// in the `.wh/` directory.
///
/// Uses `git log --all -p -S '<pattern>' -n 100 -- .wh/` for each pattern.
/// Returns a list of (pattern, commit_count) pairs where secrets were found.
pub fn scan_history_for_secrets(
    workspace_root: &Path,
) -> Result<Vec<(String, usize)>, DeployError> {
    let search_terms = ["API_KEY", "SECRET_KEY", "TOKEN=", "PASSWORD=", "sk-ant-"];
    let mut findings = Vec::new();

    for term in &search_terms {
        let output = run_git(
            workspace_root,
            &["log", "--all", "-p", "-S", term, "-n", "100", "--", ".wh/"],
        )?;

        if output.status.success() {
            let log_output = String::from_utf8_lossy(&output.stdout);
            if !log_output.trim().is_empty() {
                // Count number of commits that matched
                let commit_count = log_output.matches("commit ").count();
                if commit_count > 0 {
                    findings.push((term.to_string(), commit_count));
                }
            }
        }
        // Non-zero exit from git log is not an error — just means no matches
    }

    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_gitignore_creates_file_with_all_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");
        std::fs::create_dir_all(&wh_dir).unwrap();

        let modified = ensure_gitignore(dir.path()).unwrap();
        assert!(modified, "should report file was created");

        let content = std::fs::read_to_string(wh_dir.join(".gitignore")).unwrap();

        // Check all required patterns are present
        for pattern in all_required_patterns() {
            assert!(
                content.contains(pattern),
                "gitignore must contain pattern: {pattern}"
            );
        }
    }

    #[test]
    fn ensure_gitignore_preserves_existing_content() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");
        std::fs::create_dir_all(&wh_dir).unwrap();

        // Write existing content
        std::fs::write(wh_dir.join(".gitignore"), "# my rule\ncustom_dir/\n").unwrap();

        ensure_gitignore(dir.path()).unwrap();

        let content = std::fs::read_to_string(wh_dir.join(".gitignore")).unwrap();
        assert!(content.contains("custom_dir/"), "must preserve user patterns");
        assert!(content.contains("*.db"), "must also add required patterns");
    }

    #[test]
    fn ensure_gitignore_idempotent() {
        let dir = tempfile::tempdir().unwrap();

        // First call
        let modified1 = ensure_gitignore(dir.path()).unwrap();
        assert!(modified1, "first call should modify");

        // Second call — no changes needed
        let modified2 = ensure_gitignore(dir.path()).unwrap();
        assert!(!modified2, "second call should not modify (idempotent)");
    }

    #[test]
    fn check_gitignore_completeness_all_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = check_gitignore_completeness(dir.path()).unwrap();
        assert_eq!(missing.len(), all_required_patterns().len());
    }

    #[test]
    fn check_gitignore_completeness_all_present() {
        let dir = tempfile::tempdir().unwrap();
        ensure_gitignore(dir.path()).unwrap();
        let missing = check_gitignore_completeness(dir.path()).unwrap();
        assert!(missing.is_empty(), "all patterns should be present");
    }

    #[test]
    fn is_suspicious_detects_token_files() {
        assert!(is_suspicious_filename(".wh/api.token"));
        assert!(is_suspicious_filename(".wh/secrets/my_secret.key"));
        assert!(is_suspicious_filename(".wh/cert.pem"));
        assert!(is_suspicious_filename(".wh/.env"));
        assert!(is_suspicious_filename(".wh/.env.production"));
        assert!(is_suspicious_filename(".wh/db_password.txt"));
    }

    #[test]
    fn is_suspicious_allows_normal_files() {
        assert!(!is_suspicious_filename(".wh/state.json"));
        assert!(!is_suspicious_filename(".wh/.gitignore"));
        assert!(!is_suspicious_filename(".wh/agents/donna/SOUL.md"));
        assert!(!is_suspicious_filename(".wh/compaction/main/2026-03-12.md"));
    }

    #[test]
    fn all_required_patterns_covers_wal_secrets_lock() {
        let patterns = all_required_patterns();

        // WAL
        assert!(patterns.contains(&"*.db"));
        assert!(patterns.contains(&"*.db-wal"));
        assert!(patterns.contains(&"*.db-shm"));

        // Secrets
        assert!(patterns.contains(&".env"));
        assert!(patterns.contains(&"*.token"));
        assert!(patterns.contains(&"secrets/"));

        // Lock
        assert!(patterns.contains(&"workspace.lock"));
    }
}
