//! Multi-chat routing table for Telegram surfaces.
//!
//! Maps `(chat_id, thread_id)` pairs to stream names (inbound routing)
//! and tracks `user_id -> (chat_id, thread_id)` for outbound routing.
//!
//! Built from the topology `chats` config + resolved state (`TelegramState`).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

/// A single entry in the flat routing table JSON file.
#[derive(Debug, Deserialize)]
struct RoutingEntry {
    chat_id: i64,
    thread_id: Option<i32>,
    stream: String,
    /// Human-readable topic name (e.g., "Iktos", "General").
    /// Absent in pre-10.2 routing files; defaults to None.
    #[serde(default)]
    topic_name: Option<String>,
}

/// Routing table for multi-chat Telegram surfaces.
///
/// Supports two routing modes:
/// - **Single-stream** (legacy): all messages go to one stream.
/// - **Multi-chat**: routes based on `(chat_id, Option<thread_id>)` pairs.
#[derive(Debug, Clone)]
pub struct RoutingTable {
    /// Mode of operation.
    mode: RoutingMode,

    /// Inbound routing: `(chat_id, Option<thread_id>)` -> stream name.
    inbound: HashMap<(i64, Option<i32>), String>,

    /// Topic names: `(chat_id, Option<thread_id>)` -> human-readable topic name.
    /// Only populated for forum-topic routes in multi-chat mode.
    topic_names: HashMap<(i64, Option<i32>), String>,

    /// Outbound routing: `user_id` -> `(chat_id, Option<thread_id>)`.
    /// Updated from last-seen inbound messages.
    outbound: HashMap<String, (i64, Option<i32>)>,
}

/// The routing mode for a Telegram surface.
#[derive(Debug, Clone)]
enum RoutingMode {
    /// Legacy single-stream mode: all messages go to one stream.
    SingleStream(String),
    /// Multi-chat mode: routes based on chat/thread pairs.
    MultiChat,
}

impl RoutingTable {
    /// Creates a routing table for single-stream mode (backward compatibility).
    pub fn single_stream(stream_name: &str) -> Self {
        Self {
            mode: RoutingMode::SingleStream(stream_name.to_string()),
            inbound: HashMap::new(),
            topic_names: HashMap::new(),
            outbound: HashMap::new(),
        }
    }

    /// Creates a routing table for multi-chat mode.
    ///
    /// Call `add_route` for each resolved chat/thread -> stream mapping.
    pub fn multi_chat() -> Self {
        Self {
            mode: RoutingMode::MultiChat,
            inbound: HashMap::new(),
            topic_names: HashMap::new(),
            outbound: HashMap::new(),
        }
    }

    /// Adds an inbound route: `(chat_id, thread_id)` -> `stream_name`.
    pub fn add_route(&mut self, chat_id: i64, thread_id: Option<i32>, stream_name: &str) {
        self.inbound
            .insert((chat_id, thread_id), stream_name.to_string());
    }

    /// Adds an inbound route with an associated topic name.
    pub fn add_route_with_topic(
        &mut self,
        chat_id: i64,
        thread_id: Option<i32>,
        stream_name: &str,
        topic_name: &str,
    ) {
        self.inbound
            .insert((chat_id, thread_id), stream_name.to_string());
        self.topic_names
            .insert((chat_id, thread_id), topic_name.to_string());
    }

    /// Resolves the target stream for an inbound message.
    ///
    /// In single-stream mode, always returns the configured stream.
    /// In multi-chat mode, looks up `(chat_id, thread_id)` in the routing table.
    /// Returns `None` if no mapping exists (message should be silently ignored).
    pub fn resolve_inbound(&self, chat_id: i64, thread_id: Option<i32>) -> Option<&str> {
        match &self.mode {
            RoutingMode::SingleStream(stream) => Some(stream.as_str()),
            RoutingMode::MultiChat => self.inbound.get(&(chat_id, thread_id)).map(|s| s.as_str()),
        }
    }

    /// Resolves the target stream and topic name for an inbound message.
    ///
    /// Returns `(stream_name, Option<topic_name>)`.
    /// In single-stream mode, returns the configured stream with no topic.
    /// In multi-chat mode, looks up both the stream and topic name.
    pub fn resolve_inbound_with_topic(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
    ) -> Option<(&str, Option<&str>)> {
        match &self.mode {
            RoutingMode::SingleStream(stream) => Some((stream.as_str(), None)),
            RoutingMode::MultiChat => {
                let stream = self.inbound.get(&(chat_id, thread_id))?;
                let topic = self
                    .topic_names
                    .get(&(chat_id, thread_id))
                    .map(|s| s.as_str());
                Some((stream.as_str(), topic))
            }
        }
    }

    /// Records a user's last-seen location for outbound routing.
    ///
    /// Called on every inbound message to track where to send replies.
    pub fn record_user_location(&mut self, user_id: &str, chat_id: i64, thread_id: Option<i32>) {
        self.outbound
            .insert(user_id.to_string(), (chat_id, thread_id));
    }

    /// Resolves the outbound target for a reply to a user.
    ///
    /// Returns `(chat_id, Option<thread_id>)` from the user's last-seen location.
    pub fn resolve_outbound(&self, user_id: &str) -> Option<(i64, Option<i32>)> {
        self.outbound.get(user_id).copied()
    }

    /// Builds a multi-chat routing table from the JSON file written by `resolve_telegram_surfaces`.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read routing file {}: {e}", path.display()))?;
        let entries: Vec<RoutingEntry> = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse routing file: {e}"))?;
        let mut table = Self::multi_chat();
        for entry in entries {
            if let Some(ref topic) = entry.topic_name {
                table.add_route_with_topic(entry.chat_id, entry.thread_id, &entry.stream, topic);
            } else {
                table.add_route(entry.chat_id, entry.thread_id, &entry.stream);
            }
        }
        Ok(table)
    }

    /// Returns all unique stream names in the routing table.
    pub fn all_stream_names(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.inbound
            .values()
            .filter(|s| seen.insert((*s).clone()))
            .cloned()
            .collect()
    }

    /// Returns whether this is operating in single-stream mode.
    pub fn is_single_stream(&self) -> bool {
        matches!(self.mode, RoutingMode::SingleStream(_))
    }

    /// Returns the number of inbound routes configured.
    pub fn route_count(&self) -> usize {
        self.inbound.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_stream_always_resolves() {
        let table = RoutingTable::single_stream("main");
        assert_eq!(table.resolve_inbound(12345, None), Some("main"));
        assert_eq!(table.resolve_inbound(-100999, Some(42)), Some("main"));
        assert!(table.is_single_stream());
    }

    #[test]
    fn multi_chat_routes_correctly() {
        let mut table = RoutingTable::multi_chat();
        table.add_route(12345, None, "direct");
        table.add_route(-100999, Some(42), "general");
        table.add_route(-100999, Some(99), "project-x");

        assert_eq!(table.resolve_inbound(12345, None), Some("direct"));
        assert_eq!(table.resolve_inbound(-100999, Some(42)), Some("general"));
        assert_eq!(table.resolve_inbound(-100999, Some(99)), Some("project-x"));
        // Unmapped chat/thread: silently ignored
        assert_eq!(table.resolve_inbound(99999, None), None);
        assert_eq!(table.resolve_inbound(-100999, Some(1)), None);
        assert!(!table.is_single_stream());
    }

    #[test]
    fn outbound_user_location_tracking() {
        let mut table = RoutingTable::multi_chat();
        table.record_user_location("usr_abc", -100999, Some(42));
        assert_eq!(table.resolve_outbound("usr_abc"), Some((-100999, Some(42))));
        assert_eq!(table.resolve_outbound("usr_unknown"), None);

        // Update location
        table.record_user_location("usr_abc", 12345, None);
        assert_eq!(table.resolve_outbound("usr_abc"), Some((12345, None)));
    }

    #[test]
    fn route_count() {
        let mut table = RoutingTable::multi_chat();
        assert_eq!(table.route_count(), 0);
        table.add_route(1, None, "s1");
        table.add_route(2, Some(10), "s2");
        assert_eq!(table.route_count(), 2);
    }

    // ── Story 10.2: source_stream context tests ──

    #[test]
    fn resolve_inbound_with_topic_single_stream() {
        let table = RoutingTable::single_stream("main");
        let result = table.resolve_inbound_with_topic(12345, None);
        assert_eq!(result, Some(("main", None)));
    }

    #[test]
    fn resolve_inbound_with_topic_multi_chat() {
        let mut table = RoutingTable::multi_chat();
        table.add_route_with_topic(-100999, Some(42), "iktos", "Iktos");
        table.add_route(-100999, Some(99), "general");

        // Route with topic
        let result = table.resolve_inbound_with_topic(-100999, Some(42));
        assert_eq!(result, Some(("iktos", Some("Iktos"))));

        // Route without topic
        let result = table.resolve_inbound_with_topic(-100999, Some(99));
        assert_eq!(result, Some(("general", None)));

        // Unmapped route
        let result = table.resolve_inbound_with_topic(99999, None);
        assert_eq!(result, None);
    }

    #[test]
    fn from_file_with_topic_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routing.json");
        let json = r#"[
            {"chat_id": -100999, "thread_id": 42, "stream": "iktos", "topic_name": "Iktos"},
            {"chat_id": -100999, "thread_id": 99, "stream": "general"}
        ]"#;
        std::fs::write(&path, json).unwrap();

        let table = RoutingTable::from_file(&path).unwrap();
        assert_eq!(
            table.resolve_inbound_with_topic(-100999, Some(42)),
            Some(("iktos", Some("Iktos")))
        );
        assert_eq!(
            table.resolve_inbound_with_topic(-100999, Some(99)),
            Some(("general", None))
        );
    }

    #[test]
    fn from_file_backward_compat_no_topic_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routing.json");
        // Pre-10.2 JSON without topic_name field
        let json = r#"[{"chat_id": -100999, "thread_id": 42, "stream": "iktos"}]"#;
        std::fs::write(&path, json).unwrap();

        let table = RoutingTable::from_file(&path).unwrap();
        assert_eq!(
            table.resolve_inbound_with_topic(-100999, Some(42)),
            Some(("iktos", None))
        );
    }
}
