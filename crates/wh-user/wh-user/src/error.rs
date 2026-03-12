/// Error types for user profile operations.
///
/// Never expose git internals in user-facing error messages (RT-B1).
#[derive(Debug, thiserror::Error)]
pub enum UserError {
    /// I/O error during file operations.
    #[error("failed to access user profile: {0}")]
    IoError(#[from] std::io::Error),

    /// YAML serialization or deserialization error.
    #[error("user profile data is corrupted: {0}")]
    SerializationError(String),

    /// Git repository not initialized at the workspace path.
    #[error("workspace is not a git repository")]
    GitNotInitialized,

    /// Git command failed during profile commit.
    #[error("failed to save user profile: {0}")]
    GitCommandFailed(String),

    /// Platform string is invalid (must match `[a-z][a-z0-9-]*`).
    #[error("invalid platform: {0}")]
    InvalidPlatform(String),

    /// Display name is empty.
    #[error("invalid display name: {0}")]
    InvalidDisplayName(String),

    /// A field exceeds the maximum allowed length.
    #[error("{field} exceeds maximum length of {max_len} characters")]
    FieldTooLong {
        field: String,
        max_len: usize,
    },
}
