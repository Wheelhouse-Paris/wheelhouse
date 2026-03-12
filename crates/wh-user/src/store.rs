use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use wh_proto::UserProfile;

use crate::error::UserError;

/// Maximum length for platform strings.
const MAX_PLATFORM_LEN: usize = 64;
/// Maximum length for platform_user_id and display_name strings.
const MAX_FIELD_LEN: usize = 256;

/// Manages user profile storage as YAML files in `.wh/users/`.
///
/// Profiles are stored at `{base_path}/.wh/users/{user_id}.yaml`.
/// Each user gets a deterministic `user_id` derived from their platform identity.
pub struct UserStore {
    base_path: PathBuf,
}

impl UserStore {
    /// Create a new `UserStore` rooted at the given workspace path.
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Directory where user profile YAML files are stored.
    fn users_dir(&self) -> PathBuf {
        self.base_path.join(".wh").join("users")
    }

    /// Path to a specific user profile file.
    fn profile_path(&self, user_id: &str) -> PathBuf {
        self.users_dir().join(format!("{user_id}.yaml"))
    }

    /// Register a user profile on first interaction. Returns existing profile if already registered.
    ///
    /// Creates a YAML file at `.wh/users/{user_id}.yaml` with the profile data.
    /// Deduplication: if the file already exists, reads and returns the existing profile.
    ///
    /// # Arguments
    /// * `platform` — surface platform identifier (e.g., "cli", "telegram"); must match `[a-z][a-z0-9-]*`
    /// * `platform_user_id` — user identifier on the platform (e.g., username, telegram ID)
    /// * `display_name` — human-readable display name
    #[tracing::instrument(skip(self, platform_user_id, display_name), fields(platform))]
    pub fn register(
        &self,
        platform: &str,
        platform_user_id: &str,
        display_name: &str,
    ) -> Result<UserProfile, UserError> {
        // Validate inputs
        validate_platform(platform)?;
        validate_non_empty("platform_user_id", platform_user_id)?;
        validate_non_empty("display_name", display_name)?;
        validate_field_length("platform", platform, MAX_PLATFORM_LEN)?;
        validate_field_length("platform_user_id", platform_user_id, MAX_FIELD_LEN)?;
        validate_field_length("display_name", display_name, MAX_FIELD_LEN)?;

        let user_id = generate_user_id(platform, platform_user_id);
        let path = self.profile_path(&user_id);

        // Deduplication: if profile already exists, return it
        if path.exists() {
            return self.read_profile(&path);
        }

        // Create directory if it doesn't exist
        fs::create_dir_all(self.users_dir())?;

        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let profile = UserProfile {
            user_id,
            platform: platform.to_string(),
            display_name: display_name.to_string(),
            created_at: now,
        };

        // Atomic file creation: use create_new to prevent concurrent registration races
        let yaml = serde_yaml::to_string(&profile)
            .map_err(|e| UserError::SerializationError(e.to_string()))?;

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                use std::io::Write;
                let mut writer = std::io::BufWriter::new(file);
                writer.write_all(yaml.as_bytes())?;
                writer.flush()?;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // Another process registered the same user concurrently — read and return
                return self.read_profile(&path);
            }
            Err(e) => return Err(UserError::IoError(e)),
        }

        Ok(profile)
    }

    /// Look up a user profile by user_id.
    ///
    /// Returns `None` if the profile does not exist.
    #[tracing::instrument(skip(self))]
    pub fn lookup(&self, user_id: &str) -> Result<Option<UserProfile>, UserError> {
        let path = self.profile_path(user_id);
        if !path.exists() {
            return Ok(None);
        }
        self.read_profile(&path).map(Some)
    }

    /// Read and deserialize a profile from a YAML file.
    fn read_profile(&self, path: &Path) -> Result<UserProfile, UserError> {
        let content = fs::read_to_string(path)?;
        serde_yaml::from_str(&content).map_err(|e| UserError::SerializationError(e.to_string()))
    }

    /// Returns the base path of this store.
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }
}

/// Generate a deterministic user_id from platform identity.
///
/// Format: `usr_` prefix + first 16 hex chars of SHA-256(`{platform}:{platform_user_id}`).
pub fn generate_user_id(platform: &str, platform_user_id: &str) -> String {
    let input = format!("{platform}:{platform_user_id}");
    let hash = Sha256::digest(input.as_bytes());
    let hex = format!("{:x}", hash);
    format!("usr_{}", &hex[..16])
}

/// Validate that platform matches `[a-z][a-z0-9-]*`.
fn validate_platform(platform: &str) -> Result<(), UserError> {
    if platform.is_empty() {
        return Err(UserError::InvalidPlatform(
            "platform must not be empty".to_string(),
        ));
    }

    let bytes = platform.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return Err(UserError::InvalidPlatform(format!(
            "platform must start with a lowercase letter, got: {platform}"
        )));
    }

    for &b in &bytes[1..] {
        if !(b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err(UserError::InvalidPlatform(format!(
                "platform must match [a-z][a-z0-9-]*, got: {platform}"
            )));
        }
    }

    Ok(())
}

/// Validate that a field is not empty.
fn validate_non_empty(field_name: &str, value: &str) -> Result<(), UserError> {
    if value.is_empty() {
        if field_name == "display_name" {
            return Err(UserError::InvalidDisplayName(format!(
                "{field_name} must not be empty"
            )));
        }
        return Err(UserError::InvalidPlatform(format!(
            "{field_name} must not be empty"
        )));
    }
    Ok(())
}

/// Validate that a field does not exceed the maximum length.
fn validate_field_length(field_name: &str, value: &str, max_len: usize) -> Result<(), UserError> {
    if value.len() > max_len {
        return Err(UserError::FieldTooLong {
            field: field_name.to_string(),
            max_len,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_user_id_deterministic() {
        let id1 = generate_user_id("cli", "alice");
        let id2 = generate_user_id("cli", "alice");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("usr_"));
        assert_eq!(id1.len(), 4 + 16); // "usr_" + 16 hex chars
    }

    #[test]
    fn test_generate_user_id_different_inputs() {
        let id1 = generate_user_id("cli", "alice");
        let id2 = generate_user_id("cli", "bob");
        let id3 = generate_user_id("telegram", "alice");
        assert_ne!(id1, id2);
        assert_ne!(id1, id3);
        assert_ne!(id2, id3);
    }

    #[test]
    fn test_validate_platform_valid() {
        assert!(validate_platform("cli").is_ok());
        assert!(validate_platform("telegram").is_ok());
        assert!(validate_platform("my-surface").is_ok());
        assert!(validate_platform("a123").is_ok());
    }

    #[test]
    fn test_validate_platform_invalid() {
        assert!(validate_platform("").is_err());
        assert!(validate_platform("CLI").is_err());
        assert!(validate_platform("1abc").is_err());
        assert!(validate_platform("my_surface").is_err());
        assert!(validate_platform("My-Surface").is_err());
    }

    #[test]
    fn test_validate_non_empty() {
        assert!(validate_non_empty("field", "value").is_ok());
        assert!(validate_non_empty("field", "").is_err());
    }

    #[test]
    fn test_validate_field_length() {
        assert!(validate_field_length("field", "short", 10).is_ok());
        assert!(validate_field_length("field", "a".repeat(65).as_str(), 64).is_err());
    }
}
