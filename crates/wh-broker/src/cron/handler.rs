//! CronEvent handler trait and supporting types.
//!
//! Defines the `CronEventHandler` trait that allows registering custom
//! actions to be executed when specific CronEvents arrive.

use std::time::Duration;

use super::CronEventMessage;

/// Outcome of a successfully handled CronEvent.
#[derive(Debug)]
pub enum HandlerOutcome {
    /// The handler completed its work.
    Completed {
        /// Human-readable summary of what was done.
        message: String,
    },
}

/// A progress update published by the dispatcher when a handler exceeds
/// the 30-second threshold. The broker's main loop is responsible for
/// translating this into a proto type (e.g., `TextMessage`) before
/// publishing to the stream.
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    /// Name of the cron job being processed.
    pub job_name: String,
    /// Human-readable progress message.
    pub message: String,
    /// Optional completion percentage (0-100).
    pub percent: Option<u32>,
}

/// Typed errors for CronEvent handler failures.
///
/// Error codes follow SCV-01 SCREAMING_SNAKE_CASE convention.
#[derive(Debug, thiserror::Error)]
pub enum CronHandlerError {
    /// No handler registered for the given job name.
    #[error("no handler registered for cron job '{job_name}'")]
    ActionNotFound {
        /// The job name that was not found.
        job_name: String,
    },

    /// Handler execution failed.
    #[error("handler execution failed for cron job '{job_name}': {source}")]
    ExecutionFailed {
        /// The job name that failed.
        job_name: String,
        /// The underlying error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Handler exceeded its timeout.
    #[error("handler timed out for cron job '{job_name}' after {elapsed:?}")]
    Timeout {
        /// The job name that timed out.
        job_name: String,
        /// How long the handler ran before being timed out.
        elapsed: Duration,
    },
}

impl CronHandlerError {
    /// Returns the SCREAMING_SNAKE_CASE error code per SCV-01.
    pub fn code(&self) -> &'static str {
        match self {
            CronHandlerError::ActionNotFound { .. } => "ACTION_NOT_FOUND",
            CronHandlerError::ExecutionFailed { .. } => "EXECUTION_FAILED",
            CronHandlerError::Timeout { .. } => "HANDLER_TIMEOUT",
        }
    }
}

/// Trait for handling CronEvent messages.
///
/// Implementations must be `Send + Sync + 'static` since handlers are
/// dispatched via `tokio::spawn` across threads.
#[async_trait::async_trait]
pub trait CronEventHandler: Send + Sync + 'static {
    /// Handle a CronEvent message.
    ///
    /// Returns `HandlerOutcome::Completed` when the action finishes.
    /// Errors are logged by the dispatcher and do not crash the system.
    async fn handle(&self, event: CronEventMessage) -> Result<HandlerOutcome, CronHandlerError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_action_not_found() {
        let err = CronHandlerError::ActionNotFound {
            job_name: "daily-compaction".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "no handler registered for cron job 'daily-compaction'"
        );
        assert_eq!(err.code(), "ACTION_NOT_FOUND");
    }

    #[test]
    fn error_display_execution_failed() {
        let source: Box<dyn std::error::Error + Send + Sync> = "disk full".to_string().into();
        let err = CronHandlerError::ExecutionFailed {
            job_name: "nightly-backup".to_string(),
            source,
        };
        assert!(err.to_string().contains("nightly-backup"));
        assert!(err.to_string().contains("disk full"));
        assert_eq!(err.code(), "EXECUTION_FAILED");
    }

    #[test]
    fn error_display_timeout() {
        let err = CronHandlerError::Timeout {
            job_name: "slow-task".to_string(),
            elapsed: Duration::from_secs(60),
        };
        assert!(err.to_string().contains("slow-task"));
        assert!(err.to_string().contains("60"));
        assert_eq!(err.code(), "HANDLER_TIMEOUT");
    }

    #[test]
    fn error_codes_are_screaming_snake_case() {
        let cases = vec![
            CronHandlerError::ActionNotFound {
                job_name: "x".to_string(),
            },
            CronHandlerError::ExecutionFailed {
                job_name: "x".to_string(),
                source: "err".to_string().into(),
            },
            CronHandlerError::Timeout {
                job_name: "x".to_string(),
                elapsed: Duration::from_secs(1),
            },
        ];

        for err in &cases {
            let code = err.code();
            // Verify SCREAMING_SNAKE_CASE: all uppercase, underscores allowed, no lowercase
            assert!(
                code.chars().all(|c| c.is_ascii_uppercase() || c == '_'),
                "error code '{code}' is not SCREAMING_SNAKE_CASE"
            );
        }
    }
}
