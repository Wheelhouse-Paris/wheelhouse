//! User profile storage and management.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tracing::instrument;

use wh_proto::UserProfile;

use crate::error::UserError;

/// Maximum length for platform identifier.
const MAX_PLATFORM_LEN: usize = 64;
/// Maximum length for platform user ID.
const MAX_PLATFORM_USER_ID_LEN: usize = 256;
/// Maximum length for display name.
const MAX_DISPLAY_NAME_LEN: usize = 256;

/// Manages user profile storage as YAML files in `.wh/users/`.
pub struct UserStore {
    base_path: PathBuf,
}

impl UserStore {
    /// Creates a new UserStore rooted at the given base path.
    ///
    /// User profiles will be stored at `{base_path}/.wh/users/{user_id}.yaml`.
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Returns the directory where user profiles are stored.
    fn users_dir(&self) -> PathBuf {
        self.base_path.join(".wh").join("users")
    }

    /// Returns the file path for a given user ID.
    fn profile_path(&self, user_id: &str) -> PathBuf {
        self.users_dir().join(format!("{}.yaml", user_id))
    }

    /// Registers a new user or returns existing profile if already registered.
    ///
    /// Deduplication: if a profile for the same platform + platform_user_id
    /// already exists, returns the existing profile without creating a duplicate.
    #[instrument(skip(self, platform_user_id, display_name), fields(platform = %platform))]
    pub fn register(
        &self,
        platform: &str,
        platform_user_id: &str,
        display_name: &str,
    ) -> Result<UserProfile, UserError> {
        // Validate inputs
        validate_platform(platform)?;
        validate_non_empty(platform_user_id, "platform_user_id")?;
        validate_non_empty(display_name, "display_name")?;
        validate_field_length(platform, "platform", MAX_PLATFORM_LEN)?;
        validate_field_length(platform_user_id, "platform_user_id", MAX_PLATFORM_USER_ID_LEN)?;
        validate_field_length(display_name, "display_name", MAX_DISPLAY_NAME_LEN)?;

        let user_id = generate_user_id(platform, platform_user_id);

        // Check for existing profile (deduplication)
        if let Some(existing) = self.lookup(&user_id)? {
            return Ok(existing);
        }

        // Create users directory if needed
        let users_dir = self.users_dir();
        fs::create_dir_all(&users_dir)?;

        let profile = UserProfile {
            user_id: user_id.clone(),
            platform: platform.to_string(),
            display_name: display_name.to_string(),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };

        let profile_path = self.profile_path(&user_id);

        // Atomic file creation for concurrent safety
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&profile_path)
        {
            Ok(mut file) => {
                let yaml = serde_yaml::to_string(&profile)
                    .map_err(|e| UserError::SerializationError(e.to_string()))?;
                file.write_all(yaml.as_bytes())?;
                Ok(profile)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another process created it first — read and return existing
                self.lookup(&user_id)?
                    .ok_or_else(|| UserError::IoError(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "profile file exists but cannot be read",
                    )))
            }
            Err(e) => Err(UserError::IoError(e)),
        }
    }

    /// Looks up a user profile by user_id.
    ///
    /// Returns `None` if the profile does not exist.
    #[instrument(skip(self, user_id))]
    pub fn lookup(&self, user_id: &str) -> Result<Option<UserProfile>, UserError> {
        let path = self.profile_path(user_id);
        if !path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(&path)?;
        let profile: UserProfile = serde_yaml::from_str(&contents)
            .map_err(|e| UserError::SerializationError(e.to_string()))?;
        Ok(Some(profile))
    }
}

/// Generates a deterministic user_id from platform and platform_user_id.
///
/// Format: `usr_` + first 16 hex chars of SHA-256("{platform}:{platform_user_id}")
pub fn generate_user_id(platform: &str, platform_user_id: &str) -> String {
    let input = format!("{}:{}", platform, platform_user_id);
    let hash = Sha256::digest(input.as_bytes());
    let hex = format!("{:x}", hash);
    format!("usr_{}", &hex[..16])
}

/// Validates platform format: must match `[a-z][a-z0-9-]*`.
fn validate_platform(platform: &str) -> Result<(), UserError> {
    if platform.is_empty() {
        return Err(UserError::InvalidPlatform("platform cannot be empty".into()));
    }
    let mut chars = platform.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(UserError::InvalidPlatform(
            "platform must start with lowercase letter".into(),
        ));
    }
    for ch in chars {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(UserError::InvalidPlatform(format!(
                "platform contains invalid character: '{}'",
                ch
            )));
        }
    }
    Ok(())
}

/// Validates a field is non-empty.
fn validate_non_empty(value: &str, field_name: &str) -> Result<(), UserError> {
    if value.is_empty() {
        return match field_name {
            "platform_user_id" => Err(UserError::InvalidPlatformUserId(
                "platform user ID cannot be empty".into(),
            )),
            "display_name" => Err(UserError::InvalidDisplayName(
                "display name cannot be empty".into(),
            )),
            _ => Err(UserError::InvalidPlatform(format!(
                "{} cannot be empty",
                field_name
            ))),
        };
    }
    Ok(())
}

/// Validates a field does not exceed the maximum length.
fn validate_field_length(value: &str, field: &str, max_len: usize) -> Result<(), UserError> {
    if value.len() > max_len {
        return Err(UserError::FieldTooLong {
            field: field.to_string(),
            max_len,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_user_id_is_deterministic() {
        let id1 = generate_user_id("telegram", "123456");
        let id2 = generate_user_id("telegram", "123456");
        assert_eq!(id1, id2);
    }

    #[test]
    fn generate_user_id_has_usr_prefix() {
        let id = generate_user_id("telegram", "123456");
        assert!(id.starts_with("usr_"));
        assert_eq!(id.len(), 4 + 16); // "usr_" + 16 hex chars
    }

    #[test]
    fn generate_user_id_different_for_different_inputs() {
        let id1 = generate_user_id("telegram", "123");
        let id2 = generate_user_id("telegram", "456");
        let id3 = generate_user_id("cli", "123");
        assert_ne!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn validate_platform_rejects_empty() {
        assert!(validate_platform("").is_err());
    }

    #[test]
    fn validate_platform_rejects_uppercase() {
        assert!(validate_platform("Telegram").is_err());
    }

    #[test]
    fn validate_platform_accepts_valid() {
        assert!(validate_platform("telegram").is_ok());
        assert!(validate_platform("cli").is_ok());
        assert!(validate_platform("my-surface-1").is_ok());
    }
}
