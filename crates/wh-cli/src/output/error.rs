//! WhError typed hierarchy (ADR-014)
//!
//! Structured error system with numeric error codes, human-readable messages,
//! and contextual fields. Every error code is documented in ERRORS.md (NFR-D3).
//!
//! # JSON Error Envelope
//!
//! ```json
//! {
//!   "v": 1,
//!   "status": "error",
//!   "code": 1001,
//!   "error_name": "CONNECTION_ERROR",
//!   "message": "Wheelhouse not running",
//!   "context": { "file": null, "line": null, "field": null }
//! }
//! ```
//!
//! # Human-Readable Format
//!
//! ```text
//! Error [WH-1001]: Wheelhouse not running
//! ```

use serde::ser::SerializeMap;
use serde::Serialize;
use std::fmt;

/// Contextual information about where an error occurred.
///
/// All fields are optional — populate whichever are relevant to the error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ErrorContext {
    /// Source file where the error was detected (e.g., "topology.wh")
    pub file: Option<String>,
    /// Line number in the source file
    pub line: Option<u32>,
    /// Field name that caused the error (e.g., "replicas")
    pub field: Option<String>,
}

/// Sub-categories for deploy-related errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployErrorKind {
    /// Lint validation failure
    LintError,
    /// Plan generation failure
    PlanError,
    /// Apply execution failure
    ApplyError,
}

/// Typed error hierarchy for the Wheelhouse CLI (ADR-014).
///
/// Each variant maps to a unique numeric code (WH-XXXX) and a process exit code.
/// User-facing messages never mention "broker" (RT-B1).
#[derive(Debug, Clone)]
pub enum WhError {
    /// Cannot connect to Wheelhouse. Code: WH-1001.
    ConnectionError {
        message: String,
        context: ErrorContext,
    },
    /// Deploy operation failure. Code: WH-2001 (lint), WH-2002 (plan), WH-2003 (apply).
    DeployError {
        kind: DeployErrorKind,
        message: String,
        context: ErrorContext,
    },
    /// Stream operation failure. Code: WH-3001.
    StreamError {
        message: String,
        context: ErrorContext,
    },
    /// Configuration error. Code: WH-4001.
    ConfigError {
        message: String,
        context: ErrorContext,
    },
    /// Internal/unexpected error. Code: WH-9001.
    InternalError {
        message: String,
        context: ErrorContext,
    },
}

impl WhError {
    /// Returns the unique numeric error code for this error variant.
    ///
    /// Error code ranges:
    /// - 1xxx: Connection errors
    /// - 2xxx: Deploy errors (lint/plan/apply)
    /// - 3xxx: Stream errors
    /// - 4xxx: Configuration errors
    /// - 9xxx: Internal errors
    pub fn error_code(&self) -> u32 {
        match self {
            WhError::ConnectionError { .. } => 1001,
            WhError::DeployError { kind, .. } => match kind {
                DeployErrorKind::LintError => 2001,
                DeployErrorKind::PlanError => 2002,
                DeployErrorKind::ApplyError => 2003,
            },
            WhError::StreamError { .. } => 3001,
            WhError::ConfigError { .. } => 4001,
            WhError::InternalError { .. } => 9001,
        }
    }

    /// Returns the string name of the error for the JSON envelope.
    pub fn error_name(&self) -> &'static str {
        match self {
            WhError::ConnectionError { .. } => "CONNECTION_ERROR",
            WhError::DeployError { kind, .. } => match kind {
                DeployErrorKind::LintError => "LINT_ERROR",
                DeployErrorKind::PlanError => "PLAN_ERROR",
                DeployErrorKind::ApplyError => "APPLY_ERROR",
            },
            WhError::StreamError { .. } => "STREAM_ERROR",
            WhError::ConfigError { .. } => "CONFIG_ERROR",
            WhError::InternalError { .. } => "INTERNAL_ERROR",
        }
    }

    /// Returns the process exit code for this error.
    ///
    /// Per ADR-014: 0 = success, 1 = error, 2 = plan change detected.
    /// All error variants return 1.
    pub fn exit_code(&self) -> i32 {
        1
    }

    /// Returns the human-readable message.
    pub fn message(&self) -> &str {
        match self {
            WhError::ConnectionError { message, .. }
            | WhError::DeployError { message, .. }
            | WhError::StreamError { message, .. }
            | WhError::ConfigError { message, .. }
            | WhError::InternalError { message, .. } => message,
        }
    }

    /// Returns a reference to the error context.
    pub fn context(&self) -> &ErrorContext {
        match self {
            WhError::ConnectionError { context, .. }
            | WhError::DeployError { context, .. }
            | WhError::StreamError { context, .. }
            | WhError::ConfigError { context, .. }
            | WhError::InternalError { context, .. } => context,
        }
    }
}

/// Display implementation produces human-readable format:
/// `Error [WH-XXXX]: <message>`
/// With optional context lines for file/line/field.
impl fmt::Display for WhError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error [WH-{}]: {}", self.error_code(), self.message())?;

        let ctx = self.context();
        if let Some(ref file) = ctx.file {
            write!(f, "\n  file: {}", file)?;
        }
        if let Some(line) = ctx.line {
            write!(f, "\n  line: {}", line)?;
        }
        if let Some(ref field) = ctx.field {
            write!(f, "\n  field: {}", field)?;
        }
        Ok(())
    }
}

impl std::error::Error for WhError {}

/// Custom Serialize implementation to produce the JSON error envelope:
/// `{ "v": 1, "status": "error", "code": <numeric>, "error_name": "...", "message": "...", "context": {...} }`
impl Serialize for WhError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(6))?;
        map.serialize_entry("v", &1u32)?;
        map.serialize_entry("status", "error")?;
        map.serialize_entry("code", &self.error_code())?;
        map.serialize_entry("error_name", self.error_name())?;
        map.serialize_entry("message", self.message())?;
        map.serialize_entry("context", self.context())?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_error_code() {
        let err = WhError::ConnectionError {
            message: "Wheelhouse not running".into(),
            context: ErrorContext::default(),
        };
        assert_eq!(err.error_code(), 1001);
    }

    #[test]
    fn test_deploy_lint_error_code() {
        let err = WhError::DeployError {
            kind: DeployErrorKind::LintError,
            message: "".into(),
            context: ErrorContext::default(),
        };
        assert_eq!(err.error_code(), 2001);
    }

    #[test]
    fn test_deploy_plan_error_code() {
        let err = WhError::DeployError {
            kind: DeployErrorKind::PlanError,
            message: "".into(),
            context: ErrorContext::default(),
        };
        assert_eq!(err.error_code(), 2002);
    }

    #[test]
    fn test_deploy_apply_error_code() {
        let err = WhError::DeployError {
            kind: DeployErrorKind::ApplyError,
            message: "".into(),
            context: ErrorContext::default(),
        };
        assert_eq!(err.error_code(), 2003);
    }

    #[test]
    fn test_stream_error_code() {
        let err = WhError::StreamError {
            message: "".into(),
            context: ErrorContext::default(),
        };
        assert_eq!(err.error_code(), 3001);
    }

    #[test]
    fn test_config_error_code() {
        let err = WhError::ConfigError {
            message: "".into(),
            context: ErrorContext::default(),
        };
        assert_eq!(err.error_code(), 4001);
    }

    #[test]
    fn test_internal_error_code() {
        let err = WhError::InternalError {
            message: "".into(),
            context: ErrorContext::default(),
        };
        assert_eq!(err.error_code(), 9001);
    }

    #[test]
    fn test_all_exit_codes_are_one() {
        let errors = vec![
            WhError::ConnectionError {
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::DeployError {
                kind: DeployErrorKind::LintError,
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::StreamError {
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::ConfigError {
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::InternalError {
                message: "".into(),
                context: ErrorContext::default(),
            },
        ];
        for err in &errors {
            assert_eq!(err.exit_code(), 1);
        }
    }

    #[test]
    fn test_json_serialization_envelope() {
        let err = WhError::ConnectionError {
            message: "Wheelhouse not running".into(),
            context: ErrorContext::default(),
        };
        let json_str = serde_json::to_string(&err).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(v["v"], 1);
        assert_eq!(v["status"], "error");
        assert_eq!(v["code"], 1001);
        assert_eq!(v["error_name"], "CONNECTION_ERROR");
        assert_eq!(v["message"], "Wheelhouse not running");
        assert!(v["context"].is_object());
    }

    #[test]
    fn test_json_serialization_with_context() {
        let err = WhError::DeployError {
            kind: DeployErrorKind::LintError,
            message: "invalid field".into(),
            context: ErrorContext {
                file: Some("topology.wh".into()),
                line: Some(42),
                field: Some("replicas".into()),
            },
        };
        let json_str = serde_json::to_string(&err).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(v["context"]["file"], "topology.wh");
        assert_eq!(v["context"]["line"], 42);
        assert_eq!(v["context"]["field"], "replicas");
    }

    #[test]
    fn test_json_serialization_null_context() {
        let err = WhError::StreamError {
            message: "stream not found".into(),
            context: ErrorContext::default(),
        };
        let json_str = serde_json::to_string(&err).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert!(v["context"]["file"].is_null());
        assert!(v["context"]["line"].is_null());
        assert!(v["context"]["field"].is_null());
    }

    #[test]
    fn test_display_format() {
        let err = WhError::ConnectionError {
            message: "Wheelhouse not running".into(),
            context: ErrorContext::default(),
        };
        let display = format!("{}", err);
        assert_eq!(display, "Error [WH-1001]: Wheelhouse not running");
    }

    #[test]
    fn test_display_format_with_context() {
        let err = WhError::DeployError {
            kind: DeployErrorKind::LintError,
            message: "invalid field 'replicas'".into(),
            context: ErrorContext {
                file: Some("topology.wh".into()),
                line: Some(42),
                field: Some("replicas".into()),
            },
        };
        let display = format!("{}", err);
        assert!(display.contains("WH-2001"));
        assert!(display.contains("invalid field 'replicas'"));
        assert!(display.contains("file: topology.wh"));
        assert!(display.contains("line: 42"));
        assert!(display.contains("field: replicas"));
    }

    #[test]
    fn test_no_broker_in_error_names() {
        let names = [
            "CONNECTION_ERROR",
            "LINT_ERROR",
            "PLAN_ERROR",
            "APPLY_ERROR",
            "STREAM_ERROR",
            "CONFIG_ERROR",
            "INTERNAL_ERROR",
        ];
        for name in &names {
            assert!(
                !name.to_lowercase().contains("broker"),
                "Error name must not contain 'broker': {}",
                name
            );
        }
    }

    #[test]
    fn test_unique_error_codes() {
        let errors = vec![
            WhError::ConnectionError {
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::DeployError {
                kind: DeployErrorKind::LintError,
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::DeployError {
                kind: DeployErrorKind::PlanError,
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::DeployError {
                kind: DeployErrorKind::ApplyError,
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::StreamError {
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::ConfigError {
                message: "".into(),
                context: ErrorContext::default(),
            },
            WhError::InternalError {
                message: "".into(),
                context: ErrorContext::default(),
            },
        ];

        let mut codes: Vec<u32> = errors.iter().map(|e| e.error_code()).collect();
        let original_len = codes.len();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), original_len, "all error codes must be unique");
    }
}
