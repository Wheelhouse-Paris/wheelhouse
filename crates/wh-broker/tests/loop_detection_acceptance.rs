//! Acceptance tests for Story 3-7: Agent Loop Detection and Behavioral Alerting
//!
//! These tests verify the four acceptance criteria:
//! AC1: Silence beyond configured timeout triggers alert with correct message
//! AC2: Alert contains agent name and duration for log debugging
//! AC3: Timer resets when agent resumes publishing
//! AC4: Zero timeout disables monitoring entirely

use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time;
use wh_broker::monitor::{
    AgentMonitorConfig, MonitorRegistry, SilenceAlert, SilenceMonitor,
};

/// AC1: Agent configured with timeout triggers alert after silence period.
/// Given an agent with loop_detection_timeout configured,
/// When the agent publishes no objects for the timeout duration,
/// Then a SilenceAlert is sent with correct agent name and duration.
#[tokio::test]
async fn test_ac1_silence_triggers_alert_after_timeout() {
    time::pause();

    let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
    let config = AgentMonitorConfig {
        agent_name: "researcher-2".to_string(),
        stream_name: "research-output".to_string(),
        timeout: Duration::from_millis(100),
    };
    let monitor = SilenceMonitor::new(config, tx);
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let handle = tokio::spawn(async move {
        monitor.run(cancel_clone).await;
    });

    // Advance time past the timeout
    time::advance(Duration::from_millis(150)).await;
    tokio::task::yield_now().await;

    // Should receive an alert
    let alert = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive alert within timeout")
        .expect("channel should not be closed");

    assert_eq!(alert.agent_name, "researcher-2");
    assert_eq!(alert.stream_name, "research-output");
    assert!(alert.silent_duration >= Duration::from_millis(100));

    cancel.cancel();
    let _ = handle.await;
}

/// AC1: Alert message matches the exact format specified in the epic.
#[tokio::test]
async fn test_ac1_alert_message_format() {
    let alert = SilenceAlert {
        agent_name: "researcher-2".to_string(),
        stream_name: "research-output".to_string(),
        silent_duration: Duration::from_secs(900), // 15 minutes
        message: String::new(), // will be overwritten by format_notification
    };

    let notification = alert.format_notification();
    assert_eq!(
        notification,
        "researcher-2: no stream output in 15 minutes \u{2014} possible loop or hang"
    );
}

/// AC2: Alert contains agent name and human-readable duration for log debugging.
#[tokio::test]
async fn test_ac2_alert_contains_agent_and_duration() {
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

/// AC3: Timer resets when agent resumes publishing.
/// Given the monitor is running,
/// When record_activity() is called before timeout,
/// Then no alert is sent.
#[tokio::test]
async fn test_ac3_timer_resets_on_activity() {
    time::pause();

    let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
    let config = AgentMonitorConfig {
        agent_name: "researcher-2".to_string(),
        stream_name: "research-output".to_string(),
        timeout: Duration::from_millis(200),
    };
    let monitor = SilenceMonitor::new(config, tx);
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let activity_handle = monitor.activity_handle();

    let handle = tokio::spawn(async move {
        monitor.run(cancel_clone).await;
    });

    // Advance partway, then record activity to reset timer
    time::advance(Duration::from_millis(150)).await;
    tokio::task::yield_now().await;
    activity_handle.record_activity();

    // Advance another 150ms (total 300ms from start, but only 150ms from reset)
    time::advance(Duration::from_millis(150)).await;
    tokio::task::yield_now().await;

    // No alert should have been sent (timeout is 200ms, only 150ms since reset)
    let result = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
    assert!(result.is_err(), "should not receive alert after activity reset");

    cancel.cancel();
    let _ = handle.await;
}

/// AC3: No alert after reset, but alert fires on next silence period.
#[tokio::test]
async fn test_ac3_no_alert_after_reset() {
    time::pause();

    let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
    let config = AgentMonitorConfig {
        agent_name: "researcher-2".to_string(),
        stream_name: "research-output".to_string(),
        timeout: Duration::from_millis(100),
    };
    let monitor = SilenceMonitor::new(config, tx);
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let activity_handle = monitor.activity_handle();

    let handle = tokio::spawn(async move {
        monitor.run(cancel_clone).await;
    });

    // Record activity right away to prevent alert
    activity_handle.record_activity();

    // Advance past timeout — but activity was just recorded
    time::advance(Duration::from_millis(80)).await;
    tokio::task::yield_now().await;

    // No alert (only 80ms since reset, timeout is 100ms)
    let result = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await;
    assert!(result.is_err(), "should not receive alert within timeout");

    // Now wait full timeout without activity
    time::advance(Duration::from_millis(120)).await;
    tokio::task::yield_now().await;

    // Alert should fire now
    let alert = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive alert after silence")
        .expect("channel should not be closed");

    assert_eq!(alert.agent_name, "researcher-2");

    cancel.cancel();
    let _ = handle.await;
}

/// AC4: Zero timeout disables monitoring entirely.
/// Given loop_detection_timeout: 0,
/// When the broker starts,
/// Then loop detection is disabled — no alerts ever sent.
#[tokio::test]
async fn test_ac4_zero_timeout_disables_monitoring() {
    time::pause();

    let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
    let config = AgentMonitorConfig {
        agent_name: "assistant-1".to_string(),
        stream_name: "assistant-output".to_string(),
        timeout: Duration::ZERO,
    };

    assert!(!config.is_enabled(), "zero timeout should mean disabled");

    let monitor = SilenceMonitor::new(config, tx);
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let handle = tokio::spawn(async move {
        monitor.run(cancel_clone).await;
    });

    // run() should return immediately for zero timeout.
    // The sender is moved into the task and dropped, closing the channel.
    let _ = handle.await;

    // No messages should have been sent before the channel closed.
    let result = rx.try_recv();
    assert!(result.is_err(), "disabled monitor should never send alerts");

    cancel.cancel();
}

/// Registry correctly routes activity to the right monitor.
#[tokio::test]
async fn test_registry_routes_activity_to_correct_monitor() {
    time::pause();

    let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
    let mut registry = MonitorRegistry::new(tx);

    registry.register(AgentMonitorConfig {
        agent_name: "agent-a".to_string(),
        stream_name: "stream-a".to_string(),
        timeout: Duration::from_millis(100),
    });
    registry.register(AgentMonitorConfig {
        agent_name: "agent-b".to_string(),
        stream_name: "stream-b".to_string(),
        timeout: Duration::from_millis(100),
    });

    let cancel = tokio_util::sync::CancellationToken::new();
    let _handles = registry.start_all(cancel.clone());

    // Keep agent-a alive, let agent-b go silent
    registry.record_activity("agent-a");

    time::advance(Duration::from_millis(150)).await;
    tokio::task::yield_now().await;

    // Record activity for agent-a again to prevent its alert
    registry.record_activity("agent-a");

    // Should get alert for agent-b only
    let alert = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive alert")
        .expect("channel should not be closed");

    assert_eq!(alert.agent_name, "agent-b");

    cancel.cancel();
}

/// Recording activity for an unknown agent is a no-op.
#[tokio::test]
async fn test_registry_unknown_agent_is_noop() {
    let (tx, _rx) = mpsc::channel::<SilenceAlert>(10);
    let registry = MonitorRegistry::new(tx);

    // Should not panic
    registry.record_activity("nonexistent-agent");
}

/// Only one alert per silence period (no spam).
#[tokio::test]
async fn test_no_alert_spam_single_alert_per_silence() {
    time::pause();

    let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
    let config = AgentMonitorConfig {
        agent_name: "researcher-2".to_string(),
        stream_name: "research-output".to_string(),
        timeout: Duration::from_millis(100),
    };
    let monitor = SilenceMonitor::new(config, tx);
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();

    let handle = tokio::spawn(async move {
        monitor.run(cancel_clone).await;
    });

    // Wait for first alert
    time::advance(Duration::from_millis(150)).await;
    tokio::task::yield_now().await;

    let _first_alert = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive first alert")
        .expect("channel should not be closed");

    // Wait more time — should NOT get a second alert
    time::advance(Duration::from_millis(500)).await;
    tokio::task::yield_now().await;

    let result = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
    assert!(result.is_err(), "should not receive duplicate alert");

    cancel.cancel();
    let _ = handle.await;
}

/// Multiple monitors run concurrently and independently.
#[tokio::test]
async fn test_concurrent_monitors_independent() {
    time::pause();

    let (tx, mut rx) = mpsc::channel::<SilenceAlert>(10);
    let mut registry = MonitorRegistry::new(tx);

    registry.register(AgentMonitorConfig {
        agent_name: "fast-agent".to_string(),
        stream_name: "fast-stream".to_string(),
        timeout: Duration::from_millis(50),
    });
    registry.register(AgentMonitorConfig {
        agent_name: "slow-agent".to_string(),
        stream_name: "slow-stream".to_string(),
        timeout: Duration::from_millis(200),
    });

    let cancel = tokio_util::sync::CancellationToken::new();
    let _handles = registry.start_all(cancel.clone());

    // After 100ms, fast-agent should alert but slow-agent should not
    time::advance(Duration::from_millis(100)).await;
    tokio::task::yield_now().await;

    let alert = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive fast-agent alert")
        .expect("channel should not be closed");

    assert_eq!(alert.agent_name, "fast-agent");

    // After 250ms total, slow-agent should also alert
    time::advance(Duration::from_millis(150)).await;
    tokio::task::yield_now().await;

    let alert2 = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive slow-agent alert")
        .expect("channel should not be closed");

    assert_eq!(alert2.agent_name, "slow-agent");

    cancel.cancel();
}
