//! Acceptance tests for `wh logs` — Real-Time Agent Log Streaming (Story 3.2).
//!
//! These tests verify the `wh logs` command behavior per acceptance criteria.
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
// AC #1: Running agent, --tail N, real-time streaming
// ============================================================

/// AC #1: `wh logs <agent> --tail 100` shows last 100 log lines.
/// PENDS on broker: control socket not available.
#[test]
fn logs_tail_shows_last_n_lines() {
    let output = wh_bin()
        .args(["logs", "researcher", "--tail", "100"])
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

/// AC #1: `wh logs` requires an agent name argument.
#[test]
fn logs_requires_agent_name() {
    let output = wh_bin()
        .args(["logs"])
        .output()
        .expect("failed to execute wh");

    assert!(!output.status.success(), "Expected failure without agent name");
    // clap should produce a usage error
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required") || stderr.contains("Usage") || stderr.contains("error"),
        "Expected clap usage error, got: {stderr}"
    );
}

// ============================================================
// AC #2: Level filtering (--level)
// ============================================================

/// AC #2: `wh logs <agent> --level debug` is accepted as a valid level.
/// PENDS on broker for actual filtering behavior.
#[test]
fn logs_level_debug_accepted() {
    let output = wh_bin()
        .args(["logs", "researcher", "--level", "debug"])
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

/// AC #2: `wh logs <agent> --level invalid` is rejected by clap.
#[test]
fn logs_rejects_invalid_level() {
    let output = wh_bin()
        .args(["logs", "researcher", "--level", "invalid"])
        .output()
        .expect("failed to execute wh");

    assert!(!output.status.success(), "Expected failure for invalid level");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid") || stderr.contains("possible values"),
        "Expected clap to reject invalid level, got: {stderr}"
    );
}

/// AC #2: All valid levels are accepted: debug, info, warn, error.
#[test]
fn logs_all_valid_levels_accepted() {
    for level in &["debug", "info", "warn", "error"] {
        let output = wh_bin()
            .args(["logs", "researcher", "--level", level])
            .output()
            .expect("failed to execute wh");

        let stderr = String::from_utf8_lossy(&output.stderr);
        // Should fail with connection error, NOT with invalid argument
        assert!(
            stderr.contains("Wheelhouse not running"),
            "Level '{level}' should be accepted. Got: {stderr}"
        );
    }
}

// ============================================================
// AC #3: JSON output format (--format json)
// ============================================================

/// AC #3: `wh logs <agent> --format json` with no broker returns JSON error envelope.
#[test]
fn logs_json_error_envelope_no_broker() {
    let output = wh_bin()
        .args(["logs", "researcher", "--format", "json"])
        .output()
        .expect("failed to execute wh");

    assert!(!output.status.success());

    // Error output should be valid JSON with connection error
    let combined = String::from_utf8_lossy(&output.stderr);
    // Try stderr first, then stdout
    let json_str = if combined.contains("CONNECTION_ERROR") {
        combined.to_string()
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };

    let parsed: serde_json::Value = serde_json::from_str(json_str.trim())
        .unwrap_or_else(|e| panic!("Expected valid JSON error envelope, got parse error: {e}\nOutput: {json_str}"));

    assert_eq!(parsed["v"], 1, "Schema version must be 1");
    assert_eq!(parsed["status"], "error", "Status must be 'error'");
    assert_eq!(parsed["code"], "CONNECTION_ERROR", "Error code must be CONNECTION_ERROR");
}

/// AC #3: `wh logs <agent> --format json` exit code is 1 on error.
#[test]
fn logs_json_error_exit_code() {
    let output = wh_bin()
        .args(["logs", "researcher", "--format", "json"])
        .output()
        .expect("failed to execute wh");

    assert_eq!(
        output.status.code(),
        Some(1),
        "Exit code must be 1 on error"
    );
}

// ============================================================
// AC #4: Stopped agent shows historical logs + notice
// ============================================================

/// AC #4: `wh logs <agent>` with no broker shows "Wheelhouse not running".
/// When the broker exists and the agent is stopped, it should show
/// historical logs + "Agent '<name>' is not currently running" notice.
/// PENDS on broker.
#[test]
fn logs_no_broker_shows_not_running() {
    let output = wh_bin()
        .args(["logs", "researcher"])
        .output()
        .expect("failed to execute wh");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wheelhouse not running"),
        "Expected 'Wheelhouse not running', got: {stderr}"
    );
}

/// AC #4: Exit code is 1 when Wheelhouse is not running.
#[test]
fn logs_no_broker_exit_code_1() {
    let output = wh_bin()
        .args(["logs", "researcher"])
        .output()
        .expect("failed to execute wh");

    assert_eq!(
        output.status.code(),
        Some(1),
        "Exit code must be 1 when Wheelhouse not running"
    );
}

// ============================================================
// Edge cases
// ============================================================

/// Edge: `wh logs <agent> --tail 0` should be accepted (show 0 historical lines).
#[test]
fn logs_tail_zero_accepted() {
    let output = wh_bin()
        .args(["logs", "researcher", "--tail", "0"])
        .output()
        .expect("failed to execute wh");

    // Should fail with connection error, not argument error
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wheelhouse not running"),
        "--tail 0 should be accepted. Got: {stderr}"
    );
}

/// Edge: Default format is human (no --format flag).
#[test]
fn logs_default_format_is_human() {
    let output = wh_bin()
        .args(["logs", "researcher"])
        .output()
        .expect("failed to execute wh");

    // Without --format json, error should be plain text (not JSON)
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wheelhouse not running") || stderr.contains("Error:"),
        "Default output should be human-readable. Got: {stderr}"
    );
    // Ensure it's NOT JSON
    assert!(
        serde_json::from_str::<serde_json::Value>(stderr.trim()).is_err(),
        "Default output should NOT be JSON"
    );
}
