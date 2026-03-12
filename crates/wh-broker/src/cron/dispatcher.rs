//! CronEvent dispatcher for concurrent event processing.
//!
//! Routes incoming CronEventMessages to registered handlers, spawning
//! each handler in its own tokio task for concurrent execution.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::handler::{CronEventHandler, ProgressUpdate};
use super::CronEventMessage;

/// Default progress timeout threshold in seconds.
const PROGRESS_TIMEOUT_SECS: u64 = 30;

/// Dispatches CronEvent messages to registered handlers concurrently.
///
/// Each dispatch spawns a new `tokio` task, ensuring that multiple
/// simultaneous events are processed independently without blocking.
pub struct CronEventDispatcher {
    /// Registered handlers indexed by job_name.
    handlers: HashMap<String, Arc<dyn CronEventHandler>>,
    /// Optional sender for progress updates when handlers exceed the timeout.
    progress_tx: Option<mpsc::Sender<ProgressUpdate>>,
}

impl CronEventDispatcher {
    /// Creates a new dispatcher with no handlers registered.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            progress_tx: None,
        }
    }

    /// Registers a handler for a specific job_name.
    ///
    /// If a handler is already registered for this job_name, it is replaced.
    pub fn register_handler(&mut self, job_name: &str, handler: Arc<dyn CronEventHandler>) {
        self.handlers.insert(job_name.to_string(), handler);
    }

    /// Sets the progress sender for long-running handler notifications.
    ///
    /// When set, the dispatcher will publish a `ProgressUpdate` if a handler
    /// has not completed within 30 seconds.
    pub fn set_progress_sender(&mut self, tx: mpsc::Sender<ProgressUpdate>) {
        self.progress_tx = Some(tx);
    }

    /// Dispatches a CronEvent to the appropriate handler.
    ///
    /// Returns `Some(JoinHandle)` if a handler was found and spawned,
    /// or `None` if no handler is registered for this job_name.
    #[tracing::instrument(skip_all, fields(job_name = %event.job_name))]
    pub fn dispatch(&self, event: CronEventMessage) -> Option<JoinHandle<()>> {
        let handler = match self.handlers.get(&event.job_name) {
            Some(h) => Arc::clone(h),
            None => {
                tracing::warn!(
                    job_name = %event.job_name,
                    "no handler registered for cron job"
                );
                return None;
            }
        };

        let progress_tx = self.progress_tx.clone();
        let job_name = event.job_name.clone();

        let handle = tokio::spawn(async move {
            if let Some(progress_tx) = progress_tx {
                // Race handler against 30-second progress timeout
                let handler_future = handler.handle(event);
                tokio::pin!(handler_future);

                let timer = tokio::time::sleep(Duration::from_secs(PROGRESS_TIMEOUT_SECS));
                tokio::pin!(timer);

                // First select: race handler vs timer
                tokio::select! {
                    biased;
                    result = &mut handler_future => {
                        // Handler completed before timeout
                        match result {
                            Ok(outcome) => {
                                tracing::info!(
                                    job_name = %job_name,
                                    ?outcome,
                                    "cron handler completed"
                                );
                            }
                            Err(err) => {
                                tracing::error!(
                                    job_name = %job_name,
                                    error_code = %err.code(),
                                    error = %err,
                                    "cron handler failed"
                                );
                            }
                        }
                    }
                    _ = &mut timer => {
                        // Timer fired — publish progress, then continue waiting for handler
                        let progress = ProgressUpdate {
                            job_name: job_name.clone(),
                            message: format!("cron job '{}' still running after {}s", job_name, PROGRESS_TIMEOUT_SECS),
                            percent: None,
                        };
                        if let Err(e) = progress_tx.send(progress).await {
                            tracing::warn!(
                                job_name = %job_name,
                                error = %e,
                                "failed to send progress update"
                            );
                        }

                        // Continue waiting for the handler to complete
                        match handler_future.await {
                            Ok(outcome) => {
                                tracing::info!(
                                    job_name = %job_name,
                                    ?outcome,
                                    "cron handler completed (after progress timeout)"
                                );
                            }
                            Err(err) => {
                                tracing::error!(
                                    job_name = %job_name,
                                    error_code = %err.code(),
                                    error = %err,
                                    "cron handler failed (after progress timeout)"
                                );
                            }
                        }
                    }
                }
            } else {
                // No progress sender — just run the handler
                match handler.handle(event).await {
                    Ok(outcome) => {
                        tracing::info!(
                            job_name = %job_name,
                            ?outcome,
                            "cron handler completed"
                        );
                    }
                    Err(err) => {
                        tracing::error!(
                            job_name = %job_name,
                            error_code = %err.code(),
                            error = %err,
                            "cron handler failed"
                        );
                    }
                }
            }
        });

        Some(handle)
    }

    /// Runs the dispatcher event loop, consuming events from the channel.
    ///
    /// Checks `CancellationToken` via biased `tokio::select!` for clean
    /// shutdown (CRF-01 / SC-06).
    #[tracing::instrument(skip_all)]
    pub async fn run(
        &self,
        mut receiver: mpsc::Receiver<CronEventMessage>,
        cancel: CancellationToken,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    tracing::info!("cron dispatcher shutting down");
                    break;
                }
                event = receiver.recv() => {
                    match event {
                        Some(msg) => {
                            self.dispatch(msg);
                        }
                        None => {
                            tracing::info!("cron event channel closed, dispatcher stopping");
                            break;
                        }
                    }
                }
            }
        }
    }
}

impl Default for CronEventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::handler::{CronHandlerError, HandlerOutcome};
    use prost_types::Timestamp;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    /// A test handler that records invocations.
    struct RecordingHandler {
        invocations: Arc<Mutex<Vec<String>>>,
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
            self.invocations.lock().await.push(event.job_name.clone());
            Ok(HandlerOutcome::Completed {
                message: format!("handled {}", event.job_name),
            })
        }
    }

    fn make_event(job_name: &str) -> CronEventMessage {
        CronEventMessage {
            job_name: job_name.to_string(),
            action: "event".to_string(),
            schedule: "* * * * *".to_string(),
            payload: HashMap::new(),
            target_stream: String::new(),
            triggered_at: Timestamp::default(),
        }
    }

    #[tokio::test]
    async fn dispatch_routes_to_correct_handler() {
        let handler = Arc::new(RecordingHandler::new());
        let handler_ref = Arc::clone(&handler);

        let mut dispatcher = CronEventDispatcher::new();
        dispatcher.register_handler("test-job", handler);

        let handle = dispatcher.dispatch(make_event("test-job"));
        assert!(handle.is_some());
        handle.unwrap().await.unwrap();

        let invocations = handler_ref.invocations.lock().await;
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0], "test-job");
    }

    #[tokio::test]
    async fn dispatch_unknown_job_returns_none() {
        let dispatcher = CronEventDispatcher::new();
        let handle = dispatcher.dispatch(make_event("unknown-job"));
        assert!(handle.is_none());
    }

    #[tokio::test]
    async fn two_simultaneous_dispatches_run_concurrently() {
        let counter = Arc::new(AtomicUsize::new(0));

        struct CountingHandler {
            counter: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CronEventHandler for CountingHandler {
            async fn handle(
                &self,
                _event: CronEventMessage,
            ) -> Result<HandlerOutcome, CronHandlerError> {
                self.counter.fetch_add(1, Ordering::SeqCst);
                // Small delay to ensure both are in flight
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(HandlerOutcome::Completed {
                    message: "done".to_string(),
                })
            }
        }

        let mut dispatcher = CronEventDispatcher::new();
        dispatcher.register_handler(
            "job-a",
            Arc::new(CountingHandler {
                counter: Arc::clone(&counter),
            }),
        );
        dispatcher.register_handler(
            "job-b",
            Arc::new(CountingHandler {
                counter: Arc::clone(&counter),
            }),
        );

        let h1 = dispatcher.dispatch(make_event("job-a")).unwrap();
        let h2 = dispatcher.dispatch(make_event("job-b")).unwrap();

        // Both should complete
        h1.await.unwrap();
        h2.await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn run_loop_processes_events() {
        let handler = Arc::new(RecordingHandler::new());
        let handler_ref = Arc::clone(&handler);

        let mut dispatcher = CronEventDispatcher::new();
        dispatcher.register_handler("loop-test", handler);

        let (tx, rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let run_handle = tokio::spawn(async move {
            dispatcher.run(rx, cancel_clone).await;
        });

        tx.send(make_event("loop-test")).await.unwrap();
        // Give the dispatcher time to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        cancel.cancel();
        run_handle.await.unwrap();

        let invocations = handler_ref.invocations.lock().await;
        assert_eq!(invocations.len(), 1);
    }

    #[tokio::test]
    async fn fast_handler_does_not_publish_progress() {
        tokio::time::pause();

        let handler = Arc::new(RecordingHandler::new()); // completes instantly

        let (progress_tx, mut progress_rx) = mpsc::channel::<ProgressUpdate>(16);

        let mut dispatcher = CronEventDispatcher::new();
        dispatcher.register_handler("fast-job", handler);
        dispatcher.set_progress_sender(progress_tx);

        let handle = dispatcher.dispatch(make_event("fast-job")).unwrap();

        // Advance a bit to let the handler complete
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;

        handle.await.unwrap();

        // No progress should have been sent
        assert!(
            progress_rx.try_recv().is_err(),
            "fast handler should not trigger progress"
        );
    }

    #[tokio::test]
    async fn slow_handler_publishes_progress_after_30s() {
        tokio::time::pause();

        let handler = Arc::new(RecordingHandler::with_delay(Duration::from_secs(35)));

        let (progress_tx, mut progress_rx) = mpsc::channel::<ProgressUpdate>(16);

        let mut dispatcher = CronEventDispatcher::new();
        dispatcher.register_handler("slow-job", handler);
        dispatcher.set_progress_sender(progress_tx);

        let _handle = dispatcher.dispatch(make_event("slow-job")).unwrap();

        // Yield to let the spawned task start and enter the select!
        tokio::task::yield_now().await;

        // Advance past the 30-second threshold
        tokio::time::advance(Duration::from_secs(31)).await;

        // Yield multiple times to let the spawned task process the timer
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        // Progress update should be available
        let progress = progress_rx.try_recv();
        assert!(progress.is_ok(), "progress should be published after 30s");
        let update = progress.unwrap();
        assert_eq!(update.job_name, "slow-job");
        assert!(update.message.contains("still running"));
    }

    #[tokio::test]
    async fn progress_does_not_cancel_handler() {
        tokio::time::pause();

        let handler = Arc::new(RecordingHandler::with_delay(Duration::from_secs(35)));
        let handler_ref = Arc::clone(&handler);

        let (progress_tx, _progress_rx) = mpsc::channel::<ProgressUpdate>(16);

        let mut dispatcher = CronEventDispatcher::new();
        dispatcher.register_handler("slow-job", handler);
        dispatcher.set_progress_sender(progress_tx);

        let handle = dispatcher.dispatch(make_event("slow-job")).unwrap();

        // Advance past the progress timeout AND past the handler's completion time
        tokio::time::advance(Duration::from_secs(36)).await;
        tokio::task::yield_now().await;

        handle.await.unwrap();

        // Handler should have completed (not been cancelled)
        let invocations = handler_ref.invocations.lock().await;
        assert_eq!(
            invocations.len(),
            1,
            "handler should complete even after progress timeout"
        );
    }

    #[tokio::test]
    async fn run_loop_respects_cancellation_token() {
        let dispatcher = CronEventDispatcher::new();
        let (_tx, rx) = mpsc::channel::<CronEventMessage>(16);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let run_handle = tokio::spawn(async move {
            dispatcher.run(rx, cancel_clone).await;
        });

        // Cancel immediately
        cancel.cancel();
        // Should complete without hanging
        tokio::time::timeout(Duration::from_secs(2), run_handle)
            .await
            .expect("dispatcher should shut down within timeout")
            .unwrap();
    }
}
