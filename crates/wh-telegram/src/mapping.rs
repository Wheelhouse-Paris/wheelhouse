//! Chat ID <-> user_id mapping for Telegram response routing.
//!
//! Persisted as YAML at `.wh/telegram/chat_mappings.yaml` for git-versioned recovery (FR28).
//! Telegram chat_id is `i64` (signed) because group chats have negative IDs.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::error::TelegramError;

/// A single user_id <-> chat_id mapping entry.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct MappingEntry {
    user_id: String,
    chat_id: i64,
}

/// Serialized mapping file format.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct MappingFile {
    mappings: Vec<MappingEntry>,
}

/// Manages the bidirectional mapping between user_id and Telegram chat_id.
///
/// Uses an in-memory `HashMap` for fast lookups, backed by a YAML file for persistence.
pub struct ChatMapping {
    /// Path to the directory containing the mapping file.
    dir_path: PathBuf,
    /// In-memory forward mapping: user_id -> chat_id.
    user_to_chat: HashMap<String, i64>,
    /// In-memory reverse mapping: chat_id -> user_id.
    chat_to_user: HashMap<i64, String>,
}

impl ChatMapping {
    /// Creates a new ChatMapping, loading existing data from the YAML file if present.
    ///
    /// The mapping file is stored at `{dir_path}/chat_mappings.yaml`.
    #[instrument(skip_all)]
    pub fn new(dir_path: impl Into<PathBuf>) -> Result<Self, TelegramError> {
        let dir_path = dir_path.into();
        let mut mapping = Self {
            dir_path,
            user_to_chat: HashMap::new(),
            chat_to_user: HashMap::new(),
        };
        mapping.load()?;
        Ok(mapping)
    }

    /// Returns the file path for the mapping YAML.
    fn file_path(&self) -> PathBuf {
        self.dir_path.join("chat_mappings.yaml")
    }

    /// Loads existing mappings from the YAML file.
    fn load(&mut self) -> Result<(), TelegramError> {
        let path = self.file_path();
        if !path.exists() {
            return Ok(());
        }

        let contents = fs::read_to_string(&path)
            .map_err(|e| TelegramError::MappingError(e.to_string()))?;
        let file: MappingFile = serde_yaml::from_str(&contents)
            .map_err(|e| TelegramError::MappingError(e.to_string()))?;

        for entry in file.mappings {
            self.user_to_chat.insert(entry.user_id.clone(), entry.chat_id);
            self.chat_to_user.insert(entry.chat_id, entry.user_id);
        }

        Ok(())
    }

    /// Persists current mappings to the YAML file.
    fn save(&self) -> Result<(), TelegramError> {
        fs::create_dir_all(&self.dir_path)
            .map_err(|e| TelegramError::MappingError(e.to_string()))?;

        let entries: Vec<MappingEntry> = self
            .user_to_chat
            .iter()
            .map(|(user_id, chat_id)| MappingEntry {
                user_id: user_id.clone(),
                chat_id: *chat_id,
            })
            .collect();

        let file = MappingFile { mappings: entries };
        let yaml = serde_yaml::to_string(&file)
            .map_err(|e| TelegramError::MappingError(e.to_string()))?;

        fs::write(self.file_path(), yaml)
            .map_err(|e| TelegramError::MappingError(e.to_string()))?;

        Ok(())
    }

    /// Registers a user_id <-> chat_id mapping and persists to file.
    #[instrument(skip(self))]
    pub fn register(&mut self, user_id: &str, chat_id: i64) -> Result<(), TelegramError> {
        self.user_to_chat.insert(user_id.to_string(), chat_id);
        self.chat_to_user.insert(chat_id, user_id.to_string());
        self.save()
    }

    /// Looks up the Telegram chat_id for a given user_id.
    pub fn lookup_chat_id(&self, user_id: &str) -> Option<i64> {
        self.user_to_chat.get(user_id).copied()
    }

    /// Looks up the user_id for a given Telegram chat_id.
    pub fn lookup_user_id(&self, chat_id: i64) -> Option<&str> {
        self.chat_to_user.get(&chat_id).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let mut mapping = ChatMapping::new(dir.path().join("telegram")).unwrap();
        mapping.register("usr_abc123", 12345).unwrap();
        assert_eq!(mapping.lookup_chat_id("usr_abc123"), Some(12345));
        assert_eq!(mapping.lookup_user_id(12345), Some("usr_abc123"));
    }

    #[test]
    fn lookup_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let mapping = ChatMapping::new(dir.path().join("telegram")).unwrap();
        assert_eq!(mapping.lookup_chat_id("usr_nonexistent"), None);
    }

    #[test]
    fn persists_to_yaml_file() {
        let dir = tempfile::tempdir().unwrap();
        let mapping_path = dir.path().join("telegram");
        {
            let mut m = ChatMapping::new(mapping_path.clone()).unwrap();
            m.register("usr_abc123", 12345).unwrap();
            m.register("usr_def456", -987654).unwrap();
        }
        // Load from same path — should recover mappings
        let m2 = ChatMapping::new(mapping_path).unwrap();
        assert_eq!(m2.lookup_chat_id("usr_abc123"), Some(12345));
        assert_eq!(m2.lookup_chat_id("usr_def456"), Some(-987654));
    }

    #[test]
    fn handles_negative_chat_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut mapping = ChatMapping::new(dir.path().join("telegram")).unwrap();
        mapping.register("usr_group", -100123456789).unwrap();
        assert_eq!(mapping.lookup_chat_id("usr_group"), Some(-100123456789));
    }

    #[test]
    fn empty_dir_creates_new_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let mapping = ChatMapping::new(dir.path().join("telegram")).unwrap();
        assert_eq!(mapping.lookup_chat_id("anyone"), None);
    }
}
