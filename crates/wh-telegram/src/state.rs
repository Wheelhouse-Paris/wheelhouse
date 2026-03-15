//! Telegram deploy state persistence.
//!
//! Stores runtime-resolved IDs for groups and topics:
//! - `groups`: group display name -> chat_id (resolved from `my_chat_member` updates)
//! - `topics`: (chat_id, topic_name) -> thread_id (resolved from `createForumTopic`)
//!
//! Persisted to `.wh/telegram-state.json` for recovery across restarts (AC-8).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::error::TelegramError;

/// Serialized topic key: `"chat_id:topic_name"`.
///
/// We serialize `(i64, String)` as a single string key because JSON object keys must be strings.
fn topic_key(chat_id: i64, topic_name: &str) -> String {
    format!("{chat_id}:{topic_name}")
}

/// Parse a topic key back into `(chat_id, topic_name)`.
fn parse_topic_key(key: &str) -> Option<(i64, String)> {
    let colon_pos = key.find(':')?;
    let chat_id: i64 = key[..colon_pos].parse().ok()?;
    let topic_name = key[colon_pos + 1..].to_string();
    Some((chat_id, topic_name))
}

/// Serialization format for `.wh/telegram-state.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StateFile {
    /// Group display name -> Telegram chat_id.
    #[serde(default)]
    groups: HashMap<String, i64>,

    /// Serialized `"chat_id:topic_name"` -> thread_id.
    #[serde(default)]
    topics: HashMap<String, i32>,
}

/// Runtime state for Telegram group and topic ID resolution.
///
/// Loaded from and persisted to `.wh/telegram-state.json`.
#[derive(Debug, Clone)]
pub struct TelegramState {
    /// Directory containing the state file.
    state_dir: PathBuf,

    /// Group display name -> chat_id.
    groups: HashMap<String, i64>,

    /// (chat_id, topic_name) -> thread_id.
    topics: HashMap<(i64, String), i32>,
}

impl TelegramState {
    /// Loads state from `.wh/telegram-state.json` in the given directory.
    ///
    /// If the file does not exist, returns an empty state.
    #[instrument(skip_all, fields(dir = %state_dir.as_ref().display()))]
    pub fn load(state_dir: impl AsRef<Path>) -> Result<Self, TelegramError> {
        let state_dir = state_dir.as_ref().to_path_buf();
        let file_path = state_dir.join("telegram-state.json");

        let (groups, topics) = if file_path.exists() {
            let content = std::fs::read_to_string(&file_path).map_err(|e| {
                TelegramError::StateError(format!("failed to read state file: {e}"))
            })?;
            let state_file: StateFile = serde_json::from_str(&content).map_err(|e| {
                TelegramError::StateError(format!("failed to parse state file: {e}"))
            })?;

            let mut topics = HashMap::new();
            for (key, thread_id) in state_file.topics {
                if let Some((chat_id, topic_name)) = parse_topic_key(&key) {
                    topics.insert((chat_id, topic_name), thread_id);
                }
            }

            (state_file.groups, topics)
        } else {
            (HashMap::new(), HashMap::new())
        };

        Ok(Self {
            state_dir,
            groups,
            topics,
        })
    }

    /// Persists current state to `.wh/telegram-state.json`.
    #[instrument(skip(self))]
    pub fn save(&self) -> Result<(), TelegramError> {
        std::fs::create_dir_all(&self.state_dir)
            .map_err(|e| TelegramError::StateError(format!("failed to create state dir: {e}")))?;

        let topics: HashMap<String, i32> = self
            .topics
            .iter()
            .map(|((chat_id, name), thread_id)| (topic_key(*chat_id, name), *thread_id))
            .collect();

        let state_file = StateFile {
            groups: self.groups.clone(),
            topics,
        };

        let json = serde_json::to_string_pretty(&state_file)
            .map_err(|e| TelegramError::StateError(format!("failed to serialize state: {e}")))?;

        let file_path = self.state_dir.join("telegram-state.json");
        std::fs::write(&file_path, json)
            .map_err(|e| TelegramError::StateError(format!("failed to write state file: {e}")))?;

        Ok(())
    }

    /// Registers a group name -> chat_id mapping (from `my_chat_member` update).
    pub fn register_group(&mut self, group_name: &str, chat_id: i64) {
        self.groups.insert(group_name.to_string(), chat_id);
    }

    /// Looks up the chat_id for a group by display name.
    pub fn lookup_group(&self, group_name: &str) -> Option<i64> {
        self.groups.get(group_name).copied()
    }

    /// Registers a topic thread_id for a (chat_id, topic_name) pair.
    pub fn register_topic(&mut self, chat_id: i64, topic_name: &str, thread_id: i32) {
        self.topics
            .insert((chat_id, topic_name.to_string()), thread_id);
    }

    /// Looks up the thread_id for a (chat_id, topic_name) pair.
    pub fn lookup_topic(&self, chat_id: i64, topic_name: &str) -> Option<i32> {
        self.topics.get(&(chat_id, topic_name.to_string())).copied()
    }

    /// Returns a reference to all groups.
    pub fn groups(&self) -> &HashMap<String, i64> {
        &self.groups
    }

    /// Returns a reference to all topics.
    pub fn topics(&self) -> &HashMap<(i64, String), i32> {
        &self.topics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = TelegramState::load(dir.path().join(".wh")).unwrap();
        assert!(state.groups.is_empty());
        assert!(state.topics.is_empty());
    }

    #[test]
    fn register_group_and_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = TelegramState::load(dir.path().join(".wh")).unwrap();
        state.register_group("Wheelhouse Ops", -100123456789);
        assert_eq!(state.lookup_group("Wheelhouse Ops"), Some(-100123456789));
        assert_eq!(state.lookup_group("Nonexistent"), None);
    }

    #[test]
    fn register_topic_and_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = TelegramState::load(dir.path().join(".wh")).unwrap();
        state.register_topic(-100123, "General", 42);
        assert_eq!(state.lookup_topic(-100123, "General"), Some(42));
        assert_eq!(state.lookup_topic(-100123, "Other"), None);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");

        {
            let mut state = TelegramState::load(&wh_dir).unwrap();
            state.register_group("Wheelhouse Ops", -100123456789);
            state.register_topic(-100123456789, "General", 42);
            state.register_topic(-100123456789, "Project X", 99);
            state.save().unwrap();
        }

        // Load from same path
        let state2 = TelegramState::load(&wh_dir).unwrap();
        assert_eq!(state2.lookup_group("Wheelhouse Ops"), Some(-100123456789));
        assert_eq!(state2.lookup_topic(-100123456789, "General"), Some(42));
        assert_eq!(state2.lookup_topic(-100123456789, "Project X"), Some(99));
    }

    #[test]
    fn topic_key_roundtrip() {
        let key = topic_key(-100123, "General Discussion");
        let (chat_id, name) = parse_topic_key(&key).unwrap();
        assert_eq!(chat_id, -100123);
        assert_eq!(name, "General Discussion");
    }

    #[test]
    fn topic_key_with_colon_in_name() {
        // Topic name could contain colons — only the first colon is the delimiter
        let key = topic_key(12345, "Topic: Important");
        let (chat_id, name) = parse_topic_key(&key).unwrap();
        assert_eq!(chat_id, 12345);
        assert_eq!(name, "Topic: Important");
    }
}
