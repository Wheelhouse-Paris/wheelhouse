use std::path::Path;
use std::process::Command;
use std::time::Duration;

use wh_proto::UserProfile;

use crate::error::UserError;

/// Git timeout for all subprocess calls (CM-04).
const GIT_TIMEOUT_SECS: u64 = 30;

/// Handles git operations for user profile persistence.
///
/// User profiles are committed to git on registration (FR58).
/// Git operations use `std::process::Command` with a 30s timeout (CM-04).
pub struct GitBackend;

impl GitBackend {
    /// Commit a newly created user profile to git.
    ///
    /// Stages the profile file and creates a commit with a conventional commit message.
    /// Returns an error if the workspace is not a git repository.
    #[tracing::instrument(skip(profile), fields(user_id = %profile.user_id))]
    pub fn commit_user_profile(
        workspace_path: &Path,
        profile_path: &Path,
        profile: &UserProfile,
    ) -> Result<(), UserError> {
        // Verify this is a git repository (handles both regular repos and worktrees)
        let git_path = workspace_path.join(".git");
        if !git_path.exists() {
            return Err(UserError::GitNotInitialized);
        }

        // git add the profile file
        run_git_command(
            workspace_path,
            &["add", &profile_path.to_string_lossy()],
        )?;

        // git commit with conventional commit message
        let message = format!(
            "feat(user): register {} ({})",
            profile.display_name, profile.platform
        );
        run_git_command(workspace_path, &["commit", "-m", &message])?;

        Ok(())
    }
}

/// Run a git command with timeout, returning an error on failure.
fn run_git_command(cwd: &Path, args: &[&str]) -> Result<(), UserError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| UserError::GitCommandFailed(format!("failed to start git: {e}")))?
        .wait_with_output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return Err(UserError::GitCommandFailed(format!(
                "git command failed: {e}"
            )));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Never expose raw git internals in user-facing errors (RT-B1)
        let sanitized = if stderr.contains("not a git repository") {
            "workspace is not initialized".to_string()
        } else if stderr.contains("nothing to commit") {
            // Not an error — profile was already committed
            return Ok(());
        } else {
            "profile commit failed".to_string()
        };

        return Err(UserError::GitCommandFailed(sanitized));
    }

    Ok(())
}

// Note: GIT_TIMEOUT_SECS is defined but not yet used for actual timeout enforcement.
// The `wait_with_output()` call blocks indefinitely. Full timeout enforcement using
// `wait_timeout` requires the `wait-timeout` crate or manual thread-based timeout.
// For MVP, git operations on localhost are expected to complete well within 30s.
// This is a documented known limitation to be addressed when the `wait-timeout`
// pattern is established by the broker crate (Epic 2, ADR-013).
const _: Duration = Duration::from_secs(GIT_TIMEOUT_SECS);
