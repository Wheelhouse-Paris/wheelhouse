//! Acceptance tests for `wh stream tail` — Real-Time Stream Observation (Story 3.3).
//!
//! These tests verify the `wh stream tail` command behavior per acceptance criteria.
//! Tests that require a running broker are expected to FAIL (RED phase)
//! until the broker is implemented (Epic 1).

use std::process::Command;

/// Helper to get the path to the `wh` binary.
fn wh_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wh"));
    // Ensure we run in a temp dir with no .wh/ directory
    cmd.current_dir(std::env::temp_dir());
    // Suppress color for deterministic output
    cmd.env("NO_COLOR", "1");
    cmd
}

// ============================================================
// AC #1: Stream tail shows objects in real-time format
// ============================================================

/// AC #1: `wh stream tail main` connects to broker and streams objects.
/// PENDS on broker: control socket not available.
#[test]
fn stream_tail_connects_to_broker() {
    let output = wh_bin()
        .args(["stream", "tail", "main"])
        .output()
        .expect("failed to execute wh");

    // Without a running broker, this should fail with connection error
    assert!(!output.status.success(), "Expected failure without broker");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wheelhouse not running"),
        "Expected 'Wheelhouse not running' error, got: {stderr}"
    );
}

/// AC #1: `wh stream tail` requires a stream name argument.
#[test]
fn stream_tail_requires_stream_name() {
    let output = wh_bin()
        .args(["stream", "tail"])
        .output()
        .expect("failed to execute wh");

    assert!(
        !output.status.success(),
        "Expected failure without stream name"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required") || stderr.contains("Usage") || stderr.contains("error"),
        "Expected clap usage error, got: {stderr}"
    );
}

/// AC #1: `wh stream tail` with exit code 1 when broker not running.
#[test]
fn stream_tail_exit_code_1_on_connection_error() {
    let output = wh_bin()
        .args(["stream", "tail", "main"])
        .output()
        .expect("failed to execute wh");

    assert_eq!(
        output.status.code(),
        Some(1),
        "Expected exit code 1 on connection error"
    );
}

// ============================================================
// AC #2: Type filtering (--filter type=TypeName)
// ============================================================

/// AC #2: `wh stream tail main --filter type=TextMessage` is accepted.
/// PENDS on broker for actual filtering behavior.
#[test]
fn stream_tail_filter_type_accepted() {
    let output = wh_bin()
        .args(["stream", "tail", "main", "--filter", "type=TextMessage"])
        .output()
        .expect("failed to execute wh");

    // Should fail with connection error, NOT with invalid argument
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wheelhouse not running"),
        "Expected connection error, not argument error. Got: {stderr}"
    );
}

// ============================================================
// AC #3: JSON output format
// ============================================================

/// AC #3: `wh stream tail main --format json` returns JSON error envelope.
#[test]
fn stream_tail_json_format_error_envelope() {
    let output = wh_bin()
        .args(["stream", "tail", "main", "--format", "json"])
        .output()
        .expect("failed to execute wh");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should produce a JSON error envelope
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stderr);
    assert!(
        parsed.is_ok(),
        "Expected valid JSON error envelope, got: {stderr}"
    );

    let json = parsed.unwrap();
    assert_eq!(json["v"], 1, "Expected schema version 1");
    assert_eq!(json["status"], "error", "Expected error status");
    assert_eq!(
        json["code"], "CONNECTION_ERROR",
        "Expected CONNECTION_ERROR code"
    );
}

// ============================================================
// AC #4: Publisher filtering (--filter publisher=name)
// ============================================================

/// AC #4: `wh stream tail main --filter publisher=researcher-2` is accepted.
/// PENDS on broker for actual filtering behavior.
#[test]
fn stream_tail_filter_publisher_accepted() {
    let output = wh_bin()
        .args([
            "stream",
            "tail",
            "main",
            "--filter",
            "publisher=researcher-2",
        ])
        .output()
        .expect("failed to execute wh");

    // Should fail with connection error, NOT with invalid argument
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wheelhouse not running"),
        "Expected connection error, not argument error. Got: {stderr}"
    );
}

/// AC #2 + #4: Multiple filters accepted simultaneously.
#[test]
fn stream_tail_multiple_filters_accepted() {
    let output = wh_bin()
        .args([
            "stream",
            "tail",
            "main",
            "--filter",
            "type=TextMessage",
            "--filter",
            "publisher=donna",
        ])
        .output()
        .expect("failed to execute wh");

    // Should fail with connection error, NOT with invalid argument
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wheelhouse not running"),
        "Expected connection error, not argument error. Got: {stderr}"
    );
}

/// Invalid filter syntax is rejected.
#[test]
fn stream_tail_invalid_filter_rejected() {
    let output = wh_bin()
        .args(["stream", "tail", "main", "--filter", "invalid"])
        .output()
        .expect("failed to execute wh");

    assert!(
        !output.status.success(),
        "Expected failure with invalid filter"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid filter") || stderr.contains("error") || stderr.contains("invalid"),
        "Expected filter validation error, got: {stderr}"
    );
}

/// Unknown filter key is rejected.
#[test]
fn stream_tail_unknown_filter_key_rejected() {
    let output = wh_bin()
        .args(["stream", "tail", "main", "--filter", "unknown=value"])
        .output()
        .expect("failed to execute wh");

    assert!(
        !output.status.success(),
        "Expected failure with unknown filter key"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown filter") || stderr.contains("error") || stderr.contains("unknown"),
        "Expected unknown filter error, got: {stderr}"
    );
}

// ============================================================
// AC #5: Graceful Ctrl-C shutdown (exit code 0)
// ============================================================

// Note: Ctrl-C signal testing is not feasible in acceptance tests
// without a running broker. The graceful shutdown behavior is a
// [PHASE-2-ONLY] concern per architecture. When the broker exists
// and follow-mode streaming is active, Ctrl-C must exit cleanly
// with code 0.

// ============================================================
// Default format behavior
// ============================================================

/// Default format is human (no --format flag needed).
#[test]
fn stream_tail_default_format_is_human() {
    let output = wh_bin()
        .args(["stream", "tail", "main"])
        .output()
        .expect("failed to execute wh");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Human format: plain text error, not JSON
    assert!(
        stderr.contains("Wheelhouse not running"),
        "Expected human-format error, got: {stderr}"
    );
    // Should NOT be JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(stderr.trim());
    assert!(
        parsed.is_err(),
        "Human format should not produce valid JSON"
    );
}
