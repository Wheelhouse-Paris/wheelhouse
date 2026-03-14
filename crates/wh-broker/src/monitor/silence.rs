//! Silence monitor — per-agent inactivity tracker.
//!
//! Tracks the last time an agent published to its stream. If no activity
//! is recorded within the configured timeout, a `SilenceAlert` is sent
//! via channel. After alerting, the monitor waits for the next
//! `record_activity()` call before re-alerting (no spam).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time;
use tokio_util::sync::CancellationToken;

use super::AgentMonitorConfig;

/// Alert emitted when an agent has been silent beyond its configured timeout.
#[derive(Debug, Clone)]
pub struct SilenceAlert {
    pub agent_name: String,
    pub stream_name: String,
    pub silent_duration: Duration,
    pub message: String,
}

impl SilenceAlert {
    /// Format a human-readable notification message.
    ///
    /// Format: `"{agent_name}: no stream output in {duration} \u{2014} possible loop or hang"`
    pub fn format_notification(&self) -> String {
        let human = format_human_duration(self.silent_duration);
        format!(
            "{}: no stream output in {} \u{2014} possible loop or hang",
            self.agent_name, human
        )
    }
}

/// Format a duration as a human-readable string.
///
/// Returns `"X seconds"`, `"X minutes"`, or `"X hours"` based on magnitude.
/// Rounds to the nearest whole unit.
pub fn format_human_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    if total_secs == 0 {
        return "0 seconds".to_string();
    }

    if total_secs >= 3600 {
        let hours = (total_secs as f64 / 3600.0).round() as u64;
        if hours == 1 {
            "1 hour".to_string()
        } else {
            format!("{hours} hours")
        }
    } else if total_secs >= 60 {
        let minutes = (total_secs as f64 / 60.0).round() as u64;
        if minutes == 1 {
            "1 minute".to_string()
        } else {
            format!("{minutes} minutes")
        }
    } else if total_secs == 1 {
        "1 second".to_string()
    } else {
        format!("{total_secs} seconds")
    }
}

/// Cheap, cloneable handle for recording activity from the routing hot path.
///
/// Wraps an `Arc<AtomicU64>` storing elapsed milliseconds from a shared
/// monotonic epoch. Lock-free for performance on the publish hot path.
#[derive(Debug, Clone)]
pub struct ActivityHandle {
    last_activity_ms: Arc<AtomicU64>,
    epoch: tokio::time::Instant,
}

impl ActivityHandle {
    /// Record that the agent has just published. Resets the silence timer.
    pub fn record_activity(&self) {
        let elapsed_ms = self.epoch.elapsed().as_millis() as u64;
        self.last_activity_ms.store(elapsed_ms, Ordering::Release);
    }
}

/// Per-agent silence monitor.
///
/// Tracks publish activity via a shared `AtomicU64` timestamp.
/// When silence exceeds the configured timeout, sends a `SilenceAlert`
/// via the provided channel. After alerting, waits for activity before
/// re-alerting.
pub struct SilenceMonitor {
    config: AgentMonitorConfig,
    last_activity_ms: Arc<AtomicU64>,
    alert_sender: mpsc::Sender<SilenceAlert>,
    epoch: tokio::time::Instant,
}

impl SilenceMonitor {
    /// Create a new silence monitor for the given agent configuration.
    pub fn new(config: AgentMonitorConfig, alert_sender: mpsc::Sender<SilenceAlert>) -> Self {
        let epoch = tokio::time::Instant::now();
        let last_activity_ms = Arc::new(AtomicU64::new(0));
        Self {
            config,
            last_activity_ms,
            alert_sender,
            epoch,
        }
    }

    /// Get an `ActivityHandle` for recording activity from external callers.
    pub fn activity_handle(&self) -> ActivityHandle {
        ActivityHandle {
            last_activity_ms: Arc::clone(&self.last_activity_ms),
            epoch: self.epoch,
        }
    }

    /// Run the silence monitoring loop.
    ///
    /// Checks periodically whether the agent has been silent beyond the
    /// configured timeout. If timeout is zero, returns immediately.
    ///
    /// After sending one alert, waits for the next `record_activity()`
    /// before checking again (prevents alert spam).
    pub async fn run(self, cancel: CancellationToken) {
        if !self.config.is_enabled() {
            return;
        }

        let timeout = self.config.timeout;
        // Check interval: timeout/2, minimum 10ms (for test compatibility)
        let check_interval = std::cmp::max(Duration::from_millis(10), timeout / 2);

        let mut interval = time::interval(check_interval);
        interval.tick().await; // first tick is immediate

        let mut alerted = false;
        let mut last_alerted_activity_ms = u64::MAX;

        loop {
            tokio::select! {
                biased;

                _ = cancel.cancelled() => {
                    tracing::info!(
                        agent = %self.config.agent_name,
                        "silence monitor shutting down"
                    );
                    return;
                }

                _ = interval.tick() => {
                    let current_activity_ms = self.last_activity_ms.load(Ordering::Acquire);
                    let now_ms = self.epoch.elapsed().as_millis() as u64;

                    // If activity recorded since last alert, reset alert state
                    if alerted && current_activity_ms != last_alerted_activity_ms {
                        alerted = false;
                    }

                    if alerted {
                        continue;
                    }

                    let silent_ms = now_ms.saturating_sub(current_activity_ms);
                    let silent_duration = Duration::from_millis(silent_ms);

                    if silent_duration >= timeout {
                        let alert = SilenceAlert {
                            agent_name: self.config.agent_name.clone(),
                            stream_name: self.config.stream_name.clone(),
                            silent_duration,
                            message: String::new(),
                        };
                        let notification = alert.format_notification();

                        tracing::warn!(
                            agent = %self.config.agent_name,
                            stream = %self.config.stream_name,
                            silent_seconds = silent_duration.as_secs(),
                            "{}", notification
                        );

                        let alert_with_message = SilenceAlert {
                            message: notification,
                            ..alert
                        };

                        if self.alert_sender.send(alert_with_message).await.is_err() {
                            tracing::error!(
                                agent = %self.config.agent_name,
                                "alert channel closed, stopping monitor"
                            );
                            return;
                        }

                        alerted = true;
                        last_alerted_activity_ms = current_activity_ms;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_human_duration_seconds() {
        assert_eq!(format_human_duration(Duration::from_secs(30)), "30 seconds");
    }

    #[test]
    fn test_format_human_duration_single_second() {
        assert_eq!(format_human_duration(Duration::from_secs(1)), "1 second");
    }

    #[test]
    fn test_format_human_duration_minutes() {
        assert_eq!(
            format_human_duration(Duration::from_secs(900)),
            "15 minutes"
        );
    }

    #[test]
    fn test_format_human_duration_single_hour() {
        assert_eq!(format_human_duration(Duration::from_secs(3600)), "1 hour");
    }

    #[test]
    fn test_format_human_duration_rounded_minutes() {
        // 90s = 1.5 min, rounds to 2
        assert_eq!(format_human_duration(Duration::from_secs(90)), "2 minutes");
    }

    #[test]
    fn test_format_human_duration_zero() {
        assert_eq!(format_human_duration(Duration::ZERO), "0 seconds");
    }

    #[test]
    fn test_alert_format_notification_15m() {
        let alert = SilenceAlert {
            agent_name: "researcher-2".to_string(),
            stream_name: "research-output".to_string(),
            silent_duration: Duration::from_secs(900),
            message: String::new(),
        };
        assert_eq!(
            alert.format_notification(),
            "researcher-2: no stream output in 15 minutes \u{2014} possible loop or hang"
        );
    }

    #[test]
    fn test_alert_format_notification_1h() {
        let alert = SilenceAlert {
            agent_name: "assistant-1".to_string(),
            stream_name: "assistant-output".to_string(),
            silent_duration: Duration::from_secs(3600),
            message: String::new(),
        };
        let notification = alert.format_notification();
        assert!(notification.contains("assistant-1"));
        assert!(notification.contains("1 hour"));
        assert!(notification.contains("possible loop or hang"));
    }

    #[tokio::test]
    async fn test_record_activity_updates_timestamp() {
        let (tx, _rx) = mpsc::channel::<SilenceAlert>(10);
        let config = AgentMonitorConfig {
            agent_name: "test-agent".to_string(),
            stream_name: "test-stream".to_string(),
            timeout: Duration::from_secs(60),
        };
        let monitor = SilenceMonitor::new(config, tx);
        let handle = monitor.activity_handle();

        let before = monitor.last_activity_ms.load(Ordering::Acquire);
        tokio::time::sleep(Duration::from_millis(1)).await;
        handle.record_activity();
        let after = monitor.last_activity_ms.load(Ordering::Acquire);

        assert!(after > before, "activity should update timestamp");
    }

    #[tokio::test]
    async fn test_monitor_fires_alert_after_silence() {
        tokio::time::pause();

        let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
        let config = AgentMonitorConfig {
            agent_name: "test-agent".to_string(),
            stream_name: "test-stream".to_string(),
            timeout: Duration::from_millis(100),
        };
        let monitor = SilenceMonitor::new(config, tx);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            monitor.run(cancel_clone).await;
        });

        // Advance past timeout
        tokio::time::advance(Duration::from_millis(150)).await;
        tokio::task::yield_now().await;

        let alert = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive alert")
            .expect("channel open");

        assert_eq!(alert.agent_name, "test-agent");
        assert!(alert.silent_duration >= Duration::from_millis(100));

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_monitor_resets_on_activity() {
        tokio::time::pause();

        let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
        let config = AgentMonitorConfig {
            agent_name: "test-agent".to_string(),
            stream_name: "test-stream".to_string(),
            timeout: Duration::from_millis(200),
        };
        let monitor = SilenceMonitor::new(config, tx);
        let activity = monitor.activity_handle();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            monitor.run(cancel_clone).await;
        });

        // Advance partway, then record activity
        tokio::time::advance(Duration::from_millis(150)).await;
        tokio::task::yield_now().await;
        activity.record_activity();

        // Advance another 150ms (only 150ms since reset, timeout is 200ms)
        tokio::time::advance(Duration::from_millis(150)).await;
        tokio::task::yield_now().await;

        let result = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(result.is_err(), "no alert expected after activity reset");

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_monitor_disabled_with_zero_timeout() {
        tokio::time::pause();

        let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
        let config = AgentMonitorConfig {
            agent_name: "test-agent".to_string(),
            stream_name: "test-stream".to_string(),
            timeout: Duration::ZERO,
        };

        assert!(!config.is_enabled());

        let monitor = SilenceMonitor::new(config, tx);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            monitor.run(cancel_clone).await;
        });

        // run() should return immediately for zero timeout
        let _ = handle.await;

        // Channel sender was moved into the spawned task and dropped when run() returned.
        // rx.recv() should return None (channel closed) with no messages sent.
        let result = rx.try_recv();
        assert!(result.is_err(), "disabled monitor should never send alerts");

        cancel.cancel();
    }

    #[tokio::test]
    async fn test_monitor_no_alert_spam() {
        tokio::time::pause();

        let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
        let config = AgentMonitorConfig {
            agent_name: "test-agent".to_string(),
            stream_name: "test-stream".to_string(),
            timeout: Duration::from_millis(100),
        };
        let monitor = SilenceMonitor::new(config, tx);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            monitor.run(cancel_clone).await;
        });

        // Wait for first alert
        tokio::time::advance(Duration::from_millis(150)).await;
        tokio::task::yield_now().await;

        let _first = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("first alert")
            .expect("channel open");

        // Wait more — should NOT get second alert
        tokio::time::advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;

        let result = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(result.is_err(), "should not receive duplicate alert");

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_monitor_respects_cancellation() {
        tokio::time::pause();

        let (tx, _rx) = mpsc::channel::<SilenceAlert>(10);
        let config = AgentMonitorConfig {
            agent_name: "test-agent".to_string(),
            stream_name: "test-stream".to_string(),
            timeout: Duration::from_secs(60),
        };
        let monitor = SilenceMonitor::new(config, tx);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            monitor.run(cancel_clone).await;
        });

        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "monitor should shut down after cancel");
    }
}
