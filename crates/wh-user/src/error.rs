//! Error types for the wh-user crate.

use thiserror::Error;

/// Errors that can occur during user profile operations.
#[derive(Error, Debug)]
pub enum UserError {
    /// I/O error during file operations.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// YAML serialization/deserialization error.
    #[error("serialization error: {0}")]
    SerializationError(String),

    /// Git repository not initialized at the expected path.
    #[error("user store is not initialized")]
    GitNotInitialized,

    /// Git command execution failed.
    #[error("version control operation failed")]
    GitCommandFailed(String),

    /// Invalid platform identifier.
    #[error("invalid platform identifier")]
    InvalidPlatform(String),

    /// Invalid display name.
    #[error("invalid display name")]
    InvalidDisplayName(String),

    /// Invalid platform user ID.
    #[error("invalid platform user identifier")]
    InvalidPlatformUserId(String),

    /// Field exceeds maximum length.
    #[error("field '{field}' exceeds maximum length of {max_len} characters")]
    FieldTooLong {
        /// The field name.
        field: String,
        /// The maximum allowed length.
        max_len: usize,
    },
}
