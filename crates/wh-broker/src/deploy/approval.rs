//! Approval module for human validation threshold system.
//!
//! Provides types and logic for managing human approval requests
//! when autonomous changes exceed the configured threshold.
//!
//! The broker library is stateless — the agent runtime (Python SDK)
//! is responsible for publishing approval requests and tracking pending approvals.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Default approval timeout: 24 hours (86400 seconds).
pub const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 86400;

/// A pending approval request awaiting human response.
///
/// Created when an autonomous change exceeds the configured threshold.
/// The agent runtime tracks these and checks for expiry.
#[derive(Debug)]
pub struct PendingApproval {
    /// Unique identifier for this approval request.
    pub id: String,
    /// Name of the agent that proposed the change.
    pub agent_name: String,
    /// When the approval was requested (monotonic clock for timeout).
    pub requested_at: Instant,
    /// How long to wait for a response before expiring.
    pub timeout: Duration,
    /// Path to the `.wh` file that would be modified.
    pub wh_path: PathBuf,
}

/// A formatted approval request to send to the operator via surface.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// What change is being proposed.
    pub what: String,
    /// Why the change is being proposed.
    pub why: String,
    /// The classified impact level (e.g. "High", "Medium").
    pub impact_level: String,
    /// Instructions for the operator on how to respond.
    pub instruction: String,
}

/// The operator's response to an approval request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalResponse {
    /// The operator approved the change.
    Approved,
    /// The operator rejected the change.
    Rejected,
    /// The response was not recognized as approval or rejection.
    Unrecognized(String),
}

/// Errors related to the approval process.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("approval request timed out: {0}")]
    Timeout(String),

    #[error("approval request rejected: {0}")]
    Rejected(String),

    #[error("invalid approval response: {0}")]
    InvalidResponse(String),
}

impl ApprovalError {
    /// Returns the error code string in SCREAMING_SNAKE_CASE (NP-01).
    pub fn code(&self) -> &'static str {
        match self {
            ApprovalError::Timeout(_) => "APPROVAL_TIMEOUT",
            ApprovalError::Rejected(_) => "APPROVAL_REJECTED",
            ApprovalError::InvalidResponse(_) => "INVALID_APPROVAL_RESPONSE",
        }
    }
}

/// Parse an operator's text response into an `ApprovalResponse`.
///
/// Recognized approval signals (case-insensitive): "yes", "approve", "approved", "ok"
/// Recognized rejection signals (case-insensitive): "no", "reject", "rejected", "deny", "denied"
/// Everything else is `Unrecognized`.
#[tracing::instrument(skip_all)]
pub fn parse_approval_response(text: &str) -> ApprovalResponse {
    let normalized = text.trim().to_lowercase();
    match normalized.as_str() {
        "yes" | "approve" | "approved" | "ok" => ApprovalResponse::Approved,
        "no" | "reject" | "rejected" | "deny" | "denied" => ApprovalResponse::Rejected,
        _ => ApprovalResponse::Unrecognized(text.to_string()),
    }
}

/// Check whether a pending approval request has expired.
#[tracing::instrument(skip_all)]
pub fn is_expired(pending: &PendingApproval) -> bool {
    pending.requested_at.elapsed() >= pending.timeout
}

/// Format an approval request as a human-readable text message.
#[tracing::instrument(skip_all)]
pub fn format_approval_message(request: &ApprovalRequest) -> String {
    format!(
        "APPROVAL REQUIRED\n\
         Impact: {}\n\
         Change: {}\n\
         Reason: {}\n\
         \n\
         {}",
        request.impact_level, request.what, request.why, request.instruction
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_approval_yes_variants() {
        assert_eq!(parse_approval_response("yes"), ApprovalResponse::Approved);
        assert_eq!(parse_approval_response("YES"), ApprovalResponse::Approved);
        assert_eq!(
            parse_approval_response("approve"),
            ApprovalResponse::Approved
        );
        assert_eq!(
            parse_approval_response("Approved"),
            ApprovalResponse::Approved
        );
        assert_eq!(parse_approval_response("ok"), ApprovalResponse::Approved);
        assert_eq!(parse_approval_response("OK"), ApprovalResponse::Approved);
    }

    #[test]
    fn parse_approval_no_variants() {
        assert_eq!(parse_approval_response("no"), ApprovalResponse::Rejected);
        assert_eq!(parse_approval_response("NO"), ApprovalResponse::Rejected);
        assert_eq!(
            parse_approval_response("reject"),
            ApprovalResponse::Rejected
        );
        assert_eq!(
            parse_approval_response("rejected"),
            ApprovalResponse::Rejected
        );
        assert_eq!(parse_approval_response("deny"), ApprovalResponse::Rejected);
        assert_eq!(
            parse_approval_response("denied"),
            ApprovalResponse::Rejected
        );
    }

    #[test]
    fn parse_approval_unrecognized() {
        assert!(matches!(
            parse_approval_response("maybe later"),
            ApprovalResponse::Unrecognized(_)
        ));
        assert!(matches!(
            parse_approval_response("hello world"),
            ApprovalResponse::Unrecognized(_)
        ));
    }

    #[test]
    fn parse_approval_trims_whitespace() {
        assert_eq!(
            parse_approval_response("  yes  "),
            ApprovalResponse::Approved
        );
        assert_eq!(
            parse_approval_response("  no  "),
            ApprovalResponse::Rejected
        );
    }

    #[test]
    fn is_expired_returns_true_after_timeout() {
        let pending = PendingApproval {
            id: "test-1".to_string(),
            agent_name: "donna".to_string(),
            requested_at: Instant::now() - Duration::from_secs(100),
            timeout: Duration::from_secs(60),
            wh_path: PathBuf::from("/tmp/test.wh"),
        };
        assert!(is_expired(&pending));
    }

    #[test]
    fn is_expired_returns_false_within_timeout() {
        let pending = PendingApproval {
            id: "test-2".to_string(),
            agent_name: "donna".to_string(),
            requested_at: Instant::now(),
            timeout: Duration::from_secs(3600),
            wh_path: PathBuf::from("/tmp/test.wh"),
        };
        assert!(!is_expired(&pending));
    }

    #[test]
    fn error_codes_are_screaming_snake_case() {
        assert_eq!(
            ApprovalError::Timeout("t".into()).code(),
            "APPROVAL_TIMEOUT"
        );
        assert_eq!(
            ApprovalError::Rejected("r".into()).code(),
            "APPROVAL_REJECTED"
        );
        assert_eq!(
            ApprovalError::InvalidResponse("i".into()).code(),
            "INVALID_APPROVAL_RESPONSE"
        );
    }

    #[test]
    fn format_approval_message_includes_all_fields() {
        let request = ApprovalRequest {
            what: "Scale researcher from 1 to 3".to_string(),
            why: "High error rate".to_string(),
            impact_level: "High".to_string(),
            instruction: "Reply 'yes' to approve".to_string(),
        };
        let msg = format_approval_message(&request);
        assert!(msg.contains("High"));
        assert!(msg.contains("Scale researcher"));
        assert!(msg.contains("High error rate"));
        assert!(msg.contains("Reply 'yes'"));
    }
}
