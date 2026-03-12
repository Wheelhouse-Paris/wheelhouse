//! Skill invocation domain types, proto conversions, and builders.
//!
//! This module provides domain types for skill invocations that wrap
//! the protobuf types for business logic, plus builder helpers for
//! constructing `SkillResult` and `SkillProgress` protobuf messages.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A skill invocation request (domain type wrapping protobuf `SkillInvocation`).
#[derive(Debug, Clone)]
pub struct SkillInvocationRequest {
    /// The skill to invoke.
    pub skill_name: String,
    /// The agent making the invocation.
    pub agent_id: String,
    /// Unique invocation identifier.
    pub invocation_id: String,
    /// Input parameters for the skill.
    pub parameters: HashMap<String, String>,
    /// Timestamp in milliseconds since epoch.
    pub timestamp_ms: i64,
}

/// The outcome of a skill invocation.
#[derive(Debug, Clone)]
pub enum SkillInvocationOutcome {
    /// Skill completed successfully.
    Success {
        /// Output produced by the skill.
        output: String,
    },
    /// Skill failed with an error.
    Error {
        /// Error code in SCREAMING_SNAKE_CASE (e.g., `SKILL_NOT_PERMITTED`).
        error_code: String,
        /// Human-readable error message.
        error_message: String,
    },
}

impl From<wh_proto::SkillInvocation> for SkillInvocationRequest {
    fn from(proto: wh_proto::SkillInvocation) -> Self {
        SkillInvocationRequest {
            skill_name: proto.skill_name,
            agent_id: proto.agent_id,
            invocation_id: proto.invocation_id,
            parameters: proto.parameters,
            timestamp_ms: proto.timestamp_ms,
        }
    }
}

/// Get current timestamp in milliseconds since epoch.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Build a `SkillProgress` protobuf message.
///
/// Used to indicate skill execution has started (CM-06) or provide
/// intermediate progress updates.
pub fn build_skill_progress(
    invocation_id: &str,
    skill_name: &str,
    progress_percent: f32,
    status_message: &str,
) -> wh_proto::SkillProgress {
    wh_proto::SkillProgress {
        invocation_id: invocation_id.to_string(),
        skill_name: skill_name.to_string(),
        progress_percent,
        status_message: status_message.to_string(),
        timestamp_ms: now_ms(),
    }
}

/// Build a `SkillResult` protobuf message for a successful invocation.
pub fn build_skill_result_success(
    invocation_id: &str,
    skill_name: &str,
    output: &str,
) -> wh_proto::SkillResult {
    wh_proto::SkillResult {
        invocation_id: invocation_id.to_string(),
        skill_name: skill_name.to_string(),
        success: true,
        output: output.to_string(),
        error_message: String::new(),
        error_code: String::new(),
        timestamp_ms: now_ms(),
    }
}

/// Build a `SkillResult` protobuf message for a failed invocation.
///
/// Error codes must be `SCREAMING_SNAKE_CASE` per SCV-01.
pub fn build_skill_result_error(
    invocation_id: &str,
    skill_name: &str,
    error_code: &str,
    error_message: &str,
) -> wh_proto::SkillResult {
    wh_proto::SkillResult {
        invocation_id: invocation_id.to_string(),
        skill_name: skill_name.to_string(),
        success: false,
        output: String::new(),
        error_message: error_message.to_string(),
        error_code: error_code.to_string(),
        timestamp_ms: now_ms(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_proto_conversion() {
        let proto = wh_proto::SkillInvocation {
            skill_name: "summarize".into(),
            agent_id: "agent-1".into(),
            invocation_id: "inv-001".into(),
            parameters: {
                let mut m = HashMap::new();
                m.insert("text".into(), "hello world".into());
                m
            },
            timestamp_ms: 1710000000000,
        };
        let req: SkillInvocationRequest = proto.into();
        assert_eq!(req.skill_name, "summarize");
        assert_eq!(req.agent_id, "agent-1");
        assert_eq!(req.invocation_id, "inv-001");
        assert_eq!(req.parameters.get("text").unwrap(), "hello world");
        assert_eq!(req.timestamp_ms, 1710000000000);
    }

    #[test]
    fn outcome_success_variant() {
        let outcome = SkillInvocationOutcome::Success {
            output: "done".into(),
        };
        assert!(matches!(outcome, SkillInvocationOutcome::Success { .. }));
    }

    #[test]
    fn outcome_error_variant() {
        let outcome = SkillInvocationOutcome::Error {
            error_code: "SKILL_NOT_PERMITTED".into(),
            error_message: "not allowed".into(),
        };
        match outcome {
            SkillInvocationOutcome::Error {
                error_code,
                error_message,
            } => {
                assert_eq!(error_code, "SKILL_NOT_PERMITTED");
                assert_eq!(error_message, "not allowed");
            }
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn build_progress_sets_timestamp() {
        let p = build_skill_progress("inv-1", "summarize", 0.5, "halfway");
        assert_eq!(p.invocation_id, "inv-1");
        assert_eq!(p.skill_name, "summarize");
        assert!((p.progress_percent - 0.5).abs() < f32::EPSILON);
        assert_eq!(p.status_message, "halfway");
        assert!(p.timestamp_ms > 0);
    }

    #[test]
    fn build_result_success_fields() {
        let r = build_skill_result_success("inv-2", "summarize", "output text");
        assert_eq!(r.invocation_id, "inv-2");
        assert_eq!(r.skill_name, "summarize");
        assert!(r.success);
        assert_eq!(r.output, "output text");
        assert!(r.error_message.is_empty());
        assert!(r.error_code.is_empty());
        assert!(r.timestamp_ms > 0);
    }

    #[test]
    fn build_result_error_fields() {
        let r = build_skill_result_error("inv-3", "web-search", "SKILL_NOT_PERMITTED", "denied");
        assert_eq!(r.invocation_id, "inv-3");
        assert_eq!(r.skill_name, "web-search");
        assert!(!r.success);
        assert!(r.output.is_empty());
        assert_eq!(r.error_code, "SKILL_NOT_PERMITTED");
        assert_eq!(r.error_message, "denied");
        assert!(r.timestamp_ms > 0);
    }
}
