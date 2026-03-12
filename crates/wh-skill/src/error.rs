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
}
