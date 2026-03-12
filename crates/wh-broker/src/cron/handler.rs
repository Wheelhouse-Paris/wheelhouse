//! CronEventHandler trait and related types — bootstrapped from Story 5-6.

use async_trait::async_trait;
use thiserror::Error;

use super::CronEventMessage;

/// Outcome of a successful handler execution.
#[derive(Debug, Clone)]
pub enum HandlerOutcome {
    /// Handler completed successfully with an optional message.
    Completed { message: String },
}

/// Typed errors for cron event handlers (SCV-04, SCV-01).
#[derive(Debug, Error)]
pub enum CronHandlerError {
    #[error("action not found for job '{job_name}'")]
    ActionNotFound { job_name: String },

    #[error("execution failed for job '{job_name}': {source}")]
    ExecutionFailed {
        job_name: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("handler timeout for job '{job_name}' after {elapsed}s")]
    Timeout { job_name: String, elapsed: u64 },
}

impl CronHandlerError {
    /// Error code in SCREAMING_SNAKE_CASE (SCV-01).
    pub fn code(&self) -> &'static str {
        match self {
            Self::ActionNotFound { .. } => "ACTION_NOT_FOUND",
            Self::ExecutionFailed { .. } => "EXECUTION_FAILED",
            Self::Timeout { .. } => "HANDLER_TIMEOUT",
        }
    }
}

/// Progress update emitted by the dispatcher for long-running actions.
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub job_name: String,
    pub message: String,
    pub percent: Option<u32>,
}

/// Async trait for handling cron events. Must be Send + Sync + 'static
/// because handlers are dispatched via tokio::spawn.
#[async_trait]
pub trait CronEventHandler: Send + Sync + 'static {
    async fn handle(&self, event: CronEventMessage) -> Result<HandlerOutcome, CronHandlerError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_screaming_snake_case() {
        let e1 = CronHandlerError::ActionNotFound {
            job_name: "test".into(),
        };
        assert_eq!(e1.code(), "ACTION_NOT_FOUND");

        let e2 = CronHandlerError::Timeout {
            job_name: "test".into(),
            elapsed: 30,
        };
        assert_eq!(e2.code(), "HANDLER_TIMEOUT");
    }

    #[test]
    fn error_display_includes_job_name() {
        let e = CronHandlerError::ActionNotFound {
            job_name: "daily-compaction".into(),
        };
        assert!(e.to_string().contains("daily-compaction"));
    }
}
