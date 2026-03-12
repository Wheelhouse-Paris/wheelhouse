//! Acceptance tests for Story 3.5: Structured Error System and ERRORS.md
//!
//! These tests verify the WhError typed hierarchy, JSON serialization,
//! human-readable Display output, and ERRORS.md documentation coverage.

use wh_cli::output::error::{DeployErrorKind, ErrorContext, WhError};
use wh_cli::output::{render_error, OutputFormat};

// --- AC#1: Every error has numeric code, message, and context ---

#[test]
fn test_connection_error_has_numeric_code() {
    let err = WhError::ConnectionError {
        message: "Wheelhouse not running".to_string(),
        context: ErrorContext::default(),
    };
    assert_eq!(err.error_code(), 1001);
}

#[test]
fn test_connection_error_exit_code_is_one() {
    let err = WhError::ConnectionError {
        message: "Wheelhouse not running".to_string(),
        context: ErrorContext::default(),
    };
    assert_eq!(err.exit_code(), 1);
}

// --- AC#2: Deploy lint errors include context fields; codes in ERRORS.md ---

#[test]
fn test_deploy_lint_error_json_has_context_fields() {
    let err = WhError::DeployError {
        kind: DeployErrorKind::LintError,
        message: "invalid field 'replicas'".to_string(),
        context: ErrorContext {
            file: Some("topology.wh".to_string()),
            line: Some(42),
            field: Some("replicas".to_string()),
        },
    };

    let json_str = serde_json::to_string(&err).expect("serialization must succeed");
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(v["context"]["file"], "topology.wh");
    assert_eq!(v["context"]["line"], 42);
    assert_eq!(v["context"]["field"], "replicas");
}

#[test]
fn test_all_error_codes_documented_in_errors_md() {
    let errors_md =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ERRORS.md"))
            .expect("ERRORS.md must exist at repo root");

    // Every WhError variant's code must appear in ERRORS.md
    let codes = [1001u32, 2001, 2002, 2003, 3001, 4001, 9001];
    for code in &codes {
        let code_str = format!("WH-{}", code);
        assert!(
            errors_md.contains(&code_str),
            "ERRORS.md must document error code {}",
            code_str
        );
    }
}

// --- AC#3: JSON output has all error info as first-class fields ---

#[test]
fn test_json_error_envelope_format() {
    let err = WhError::ConnectionError {
        message: "Wheelhouse not running".to_string(),
        context: ErrorContext::default(),
    };

    let json_str = serde_json::to_string(&err).expect("serialization must succeed");
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(v["v"], 1, "envelope must have version 1");
    assert_eq!(v["status"], "error", "envelope status must be 'error'");
    assert!(v["code"].is_number(), "code must be numeric");
    assert!(v["message"].is_string(), "message must be a string");
    assert!(v["context"].is_object(), "context must be an object");
}

#[test]
fn test_json_error_fields_are_first_class() {
    let err = WhError::DeployError {
        kind: DeployErrorKind::PlanError,
        message: "plan failed".to_string(),
        context: ErrorContext {
            file: Some("deploy.wh".to_string()),
            line: None,
            field: None,
        },
    };

    let json_str = serde_json::to_string(&err).expect("serialization must succeed");
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    // All fields must be directly accessible without string parsing
    assert_eq!(v["code"], 2002);
    assert_eq!(v["message"], "plan failed");
    assert_eq!(v["context"]["file"], "deploy.wh");
    // Null fields should be present as null (not missing)
    assert!(v["context"].get("line").is_some());
    assert!(v["context"].get("field").is_some());
}

// --- Human-readable format ---

#[test]
fn test_human_format_includes_error_code() {
    let err = WhError::ConnectionError {
        message: "Wheelhouse not running".to_string(),
        context: ErrorContext::default(),
    };

    let display = format!("{}", err);
    assert!(
        display.contains("WH-1001"),
        "human-readable output must contain error code WH-1001, got: {}",
        display
    );
    assert!(
        display.contains("Wheelhouse not running"),
        "human-readable output must contain the message"
    );
}

// --- RT-B1: No "broker" in user-facing messages ---

#[test]
fn test_error_messages_never_contain_broker() {
    let errors: Vec<WhError> = vec![
        WhError::ConnectionError {
            message: "Wheelhouse not running".to_string(),
            context: ErrorContext::default(),
        },
        WhError::DeployError {
            kind: DeployErrorKind::LintError,
            message: "invalid field".to_string(),
            context: ErrorContext::default(),
        },
        WhError::StreamError {
            message: "stream not found".to_string(),
            context: ErrorContext::default(),
        },
        WhError::ConfigError {
            message: "invalid config".to_string(),
            context: ErrorContext::default(),
        },
        WhError::InternalError {
            message: "unexpected state".to_string(),
            context: ErrorContext::default(),
        },
    ];

    for err in &errors {
        let display = format!("{}", err);
        assert!(
            !display.to_lowercase().contains("broker"),
            "Error message must not contain 'broker': {}",
            display
        );
    }
}

// --- render_error function ---

#[test]
fn test_render_error_human_format() {
    let err = WhError::ConnectionError {
        message: "Wheelhouse not running".to_string(),
        context: ErrorContext::default(),
    };

    let output = render_error(&err, OutputFormat::Human);
    assert!(output.contains("WH-1001"));
    assert!(output.contains("Wheelhouse not running"));
}

#[test]
fn test_render_error_json_format() {
    let err = WhError::StreamError {
        message: "stream 'main' not found".to_string(),
        context: ErrorContext::default(),
    };

    let output = render_error(&err, OutputFormat::Json);
    let v: serde_json::Value =
        serde_json::from_str(&output).expect("render_error Json must produce valid JSON");
    assert_eq!(v["v"], 1);
    assert_eq!(v["status"], "error");
    assert_eq!(v["code"], 3001);
}

// --- Uniqueness ---

#[test]
fn test_all_variants_have_unique_error_codes() {
    let errors: Vec<WhError> = vec![
        WhError::ConnectionError {
            message: "".to_string(),
            context: ErrorContext::default(),
        },
        WhError::DeployError {
            kind: DeployErrorKind::LintError,
            message: "".to_string(),
            context: ErrorContext::default(),
        },
        WhError::DeployError {
            kind: DeployErrorKind::PlanError,
            message: "".to_string(),
            context: ErrorContext::default(),
        },
        WhError::DeployError {
            kind: DeployErrorKind::ApplyError,
            message: "".to_string(),
            context: ErrorContext::default(),
        },
        WhError::StreamError {
            message: "".to_string(),
            context: ErrorContext::default(),
        },
        WhError::ConfigError {
            message: "".to_string(),
            context: ErrorContext::default(),
        },
        WhError::InternalError {
            message: "".to_string(),
            context: ErrorContext::default(),
        },
    ];

    let codes: Vec<u32> = errors.iter().map(|e| e.error_code()).collect();
    let mut unique_codes = codes.clone();
    unique_codes.sort();
    unique_codes.dedup();
    assert_eq!(
        codes.len(),
        unique_codes.len(),
        "all error codes must be unique"
    );
}
