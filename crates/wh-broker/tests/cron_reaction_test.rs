//! Acceptance tests for Story 5-6: Agent Reaction to CronEvent
//!
//! These tests verify that agents can subscribe to CronEvent objects and
//! execute scheduled actions when they arrive.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use prost_types::Timestamp;
use tokio::sync::{mpsc, Mutex};

use wh_broker::cron::dispatcher::CronEventDispatcher;
use wh_broker::cron::handler::{
    CronEventHandler, CronHandlerError, HandlerOutcome, ProgressUpdate,
};
use wh_broker::cron::CronEventMessage;

/// Helper: creates a CronEventMessage for testing.
fn make_event(job_name: &str, action: &str) -> CronEventMessage {
    CronEventMessage {
        job_name: job_name.to_string(),
        action: action.to_string(),
        schedule: "* * * * * *".to_string(),
        payload: HashMap::new(),
        target_stream: String::new(),
        triggered_at: Timestamp::default(),
    }
}

// --- Test Helpers ---

/// A mock handler that records invocations.
#[derive(Clone)]
struct RecordingHandler {
    invocations: Arc<Mutex<Vec<CronEventMessage>>>,
    delay: Option<Duration>,
}

impl RecordingHandler {
    fn new() -> Self {
        Self {
            invocations: Arc::new(Mutex::new(Vec::new())),
            delay: None,
        }
    }

    fn with_delay(delay: Duration) -> Self {
        Self {
            invocations: Arc::new(Mutex::new(Vec::new())),
            delay: Some(delay),
        }
    }
}

#[async_trait::async_trait]
impl CronEventHandler for RecordingHandler {
    async fn handle(
        &self,
        event: CronEventMessage,
    ) -> Result<HandlerOutcome, CronHandlerError> {
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        self.invocations.lock().await.push(event);
        Ok(HandlerOutcome::Completed {
            message: "handled".to_string(),
        })
    }
}

// =============================================================================
// AC1: Agent processes CronEvent and executes associated action (FR38)
// =============================================================================

#[tokio::test]
async fn ac1_agent_processes_cronevent_and_executes_action() {
    // Given: a dispatcher with a handler registered for "daily-compaction"
    let handler = Arc::new(RecordingHandler::new());
    let handler_ref = Arc::clone(&handler);

    let mut dispatcher = CronEventDispatcher::new();
    dispatcher.register_handler("daily-compaction", handler);

    // When: a CronEvent with job_name "daily-compaction" arrives
    let event = make_event("daily-compaction", "compact");
    let handle = dispatcher.dispatch(event).unwrap();
    handle.await.unwrap();

    // Then: the handler is invoked exactly once
    let invocations = handler_ref.invocations.lock().await;
    assert_eq!(invocations.len(), 1);
    assert_eq!(invocations[0].job_name, "daily-compaction");
}

#[tokio::test]
async fn ac1_handler_receives_correct_event_fields() {
    // Given: a handler that records the full event
    let handler = Arc::new(RecordingHandler::new());
    let handler_ref = Arc::clone(&handler);

    let mut dispatcher = CronEventDispatcher::new();
    dispatcher.register_handler("daily-compaction", handler);

    // When: a CronEvent with specific fields arrives
    let mut event = make_event("daily-compaction", "compact");
    event.payload.insert("stream".to_string(), "main".to_string());

    let handle = dispatcher.dispatch(event).unwrap();
    handle.await.unwrap();

    // Then: the handler receives the event with all correct fields
    let invocations = handler_ref.invocations.lock().await;
    assert_eq!(invocations.len(), 1);
    assert_eq!(invocations[0].job_name, "daily-compaction");
    assert_eq!(invocations[0].action, "compact");
    assert_eq!(invocations[0].schedule, "* * * * * *");
    assert_eq!(
        invocations[0].payload.get("stream"),
        Some(&"main".to_string())
    );
}

#[tokio::test]
async fn ac1_unknown_job_handled_gracefully() {
    // Given: dispatcher with no handler for "unknown-job"
    let dispatcher = CronEventDispatcher::new();

    // When: a CronEvent with unknown job_name arrives
    let event = make_event("unknown-job", "event");

    // Then: dispatch returns None without panic
    let handle = dispatcher.dispatch(event);
    assert!(handle.is_none(), "unknown job should not spawn a task");
}

// =============================================================================
// AC2: Long-running action publishes progress after 30 seconds
// =============================================================================

#[tokio::test]
async fn ac2_long_running_handler_publishes_progress_after_30s() {
    tokio::time::pause();

    // Given: a handler that takes 35 seconds to complete
    let handler = Arc::new(RecordingHandler::with_delay(Duration::from_secs(35)));

    let (progress_tx, mut progress_rx) = mpsc::channel::<ProgressUpdate>(16);

    let mut dispatcher = CronEventDispatcher::new();
    dispatcher.register_handler("slow-job", handler);
    dispatcher.set_progress_sender(progress_tx);

    // When: the CronEvent is dispatched
    let _handle = dispatcher.dispatch(make_event("slow-job", "event")).unwrap();

    // Yield to let the spawned task start
    tokio::task::yield_now().await;

    // Advance time past the 30-second threshold
    tokio::time::advance(Duration::from_secs(31)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    // Then: at least one progress update is published
    let progress = progress_rx.try_recv();
    assert!(
        progress.is_ok(),
        "progress update should be published after 30s"
    );
    let update = progress.unwrap();
    assert_eq!(update.job_name, "slow-job");
    assert!(update.message.contains("still running"));
}

#[tokio::test]
async fn ac2_fast_handler_does_not_trigger_progress() {
    tokio::time::pause();

    // Given: a handler that completes in 1 second
    let handler = Arc::new(RecordingHandler::with_delay(Duration::from_millis(100)));

    let (progress_tx, mut progress_rx) = mpsc::channel::<ProgressUpdate>(16);

    let mut dispatcher = CronEventDispatcher::new();
    dispatcher.register_handler("fast-job", handler);
    dispatcher.set_progress_sender(progress_tx);

    // When: the CronEvent completes quickly
    let handle = dispatcher
        .dispatch(make_event("fast-job", "event"))
        .unwrap();

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(200)).await;
    tokio::task::yield_now().await;

    handle.await.unwrap();

    // Then: no progress update is published
    assert!(
        progress_rx.try_recv().is_err(),
        "fast handler should not trigger progress"
    );
}

// =============================================================================
// AC3: Multiple simultaneous CronEvents processed concurrently
// =============================================================================

#[tokio::test]
async fn ac3_two_simultaneous_events_processed_concurrently() {
    // Given: two handlers registered for different job names
    let handler_a = Arc::new(RecordingHandler::with_delay(Duration::from_millis(50)));
    let handler_b = Arc::new(RecordingHandler::with_delay(Duration::from_millis(50)));
    let ref_a = Arc::clone(&handler_a);
    let ref_b = Arc::clone(&handler_b);

    let mut dispatcher = CronEventDispatcher::new();
    dispatcher.register_handler("job-a", handler_a);
    dispatcher.register_handler("job-b", handler_b);

    // When: two CronEvents arrive simultaneously
    let handle_a = dispatcher
        .dispatch(make_event("job-a", "event"))
        .unwrap();
    let handle_b = dispatcher
        .dispatch(make_event("job-b", "event"))
        .unwrap();

    // Then: both complete independently
    handle_a.await.unwrap();
    handle_b.await.unwrap();

    assert_eq!(ref_a.invocations.lock().await.len(), 1);
    assert_eq!(ref_b.invocations.lock().await.len(), 1);
}

#[tokio::test]
async fn ac3_fast_handler_completes_independently_of_slow() {
    tokio::time::pause();

    // Given: handler A takes 5 seconds, handler B takes 100ms
    let handler_a = Arc::new(RecordingHandler::with_delay(Duration::from_secs(5)));
    let handler_b = Arc::new(RecordingHandler::with_delay(Duration::from_millis(100)));
    let ref_b = Arc::clone(&handler_b);

    let mut dispatcher = CronEventDispatcher::new();
    dispatcher.register_handler("slow-a", handler_a);
    dispatcher.register_handler("fast-b", handler_b);

    // When: both events dispatched
    let _handle_a = dispatcher
        .dispatch(make_event("slow-a", "event"))
        .unwrap();
    let handle_b = dispatcher
        .dispatch(make_event("fast-b", "event"))
        .unwrap();

    // Advance 200ms — handler B should complete, handler A still running
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(200)).await;
    tokio::task::yield_now().await;

    handle_b.await.unwrap();

    // Then: handler B completed without waiting for handler A
    assert_eq!(
        ref_b.invocations.lock().await.len(),
        1,
        "fast handler should complete independently of slow handler"
    );
}
