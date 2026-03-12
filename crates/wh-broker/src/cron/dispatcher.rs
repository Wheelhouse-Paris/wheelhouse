//! CronEventDispatcher — bootstrapped from Story 5-6.
//! Dispatches CronEvent messages to registered handlers via tokio::spawn.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{error, warn};

use super::handler::CronEventHandler;
use super::CronEventMessage;

/// Dispatches cron events to registered handlers concurrently.
pub struct CronEventDispatcher {
    handlers: HashMap<String, Arc<dyn CronEventHandler>>,
}

impl CronEventDispatcher {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a specific job name.
    pub fn register_handler(&mut self, job_name: &str, handler: Arc<dyn CronEventHandler>) {
        self.handlers.insert(job_name.to_string(), handler);
    }

    /// Dispatch a cron event to its registered handler.
    /// Returns a JoinHandle if a handler was found, None otherwise.
    #[tracing::instrument(skip_all, fields(job_name = %event.job_name))]
    pub fn dispatch(&self, event: CronEventMessage) -> Option<JoinHandle<()>> {
        let handler = match self.handlers.get(&event.job_name) {
            Some(h) => Arc::clone(h),
            None => {
                warn!(job_name = %event.job_name, "no handler registered for cron job");
                return None;
            }
        };

        let job_name = event.job_name.clone();
        let handle = tokio::spawn(async move {
            match handler.handle(event).await {
                Ok(outcome) => {
                    tracing::info!(job_name = %job_name, ?outcome, "cron handler completed");
                }
                Err(e) => {
                    error!(
                        job_name = %job_name,
                        error_code = %e.code(),
                        error = %e,
                        "cron handler failed"
                    );
                }
            }
        });

        Some(handle)
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
    use async_trait::async_trait;

    struct TestHandler;

    #[async_trait]
    impl CronEventHandler for TestHandler {
        async fn handle(
            &self,
            _event: CronEventMessage,
        ) -> Result<HandlerOutcome, CronHandlerError> {
            Ok(HandlerOutcome::Completed {
                message: "done".into(),
            })
        }
    }

    #[tokio::test]
    async fn dispatch_routes_to_correct_handler() {
        let mut dispatcher = CronEventDispatcher::new();
        dispatcher.register_handler("test-job", Arc::new(TestHandler));

        let event = CronEventMessage {
            job_name: "test-job".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            payload: HashMap::new(),
        };

        let handle = dispatcher.dispatch(event);
        assert!(handle.is_some());
        handle.unwrap().await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_unknown_job_returns_none() {
        let dispatcher = CronEventDispatcher::new();
        let event = CronEventMessage {
            job_name: "unknown".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            payload: HashMap::new(),
        };

        let handle = dispatcher.dispatch(event);
        assert!(handle.is_none());
    }
}
