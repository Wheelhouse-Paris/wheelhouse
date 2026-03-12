//! Acceptance tests for `wh ps` — Story 3.1
//!
//! These tests are written RED-first (TDD). They verify the acceptance criteria
//! for the unified component inspection command. All tests are expected to FAIL
//! until the implementation is complete.

use std::process::Command;

/// Helper: run `wh ps` with optional args and return (stdout, stderr, exit_code)
fn run_wh_ps(args: &[&str]) -> (String, String, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wh"));
    cmd.arg("ps");
    for arg in args {
        cmd.arg(arg);
    }
    // Ensure no TTY for deterministic output in tests
    cmd.env("NO_COLOR", "1");
    let output = cmd.output().expect("failed to execute wh binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

// ── AC #1: Columnar table with component info ──

#[test]
#[ignore = "requires running Wheelhouse broker"]
fn ac1_ps_shows_columnar_table_with_headers() {
    let (stdout, _stderr, _code) = run_wh_ps(&[]);
    // Table must have NAME, STATUS, STREAM, PROVIDER, UPTIME columns
    assert!(
        stdout.contains("NAME"),
        "Table output must contain NAME column header"
    );
    assert!(
        stdout.contains("STATUS"),
        "Table output must contain STATUS column header"
    );
    assert!(
        stdout.contains("STREAM"),
        "Table output must contain STREAM column header"
    );
    assert!(
        stdout.contains("PROVIDER"),
        "Table output must contain PROVIDER column header"
    );
    assert!(
        stdout.contains("UPTIME"),
        "Table output must contain UPTIME column header"
    );
}

#[test]
#[ignore = "requires running Wheelhouse broker"]
fn ac1_ps_shows_summary_line() {
    let (stdout, _stderr, _code) = run_wh_ps(&[]);
    // Summary line must match pattern: "N agents · N running · N stopped"
    // Using a loose check for the format
    assert!(
        stdout.contains("agents") && stdout.contains("running") && stdout.contains("stopped"),
        "Summary line must show 'N agents · N running · N stopped', got: {}",
        stdout
    );
}

// ── AC #2: JSON output with machine-readable fields ──

#[test]
#[ignore = "requires running Wheelhouse broker"]
fn ac2_ps_json_format_is_valid_json() {
    let (stdout, _stderr, code) = run_wh_ps(&["--format", "json"]);
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(
        parsed.is_ok(),
        "wh ps --format json must output valid JSON, got: {}",
        stdout
    );
    assert_eq!(code, 0, "Exit code must be 0 on success");
}

#[test]
fn ac2_ps_json_includes_schema_version() {
    let (stdout, _stderr, _code) = run_wh_ps(&["--format", "json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");
    assert_eq!(
        parsed.get("v"),
        Some(&serde_json::Value::Number(1.into())),
        "JSON output must include '\"v\": 1' schema version field"
    );
}

#[test]
#[ignore = "requires running Wheelhouse broker"]
fn ac2_ps_json_components_have_status_enum() {
    let (stdout, _stderr, _code) = run_wh_ps(&["--format", "json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");
    let data = parsed.get("data").expect("JSON must have 'data' field");
    let components = data
        .get("components")
        .and_then(|c| c.as_array())
        .expect("data.components must be an array");

    let valid_statuses = ["running", "stopped", "degraded", "unknown"];
    for component in components {
        let status = component
            .get("status")
            .and_then(|s| s.as_str())
            .expect("each component must have a 'status' string field");
        assert!(
            valid_statuses.contains(&status),
            "Status must be one of {:?}, got: {}",
            valid_statuses,
            status
        );
    }
}

#[test]
fn ac2_ps_json_fields_are_snake_case() {
    let (stdout, _stderr, _code) = run_wh_ps(&["--format", "json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    // Recursively check all keys are snake_case
    fn check_snake_case(value: &serde_json::Value, path: &str) {
        if let Some(obj) = value.as_object() {
            for (key, val) in obj {
                assert!(
                    !key.chars().any(|c| c.is_uppercase()),
                    "JSON key '{}' at path '{}' contains uppercase chars — must be snake_case (SCV-01)",
                    key,
                    path
                );
                check_snake_case(val, &format!("{}.{}", path, key));
            }
        }
        if let Some(arr) = value.as_array() {
            for (i, val) in arr.iter().enumerate() {
                check_snake_case(val, &format!("{}[{}]", path, i));
            }
        }
    }
    check_snake_case(&parsed, "root");
}

// ── AC #3: TTY detection and NO_COLOR support ──

#[test]
fn ac3_no_color_strips_ansi_codes() {
    let (stdout, _stderr, _code) = run_wh_ps(&[]);
    // With NO_COLOR=1 set in run_wh_ps, output must not contain ANSI escape codes
    assert!(
        !stdout.contains("\x1b["),
        "Output must not contain ANSI escape codes when NO_COLOR is set"
    );
}

#[test]
fn ac3_no_tty_uses_ascii_borders() {
    let (stdout, _stderr, _code) = run_wh_ps(&[]);
    // When piped (no TTY), should use ASCII borders not Unicode box chars
    // Unicode box chars: ┌ ┐ └ ┘ │ ─ ├ ┤ ┬ ┴ ┼
    let unicode_box_chars = ['┌', '┐', '└', '┘', '│', '─', '├', '┤', '┬', '┴', '┼'];
    for ch in &unicode_box_chars {
        assert!(
            !stdout.contains(*ch),
            "Non-TTY output must use ASCII borders, but found Unicode char '{}'",
            ch
        );
    }
}

// ── AC #4: Broker not running — error handling ──

#[test]
fn ac4_no_broker_shows_friendly_error() {
    // When broker is not running, should show "Wheelhouse not running"
    let (stdout, stderr, code) = run_wh_ps(&[]);
    let combined = format!("{}{}", stdout, stderr);
    assert_eq!(
        code, 1,
        "Exit code must be 1 when Wheelhouse is not running"
    );
    assert!(
        combined.contains("Wheelhouse not running") || combined.contains("not running"),
        "Error message must say 'Wheelhouse not running', got: {}",
        combined
    );
    // Must NOT mention "broker" or "connection refused"
    let lower = combined.to_lowercase();
    assert!(
        !lower.contains("broker"),
        "Error must never mention 'broker' (RT-B1), got: {}",
        combined
    );
    assert!(
        !lower.contains("connection refused"),
        "Error must never mention 'connection refused' (RT-B1), got: {}",
        combined
    );
}

// ── AC #5: JSON error output when broker not running ──

#[test]
fn ac5_no_broker_json_error_format() {
    let (stdout, _stderr, code) = run_wh_ps(&["--format", "json"]);
    assert_eq!(
        code, 1,
        "Exit code must be 1 when Wheelhouse is not running"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Error output must be valid JSON");
    assert_eq!(
        parsed.get("v"),
        Some(&serde_json::Value::Number(1.into())),
        "Error JSON must include '\"v\": 1'"
    );
    assert_eq!(
        parsed.get("status").and_then(|s| s.as_str()),
        Some("error"),
        "Error JSON status must be 'error'"
    );
    assert_eq!(
        parsed.get("code").and_then(|s| s.as_str()),
        Some("CONNECTION_ERROR"),
        "Error JSON code must be 'CONNECTION_ERROR'"
    );
    assert!(
        parsed.get("message").and_then(|s| s.as_str()).is_some(),
        "Error JSON must include 'message' field"
    );
}
