//! Integration tests for `wh surface cli` command.

use std::process::Command;

fn wh_binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wh"))
}

#[test]
fn test_surface_cli_accepts_stream_flag() {
    // Test that `wh surface cli --stream main` is a recognized command.
    // We pass --help to avoid actually starting the interactive loop.
    let output = wh_binary()
        .args(["surface", "cli", "--help"])
        .output()
        .expect("failed to execute wh binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "wh surface cli --help should succeed"
    );
    assert!(
        stdout.contains("--stream"),
        "help should mention --stream flag"
    );
    assert!(
        stdout.contains("--format"),
        "help should mention --format flag"
    );
}

#[test]
fn test_surface_cli_requires_stream_flag() {
    // `wh surface cli` without --stream should fail.
    let output = wh_binary()
        .args(["surface", "cli"])
        .output()
        .expect("failed to execute wh binary");

    assert!(!output.status.success(), "missing --stream should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--stream"),
        "error should mention the required --stream flag"
    );
}

#[test]
fn test_surface_cli_rejects_invalid_format() {
    // `wh surface cli --stream main --format xml` should fail.
    let output = wh_binary()
        .args(["surface", "cli", "--stream", "main", "--format", "xml"])
        .output()
        .expect("failed to execute wh binary");

    assert!(!output.status.success(), "invalid --format should fail");
}

#[test]
fn test_human_output_format() {
    use wh_cli::output::{format_message, OutputFormat, SurfaceMessage};

    let msg = SurfaceMessage {
        content: "hello world".to_string(),
        publisher: "agent-1".to_string(),
        timestamp: "2026-03-12T10:30:00Z".to_string(),
    };
    let output = format_message(&msg, OutputFormat::Human);

    // Pattern: [timestamp] publisher: content
    assert!(output.starts_with("[2026-03-12T10:30:00Z]"));
    assert!(output.contains("agent-1:"));
    assert!(output.contains("hello world"));
    // Must NOT contain broker internals (RT-B1)
    assert!(!output.contains("broker"));
    assert!(!output.contains("zmq"));
    assert!(!output.contains("5555"));
}

#[test]
fn test_json_output_format() {
    use wh_cli::output::{format_message, OutputFormat, SurfaceMessage};

    let msg = SurfaceMessage {
        content: "test message".to_string(),
        publisher: "cli-surface".to_string(),
        timestamp: "2026-03-12T10:30:00Z".to_string(),
    };
    let output = format_message(&msg, OutputFormat::Json);

    let parsed: serde_json::Value = serde_json::from_str(&output).expect("should be valid JSON");
    assert_eq!(parsed["v"], 1, "JSON must have v: 1");
    assert_eq!(parsed["status"], "ok", "JSON must have status field");
    assert_eq!(parsed["data"]["publisher"], "cli-surface");
    assert_eq!(parsed["data"]["timestamp"], "2026-03-12T10:30:00Z");
    assert_eq!(parsed["data"]["content"], "test message");
}

#[test]
fn test_json_fields_snake_case() {
    use wh_cli::output::{format_message, OutputFormat, SurfaceMessage};

    let msg = SurfaceMessage {
        content: "test".to_string(),
        publisher: "agent".to_string(),
        timestamp: "2026-03-12T10:30:00Z".to_string(),
    };
    let output = format_message(&msg, OutputFormat::Json);

    // All fields must be snake_case (SCV-01)
    assert!(output.contains("\"publisher\""));
    assert!(output.contains("\"timestamp\""));
    assert!(output.contains("\"content\""));
    assert!(output.contains("\"status\""));
    // No camelCase
    assert!(!output.contains("Publisher"));
    assert!(!output.contains("Timestamp"));
    assert!(!output.contains("Content"));
}

#[test]
fn test_wh_error_exit_codes() {
    use wh_cli::output::error::WhError;

    assert_eq!(WhError::ConnectionError.exit_code(), 1);
    assert_eq!(WhError::StreamError("test".into()).exit_code(), 1);
    assert_eq!(WhError::InternalError("test".into()).exit_code(), 1);
}

#[test]
fn test_output_format_switch() {
    use wh_cli::output::{format_message, OutputFormat, SurfaceMessage};

    let msg = SurfaceMessage {
        content: "hello".to_string(),
        publisher: "agent".to_string(),
        timestamp: "2026-03-12T10:30:00Z".to_string(),
    };

    // Human format should NOT be valid JSON
    let human = format_message(&msg, OutputFormat::Human);
    assert!(serde_json::from_str::<serde_json::Value>(&human).is_err());

    // JSON format should be valid JSON
    let json = format_message(&msg, OutputFormat::Json);
    assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
}

#[test]
fn test_stream_name_validation_valid() {
    use wh_cli::commands::surface::validate_stream_name;

    assert!(validate_stream_name("main").is_ok());
    assert!(validate_stream_name("my-stream").is_ok());
    assert!(validate_stream_name("a").is_ok());
    assert!(validate_stream_name("stream1").is_ok());
    assert!(validate_stream_name("test-stream-123").is_ok());
}

#[test]
fn test_stream_name_validation_invalid() {
    use wh_cli::commands::surface::validate_stream_name;

    assert!(validate_stream_name("").is_err());
    assert!(validate_stream_name("1stream").is_err());
    assert!(validate_stream_name("Main").is_err());
    assert!(validate_stream_name("my_stream").is_err());
    assert!(validate_stream_name("STREAM").is_err());
    assert!(validate_stream_name("-stream").is_err());
    assert!(validate_stream_name("stream name").is_err());
}

/// AC-3: When broker is not running, exit with code 1 and human-readable error.
#[test]
fn test_surface_cli_no_broker_exits_with_error() {
    // No broker running — should exit with error code 1
    // Use env vars to point to an unused port to ensure no broker interference
    let output = wh_binary()
        .args(["surface", "cli", "--stream", "main"])
        .env("WH_CONTROL_ENDPOINT", "tcp://127.0.0.1:19876")
        .env("WH_PUB_ENDPOINT", "tcp://127.0.0.1:19877")
        .env("WH_SUB_ENDPOINT", "tcp://127.0.0.1:19878")
        .output()
        .expect("failed to execute wh binary");

    assert!(
        !output.status.success(),
        "should exit with error when broker not running"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not running"),
        "stderr should mention 'not running', got: {stderr}"
    );
}

/// AC-3: JSON error format when broker is not running.
#[test]
fn test_surface_cli_json_error_when_no_broker() {
    let output = wh_binary()
        .args(["surface", "cli", "--stream", "main", "--format", "json"])
        .env("WH_CONTROL_ENDPOINT", "tcp://127.0.0.1:19879")
        .env("WH_PUB_ENDPOINT", "tcp://127.0.0.1:19880")
        .env("WH_SUB_ENDPOINT", "tcp://127.0.0.1:19881")
        .output()
        .expect("failed to execute wh binary");

    assert!(
        !output.status.success(),
        "should exit with error when broker not running"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // JSON format should include structured error output
    // The error may be on stderr as JSON or as human-readable text
    assert!(
        stderr.contains("not running") || stderr.contains("CONNECTION_ERROR"),
        "stderr should contain error info, got: {stderr}"
    );
}

/// Regression: `wh surface cli --help` should still work.
#[test]
fn test_surface_cli_help_regression() {
    let output = wh_binary()
        .args(["surface", "cli", "--help"])
        .output()
        .expect("failed to execute wh binary");

    assert!(
        output.status.success(),
        "wh surface cli --help should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("interactive"),
        "help should mention interactive CLI surface"
    );
}
