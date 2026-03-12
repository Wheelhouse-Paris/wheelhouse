//! Acceptance tests for Story 3.5: Structured Error System and ERRORS.md
//!
//! These tests verify the WhError typed hierarchy, error codes, and ERRORS.md coverage.

use wh_cli::output::error::WhError;

// --- AC#1: Every error has a machine-readable error code ---

#[test]
fn test_connection_error_has_code() {
    let err = WhError::ConnectionError;
    assert_eq!(err.error_code(), "CONNECTION_ERROR");
}

#[test]
fn test_stream_error_has_code() {
    let err = WhError::StreamError("stream not found".to_string());
    assert_eq!(err.error_code(), "STREAM_ERROR");
}

#[test]
fn test_internal_error_has_code() {
    let err = WhError::InternalError("unexpected state".to_string());
    assert_eq!(err.error_code(), "INTERNAL_ERROR");
}

#[test]
fn test_secret_not_found_has_code() {
    let err = WhError::SecretNotFound("ANTHROPIC_API_KEY".to_string());
    assert_eq!(err.error_code(), "SECRET_NOT_FOUND");
}

// --- AC#2: Error codes documented in ERRORS.md ---

#[test]
fn test_all_error_codes_documented_in_errors_md() {
    let errors_md =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ERRORS.md"))
            .expect("ERRORS.md must exist at repo root");

    let codes = ["WH-1001", "WH-2001", "WH-2002", "WH-2003", "WH-3001", "WH-4001", "WH-9001"];
    for code in &codes {
        assert!(
            errors_md.contains(code),
            "ERRORS.md must document error code {code}"
        );
    }
}

// --- AC#3: Error messages never contain "broker" (RT-B1) ---

#[test]
fn test_error_messages_never_contain_broker() {
    let errors: Vec<WhError> = vec![
        WhError::ConnectionError,
        WhError::StreamError("stream not found".to_string()),
        WhError::InternalError("unexpected state".to_string()),
        WhError::StreamNotFound("main".to_string()),
    ];

    for err in &errors {
        let display = format!("{err}");
        assert!(
            !display.to_lowercase().contains("broker"),
            "Error message must not contain 'broker': {display}"
        );
    }
}

// --- AC#4: All error codes are unique ---

#[test]
fn test_all_variants_have_unique_error_codes() {
    let errors: Vec<WhError> = vec![
        WhError::ConnectionError,
        WhError::StreamError("".to_string()),
        WhError::InternalError("".to_string()),
        WhError::StreamNotFound("".to_string()),
        WhError::AgentNotFound("".to_string()),
        WhError::SecretNotFound("".to_string()),
    ];

    let mut codes: Vec<&'static str> = errors.iter().map(|e| e.error_code()).collect();
    let original_len = codes.len();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), original_len, "all error codes must be unique");
}

// --- Display format ---

#[test]
fn test_connection_error_display() {
    let err = WhError::ConnectionError;
    let display = format!("{err}");
    assert!(!display.is_empty(), "error display must not be empty");
}

#[test]
fn test_secret_not_found_display_includes_name() {
    let err = WhError::SecretNotFound("ANTHROPIC_API_KEY".to_string());
    let display = format!("{err}");
    assert!(
        display.contains("ANTHROPIC_API_KEY"),
        "display must include secret name: {display}"
    );
}
