//! Acknowledgement timer for slow responses.
//!
//! Sends "Working on it..." after a configurable timeout (default 5s)
//! if no agent response arrives. Applies to ALL requests exceeding the
//! threshold (AC #5).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex};
use tracing::instrument;

/// Tracks pending messages and fires acknowledgement signals when responses are slow.
pub struct AckTracker {
    timeout: Duration,
    /// Pending timers keyed by (user_id, message_id).
    pending: Arc<Mutex<HashMap<(String, String), tokio::task::JoinHandle<()>>>>,
}

impl AckTracker {
    /// Creates a new AckTracker with the given timeout duration.
    ///
    /// Production default: 5 seconds. Tests may use shorter durations.
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Starts tracking a message. Returns a receiver that fires when the ack should be sent.
    ///
    /// If a response arrives before the timeout, call `cancel()` to prevent the ack.
    #[instrument(skip(self))]
    pub async fn track(
        &self,
        user_id: &str,
        message_id: &str,
    ) -> mpsc::UnboundedReceiver<()> {
        let (tx, rx) = mpsc::unbounded_channel();
        let timeout = self.timeout;
        let key = (user_id.to_string(), message_id.to_string());

        let pending_clone = self.pending.clone();
        let cleanup_key = key.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let _ = tx.send(());
            // Clean up the entry after firing to prevent memory leak
            pending_clone.lock().await.remove(&cleanup_key);
        });

        self.pending.lock().await.insert(key, handle);
        rx
    }

    /// Cancels a pending ack timer for the given user/message.
    ///
    /// Call this when a response arrives before the timeout.
    #[instrument(skip(self))]
    pub async fn cancel(&self, user_id: &str, message_id: &str) {
        let key = (user_id.to_string(), message_id.to_string());
        if let Some(handle) = self.pending.lock().await.remove(&key) {
            handle.abort();
        }
    }

    /// Cancels all pending ack timers for a given user.
    #[instrument(skip(self))]
    pub async fn cancel_all_for_user(&self, user_id: &str) {
        let mut pending = self.pending.lock().await;
        let keys_to_remove: Vec<_> = pending
            .keys()
            .filter(|(uid, _)| uid == user_id)
            .cloned()
            .collect();
        for key in keys_to_remove {
            if let Some(handle) = pending.remove(&key) {
                handle.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fires_after_timeout() {
        let tracker = AckTracker::new(Duration::from_millis(50));
        let mut rx = tracker.track("usr_abc", "msg_001").await;
        // Wait for timeout to fire
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(rx.try_recv().is_ok(), "ack should have fired");
    }

    #[tokio::test]
    async fn does_not_fire_after_cancel() {
        let tracker = AckTracker::new(Duration::from_millis(100));
        let mut rx = tracker.track("usr_abc", "msg_001").await;
        // Cancel before timeout
        tracker.cancel("usr_abc", "msg_001").await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(rx.try_recv().is_err(), "ack should not have fired after cancel");
    }

    #[tokio::test]
    async fn cancel_all_for_user() {
        let tracker = AckTracker::new(Duration::from_millis(100));
        let mut rx1 = tracker.track("usr_abc", "msg_001").await;
        let mut rx2 = tracker.track("usr_abc", "msg_002").await;
        let mut rx3 = tracker.track("usr_other", "msg_003").await;

        tracker.cancel_all_for_user("usr_abc").await;
        tokio::time::sleep(Duration::from_millis(150)).await;

        assert!(rx1.try_recv().is_err(), "user_abc msg_001 should be cancelled");
        assert!(rx2.try_recv().is_err(), "user_abc msg_002 should be cancelled");
        assert!(rx3.try_recv().is_ok(), "other user should still fire");
    }
}
