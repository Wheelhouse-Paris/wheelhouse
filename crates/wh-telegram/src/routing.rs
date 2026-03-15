//! Multi-chat routing table for Telegram surfaces.
//!
//! Maps `(chat_id, thread_id)` pairs to stream names (inbound routing)
//! and tracks `user_id -> (chat_id, thread_id)` for outbound routing.
//!
//! Built from the topology `chats` config + resolved state (`TelegramState`).

use std::collections::HashMap;

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
            outbound: HashMap::new(),
        }
    }

    /// Adds an inbound route: `(chat_id, thread_id)` -> `stream_name`.
    pub fn add_route(&mut self, chat_id: i64, thread_id: Option<i32>, stream_name: &str) {
        self.inbound
            .insert((chat_id, thread_id), stream_name.to_string());
    }

    /// Resolves the target stream for an inbound message.
    ///
    /// In single-stream mode, always returns the configured stream.
    /// In multi-chat mode, looks up `(chat_id, thread_id)` in the routing table.
    /// Returns `None` if no mapping exists (message should be silently ignored).
    pub fn resolve_inbound(&self, chat_id: i64, thread_id: Option<i32>) -> Option<&str> {
        match &self.mode {
            RoutingMode::SingleStream(stream) => Some(stream.as_str()),
            RoutingMode::MultiChat => self
                .inbound
                .get(&(chat_id, thread_id))
                .map(|s| s.as_str()),
        }
    }

    /// Records a user's last-seen location for outbound routing.
    ///
    /// Called on every inbound message to track where to send replies.
    pub fn record_user_location(
        &mut self,
        user_id: &str,
        chat_id: i64,
        thread_id: Option<i32>,
    ) {
        self.outbound
            .insert(user_id.to_string(), (chat_id, thread_id));
    }

    /// Resolves the outbound target for a reply to a user.
    ///
    /// Returns `(chat_id, Option<thread_id>)` from the user's last-seen location.
    pub fn resolve_outbound(&self, user_id: &str) -> Option<(i64, Option<i32>)> {
        self.outbound.get(user_id).copied()
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
}
