//! Monitor registry — manages per-agent silence monitors.
//!
//! The registry creates and stores monitors for each configured agent,
//! provides a unified `record_activity()` interface for the routing loop,
//! and spawns all monitor tasks on `start_all()`.

use std::collections::HashMap;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::silence::{ActivityHandle, SilenceAlert, SilenceMonitor};
use super::AgentMonitorConfig;

/// Registry managing all per-agent silence monitors.
pub struct MonitorRegistry {
    activity_handles: HashMap<String, ActivityHandle>,
    monitors: Vec<SilenceMonitor>,
    alert_sender: mpsc::Sender<SilenceAlert>,
}

impl MonitorRegistry {
    /// Create a new registry with the given alert channel sender.
    pub fn new(alert_sender: mpsc::Sender<SilenceAlert>) -> Self {
        Self {
            activity_handles: HashMap::new(),
            monitors: Vec::new(),
            alert_sender,
        }
    }

    /// Register a new agent for monitoring.
    ///
    /// Creates a `SilenceMonitor` and stores its `ActivityHandle`.
    /// If the config has `timeout == 0` (disabled), the agent is skipped.
    pub fn register(&mut self, config: AgentMonitorConfig) {
        if !config.is_enabled() {
            tracing::info!(
                agent = %config.agent_name,
                "loop detection disabled (timeout=0), skipping monitor"
            );
            return;
        }

        let monitor = SilenceMonitor::new(config.clone(), self.alert_sender.clone());
        let handle = monitor.activity_handle();

        self.activity_handles
            .insert(config.agent_name.clone(), handle);
        self.monitors.push(monitor);

        tracing::info!(
            agent = %config.agent_name,
            timeout_secs = config.timeout.as_secs(),
            "registered silence monitor"
        );
    }

    /// Record publish activity for the named agent.
    ///
    /// Delegates to the agent's `ActivityHandle`. If the agent is not
    /// registered (or was disabled), this is a no-op.
    pub fn record_activity(&self, agent_name: &str) {
        if let Some(handle) = self.activity_handles.get(agent_name) {
            handle.record_activity();
        }
    }

    /// Start all registered monitors. Drains the monitor list and spawns
    /// a `tokio::spawn` task for each.
    ///
    /// Returns the `JoinHandle`s for the spawned tasks.
    pub fn start_all(&mut self, cancel: CancellationToken) -> Vec<JoinHandle<()>> {
        self.monitors
            .drain(..)
            .map(|monitor| {
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    monitor.run(cancel).await;
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_register_and_record_activity() {
        let (tx, _rx) = mpsc::channel::<SilenceAlert>(10);
        let mut registry = MonitorRegistry::new(tx);

        registry.register(AgentMonitorConfig {
            agent_name: "agent-a".to_string(),
            stream_name: "stream-a".to_string(),
            timeout: Duration::from_secs(60),
        });

        // Should not panic, should route to agent-a's handle
        registry.record_activity("agent-a");
    }

    #[tokio::test]
    async fn test_register_disabled_config_skips() {
        let (tx, _rx) = mpsc::channel::<SilenceAlert>(10);
        let mut registry = MonitorRegistry::new(tx);

        registry.register(AgentMonitorConfig {
            agent_name: "disabled-agent".to_string(),
            stream_name: "stream-d".to_string(),
            timeout: Duration::ZERO,
        });

        assert!(
            registry.activity_handles.is_empty(),
            "disabled agent should not be registered"
        );
        assert!(
            registry.monitors.is_empty(),
            "disabled agent should not have a monitor"
        );
    }

    #[tokio::test]
    async fn test_record_activity_unknown_agent_is_noop() {
        let (tx, _rx) = mpsc::channel::<SilenceAlert>(10);
        let registry = MonitorRegistry::new(tx);

        // Should not panic
        registry.record_activity("nonexistent-agent");
    }

    #[tokio::test]
    async fn test_start_all_drains_monitors() {
        let (tx, _rx) = mpsc::channel::<SilenceAlert>(10);
        let mut registry = MonitorRegistry::new(tx);

        registry.register(AgentMonitorConfig {
            agent_name: "agent-a".to_string(),
            stream_name: "stream-a".to_string(),
            timeout: Duration::from_secs(60),
        });

        let cancel = CancellationToken::new();
        let handles = registry.start_all(cancel.clone());

        assert_eq!(handles.len(), 1, "should spawn one task");
        assert!(
            registry.monitors.is_empty(),
            "monitors should be drained after start_all"
        );

        // Activity handle should still work
        registry.record_activity("agent-a");

        cancel.cancel();
        for h in handles {
            let _ = h.await;
        }
    }
}
