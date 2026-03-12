use thiserror::Error;

/// Errors that can occur when working with skills.
#[derive(Debug, Error)]
pub enum SkillError {
    /// The skill manifest file (`skill.md`) was not found at the expected path.
    #[error("skill manifest not found: {path}")]
    ManifestNotFound {
        /// The path that was searched.
        path: String,
    },

    /// The skill manifest is invalid (missing required fields, bad YAML, etc.).
    #[error("invalid skill manifest: {reason}")]
    InvalidManifest {
        /// Human-readable reason for the validation failure.
        reason: String,
    },

    /// A step file referenced in the manifest was not found.
    #[error("step file not found: {step}")]
    StepNotFound {
        /// The step file reference that could not be found.
        step: String,
    },

    /// The requested version (tag, branch, or commit) was not found in the git repository.
    #[error("version not found: {version}")]
    VersionNotFound {
        /// The version string that could not be resolved.
        version: String,
    },

    /// An error occurred while interacting with the git repository.
    #[error("git repository error: {0}")]
    RepositoryError(#[from] git2::Error),

    /// An I/O error occurred while reading skill files.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// The skill is not in the agent's allowlist (FM-05).
    #[error("skill '{skill_name}' is not permitted for agent '{agent_id}'")]
    SkillNotPermitted {
        /// The skill name that was not allowed.
        skill_name: String,
        /// The agent that attempted the invocation.
        agent_id: String,
    },

    /// The skill could not be fetched from the git repository.
    ///
    /// This covers both "repository unreachable" and "skill not found in repo at
    /// the resolved version" cases. Distinct from config-level issues which use
    /// `SKILL_LOAD_FAILED`.
    #[error("failed to fetch skill '{skill_name}' from git: {reason}")]
    SkillFetchFailed {
        /// The skill that could not be fetched.
        skill_name: String,
        /// Human-readable reason for the fetch failure.
        reason: String,
    },

    /// The skill execution timed out (Story 5-4).
    ///
    /// Emitted when the executor wall-clock timeout fires before the skill
    /// completes. Maps to error code `SKILL_TIMEOUT`.
    #[error("skill '{skill_name}' execution timed out after {timeout_secs}s")]
    SkillTimeout {
        /// The skill that timed out.
        skill_name: String,
        /// The timeout duration in seconds.
        timeout_secs: u64,
    },

    /// The skill execution failed due to an unhandled panic or exception (Story 5-4).
    ///
    /// Emitted when the executor panics during execution. Maps to error code
    /// `SKILL_EXECUTION_FAILED`.
    #[error("skill '{skill_name}' execution failed: {reason}")]
    SkillExecutionFailed {
        /// The skill that failed.
        skill_name: String,
        /// Human-readable reason for the execution failure.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_timeout_display_includes_name_and_duration() {
        let err = SkillError::SkillTimeout {
            skill_name: "web-search".into(),
            timeout_secs: 30,
        };
        let msg = err.to_string();
        assert!(msg.contains("web-search"), "should contain skill name");
        assert!(msg.contains("30"), "should contain timeout duration");
    }

    #[test]
    fn skill_execution_failed_display_includes_name_and_reason() {
        let err = SkillError::SkillExecutionFailed {
            skill_name: "summarize".into(),
            reason: "thread panicked".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("summarize"), "should contain skill name");
        assert!(msg.contains("thread panicked"), "should contain reason");
    }
}
