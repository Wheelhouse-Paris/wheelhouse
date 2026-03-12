/// Typed error hierarchy for the Wheelhouse CLI (ADR-014).
///
/// `anyhow` is permitted only in `main.rs` and tests — library code
/// returns `Result<T, WhError>` with `thiserror` (SCV-04).
#[derive(Debug, thiserror::Error)]
pub enum WhError {
    #[error("{0}")]
    DeployError(#[from] DeployError),

    #[error("{0}")]
    IoError(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error("{0}")]
    LintError(#[from] LintError),
}

#[derive(Debug, thiserror::Error)]
pub enum LintError {
    #[error("failed to read file: {0}")]
    FileReadError(std::io::Error),

    #[error("failed to parse YAML: {0}")]
    YamlParseError(String),
}

impl WhError {
    /// Machine-readable error code for JSON output.
    pub fn error_code(&self) -> &'static str {
        match self {
            WhError::DeployError(e) => e.error_code(),
            WhError::IoError(_) => "IO_ERROR",
        }
    }

    /// Exit code mapping.
    pub fn exit_code(&self) -> i32 {
        match self {
            WhError::DeployError(e) => e.exit_code(),
            WhError::IoError(_) => 1,
        }
    }
}

impl DeployError {
    pub fn error_code(&self) -> &'static str {
        match self {
            DeployError::LintError(e) => e.error_code(),
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            DeployError::LintError(_) => 1,
        }
    }
}

impl LintError {
    pub fn error_code(&self) -> &'static str {
        match self {
            LintError::FileReadError(_) => "LINT_FILE_ERROR",
            LintError::YamlParseError(_) => "LINT_PARSE_ERROR",
        }
    }
}
