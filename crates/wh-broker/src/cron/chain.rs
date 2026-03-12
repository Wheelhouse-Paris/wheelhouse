//! Chain types for the cron -> skill invocation end-to-end flow.
//!
//! Defines the event log, outcome, error, and surface notification types
//! used by the CronSkillChain orchestrator.

use std::collections::HashMap;
use thiserror::Error;

/// Events emitted during the cron -> skill invocation chain.
/// All variants include `timestamp_ms` for AC#2 ordering verification.
#[derive(Debug, Clone)]
pub enum ChainEvent {
    /// Cron event received from the scheduler.
    CronEventReceived { job_name: String, timestamp_ms: i64 },
    /// SkillInvocation published to the stream.
    SkillInvocationPublished {
        invocation_id: String,
        skill_name: String,
        timestamp_ms: i64,
    },
    /// SkillProgress published (CM-06 compliance).
    SkillProgressPublished {
        invocation_id: String,
        percent: u32,
        message: String,
        timestamp_ms: i64,
    },
    /// SkillResult received (terminal event for skill execution).
    SkillResultReceived {
        invocation_id: String,
        success: bool,
        output_or_error: String,
        timestamp_ms: i64,
    },
    /// TextMessage published as chain summary or error report.
    TextMessagePublished { content: String, timestamp_ms: i64 },
}

impl ChainEvent {
    /// Returns the event type name (for format_chain_log).
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::CronEventReceived { .. } => "CronEvent",
            Self::SkillInvocationPublished { .. } => "SkillInvocation",
            Self::SkillProgressPublished { .. } => "SkillProgress",
            Self::SkillResultReceived { .. } => "SkillResult",
            Self::TextMessagePublished { .. } => "TextMessage",
        }
    }

    /// Returns the timestamp_ms for this event.
    pub fn timestamp_ms(&self) -> i64 {
        match self {
            Self::CronEventReceived { timestamp_ms, .. }
            | Self::SkillInvocationPublished { timestamp_ms, .. }
            | Self::SkillProgressPublished { timestamp_ms, .. }
            | Self::SkillResultReceived { timestamp_ms, .. }
            | Self::TextMessagePublished { timestamp_ms, .. } => *timestamp_ms,
        }
    }
}

/// Outcome of a successful chain execution.
#[derive(Debug, Clone)]
pub struct ChainOutcome {
    /// Ordered list of events in the chain.
    pub events: Vec<ChainEvent>,
    /// Whether the chain completed successfully.
    pub success: bool,
    /// Human-readable summary for TextMessage content.
    pub summary_text: String,
}

/// Error during chain execution (separate from ChainEvent).
#[derive(Debug, Error)]
pub enum ChainError {
    #[error("chain dispatch failed for job '{job_name}': {reason}")]
    DispatchFailed { job_name: String, reason: String },

    #[error("skill invocation failed for job '{job_name}': {reason}")]
    InvocationFailed { job_name: String, reason: String },

    #[error("channel closed unexpectedly during chain for job '{job_name}'")]
    ChannelClosed { job_name: String },
}

impl ChainError {
    /// Error code in SCREAMING_SNAKE_CASE (SCV-01).
    pub fn code(&self) -> &'static str {
        match self {
            Self::DispatchFailed { .. } => "CHAIN_DISPATCH_FAILED",
            Self::InvocationFailed { .. } => "SKILL_INVOCATION_FAILED",
            Self::ChannelClosed { .. } => "CHAIN_CHANNEL_CLOSED",
        }
    }
}

/// Type of surface notification.
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationType {
    SkillFailure,
    CronChainError,
}

/// Notification sent to operator surfaces on chain failures (AC#3).
#[derive(Debug, Clone)]
pub struct SurfaceNotification {
    pub notification_type: NotificationType,
    pub message: String,
    pub metadata: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_event_type_names_are_correct() {
        let e = ChainEvent::CronEventReceived {
            job_name: "test".into(),
            timestamp_ms: 100,
        };
        assert_eq!(e.type_name(), "CronEvent");

        let e = ChainEvent::SkillInvocationPublished {
            invocation_id: "inv-1".into(),
            skill_name: "echo".into(),
            timestamp_ms: 200,
        };
        assert_eq!(e.type_name(), "SkillInvocation");

        let e = ChainEvent::SkillProgressPublished {
            invocation_id: "inv-1".into(),
            percent: 0,
            message: "started".into(),
            timestamp_ms: 300,
        };
        assert_eq!(e.type_name(), "SkillProgress");

        let e = ChainEvent::SkillResultReceived {
            invocation_id: "inv-1".into(),
            success: true,
            output_or_error: "ok".into(),
            timestamp_ms: 400,
        };
        assert_eq!(e.type_name(), "SkillResult");

        let e = ChainEvent::TextMessagePublished {
            content: "summary".into(),
            timestamp_ms: 500,
        };
        assert_eq!(e.type_name(), "TextMessage");
    }

    #[test]
    fn chain_event_timestamps_are_accessible() {
        let e = ChainEvent::CronEventReceived {
            job_name: "test".into(),
            timestamp_ms: 42,
        };
        assert_eq!(e.timestamp_ms(), 42);
    }

    #[test]
    fn chain_error_codes_are_screaming_snake_case() {
        let e = ChainError::DispatchFailed {
            job_name: "test".into(),
            reason: "no handler".into(),
        };
        assert_eq!(e.code(), "CHAIN_DISPATCH_FAILED");

        let e = ChainError::InvocationFailed {
            job_name: "test".into(),
            reason: "timeout".into(),
        };
        assert_eq!(e.code(), "SKILL_INVOCATION_FAILED");
    }
}
