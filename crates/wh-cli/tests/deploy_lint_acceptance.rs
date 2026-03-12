//! Acceptance Tests for Story 2-2: `.wh` File Format and Lint Validation
//!
//! Each test maps to a specific acceptance criterion from the story.
//!
//! Run: `cargo test -p wh-cli --test deploy_lint_acceptance`

use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

fn wh_binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wh"))
}

fn write_wh_file(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::with_suffix(".wh").expect("create temp file");
    f.write_all(content.as_bytes()).expect("write temp file");
    f.flush().expect("flush temp file");
    f
}

// ─── AC #1: Valid .wh file exits 0, no errors ───

#[test]
fn ac1_valid_wh_file_exits_zero() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
    streams: [main]

streams:
  - name: main
    provider: local
    compaction_cron: "0 2 * * *"
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert!(output.status.success(), "expected exit code 0");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.is_empty() || !stderr.contains("error"),
        "expected no errors on stderr, got: {stderr}"
    );
}

#[test]
fn ac1_valid_wh_file_minimal_agents_and_streams() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: donna
    image: wheelhouse/donna:latest
    max_replicas: 1

streams:
  - name: main
    compaction_cron: "0 0 * * *"
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert!(output.status.success(), "expected exit code 0");
}

// ─── AC #2: Invalid field → compiler-style error, exit 1 ───

#[test]
fn ac2_missing_max_replicas_produces_compiler_style_error() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    streams: [main]

streams:
  - name: main
    provider: local
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("max_replicas"),
        "error should mention max_replicas, got: {stderr}"
    );
    // Compiler-style format: file:line: message
    let filename = file.path().file_name().unwrap().to_str().unwrap();
    assert!(
        stderr.contains(filename),
        "error should reference the file name, got: {stderr}"
    );
}

#[test]
fn ac2_missing_agent_name_produces_error() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
}

#[test]
fn ac2_missing_api_version_produces_error() {
    let file = write_wh_file(
        r#"
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("apiVersion"),
        "error should mention apiVersion, got: {stderr}"
    );
}

#[test]
fn ac2_wrong_api_version_produces_error() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v2

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
}

// ─── AC #3: Stream without compaction cron → warning (FM-06) ───

#[test]
fn ac3_stream_without_compaction_cron_produces_warning() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
    streams: [main]

streams:
  - name: main
    provider: local
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    // Warnings do not cause exit 1 — only errors do
    assert!(output.status.success(), "expected exit code 0 (warnings only)");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("compaction") && stderr.contains("main"),
        "warning should mention compaction cron and stream name, got: {stderr}"
    );
    assert!(
        stderr.contains("WAL will grow unbounded"),
        "warning should include hint about WAL growth, got: {stderr}"
    );
}

// ─── AC #4: provider: local is valid, exits 0 ───

#[test]
fn ac4_provider_local_is_valid() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
    provider: local
    compaction_cron: "0 2 * * *"
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert!(output.status.success(), "provider: local should be valid, exit 0");
}

#[test]
fn ac4_provider_defaults_to_local_when_absent() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
    compaction_cron: "0 2 * * *"
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert!(
        output.status.success(),
        "absent provider should default to local, exit 0"
    );
}

// ─── AC #5: Unsupported provider → error ───

#[test]
fn ac5_unsupported_provider_aws_produces_error() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
    provider: aws
    compaction_cron: "0 2 * * *"
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("aws") || stderr.contains("unsupported"),
        "error should mention unsupported provider, got: {stderr}"
    );
}

#[test]
fn ac5_unsupported_provider_weaviate_produces_error() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
    provider: weaviate
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
}

// ─── Additional validation tests ───

#[test]
fn duplicate_agent_names_produce_error() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
  - name: researcher
    image: my-org/researcher:v2
    max_replicas: 1

streams:
  - name: main
    compaction_cron: "0 2 * * *"
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate") || stderr.contains("researcher"),
        "error should mention duplicate, got: {stderr}"
    );
}

#[test]
fn duplicate_stream_names_produce_error() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
    compaction_cron: "0 2 * * *"
  - name: main
    provider: local
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
}

#[test]
fn agent_referencing_undeclared_stream_produces_error() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
    streams: [nonexistent]

streams:
  - name: main
    compaction_cron: "0 2 * * *"
"#,
    );

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nonexistent"),
        "error should mention undeclared stream, got: {stderr}"
    );
}

// ─── JSON output tests ───

#[test]
fn json_output_valid_file_has_v1_envelope() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3

streams:
  - name: main
    provider: local
    compaction_cron: "0 2 * * *"
"#,
    );

    let output = wh_binary()
        .args([
            "deploy",
            "lint",
            file.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("run wh");

    assert!(output.status.success(), "expected exit code 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\noutput: {stdout}"));

    assert_eq!(json["v"], 1, "JSON envelope must have v: 1");
    assert_eq!(json["status"], "ok", "status should be ok for valid file");
}

#[test]
fn json_output_invalid_file_has_error_status() {
    let file = write_wh_file(
        r#"
apiVersion: wheelhouse.dev/v1

agents:
  - name: researcher
    image: my-org/researcher:latest

streams:
  - name: main
"#,
    );

    let output = wh_binary()
        .args([
            "deploy",
            "lint",
            file.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\noutput: {stdout}"));

    assert_eq!(json["v"], 1, "JSON envelope must have v: 1");
    assert_eq!(json["status"], "error", "status should be error");
    assert!(
        json["data"]["errors"].is_array(),
        "errors should be an array"
    );
}

#[test]
fn nonexistent_file_produces_error() {
    let output = wh_binary()
        .args(["deploy", "lint", "/tmp/nonexistent-12345.wh"])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
}

#[test]
fn invalid_yaml_produces_error() {
    let file = write_wh_file(":::invalid yaml{{{");

    let output = wh_binary()
        .args(["deploy", "lint", file.path().to_str().unwrap()])
        .output()
        .expect("run wh");

    assert_eq!(output.status.code(), Some(1), "expected exit code 1");
}
