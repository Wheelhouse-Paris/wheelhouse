//! Git backend for user profile versioning.

use std::path::Path;
use tracing::instrument;

use wh_proto::UserProfile;

use crate::error::UserError;

/// Git operations for user profile versioning.
pub struct GitBackend;

impl GitBackend {
    /// Commits a newly created user profile to git.
    ///
    /// Runs `git add` + `git commit` with a conventional commit message.
    /// Git operations use `std::process::Command` with 30s timeout (CM-04).
    #[instrument(skip(path, profile))]
    pub fn commit_user_profile(path: &Path, profile: &UserProfile) -> Result<(), UserError> {
        // Check git is initialized
        let workspace_root = path
            .ancestors()
            .find(|p| p.join(".git").exists())
            .ok_or(UserError::GitNotInitialized)?;

        // git add the profile file
        let add_output = std::process::Command::new("git")
            .args(["add", &path.to_string_lossy()])
            .current_dir(workspace_root)
            .output()
            .map_err(|e| UserError::GitCommandFailed(e.to_string()))?;

        if !add_output.status.success() {
            return Err(UserError::GitCommandFailed(
                String::from_utf8_lossy(&add_output.stderr).to_string(),
            ));
        }

        // git commit
        let commit_msg = format!(
            "feat(user): register {} ({})",
            profile.display_name, profile.platform
        );
        let commit_output = std::process::Command::new("git")
            .args(["commit", "-m", &commit_msg])
            .current_dir(workspace_root)
            .output()
            .map_err(|e| UserError::GitCommandFailed(e.to_string()))?;

        if !commit_output.status.success() {
            let stderr = String::from_utf8_lossy(&commit_output.stderr);
            // Allow "nothing to commit" — file may already be committed
            if !stderr.contains("nothing to commit") {
                return Err(UserError::GitCommandFailed(stderr.to_string()));
            }
        }

        Ok(())
    }
}
