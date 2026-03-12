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
}
