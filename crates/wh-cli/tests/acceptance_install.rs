//! Acceptance tests for Story 1.6: Single-Command Installation and Release Pipeline.
//!
//! Tests verify:
//! - AC#1: `wh --version` prints version, git hash, and target triple (TT-06)
//! - AC#5: bare `wh` prints concise getting-started hint (<= 5 lines)
//! - AC#3: release.yml and install.sh are valid and complete

use std::process::Command;

fn wh_binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wh"))
}

// --- AC#1: wh --version ---

#[test]
fn test_version_output_format() {
    let output = wh_binary()
        .arg("--version")
        .output()
        .expect("failed to run wh");

    assert!(output.status.success(), "wh --version should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Format: "wh X.Y.Z (hash, target-triple)"
    assert!(
        stdout.starts_with("wh "),
        "version output should start with 'wh ': {stdout}"
    );
    assert!(
        stdout.contains('(') && stdout.contains(')'),
        "version output should contain parenthesized metadata: {stdout}"
    );
}

#[test]
fn test_version_contains_git_hash() {
    let output = wh_binary()
        .arg("--version")
        .output()
        .expect("failed to run wh");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Extract content between parentheses
    let paren_content = stdout
        .split('(')
        .nth(1)
        .and_then(|s| s.split(')').next())
        .expect("should have parenthesized content");

    let parts: Vec<&str> = paren_content.split(", ").collect();
    assert!(
        parts.len() >= 2,
        "should have at least hash and target: {paren_content}"
    );

    let hash = parts[0].trim();
    // Git short hash is 7-12 hex chars, or "unknown" if built without git
    assert!(
        hash == "unknown" || (hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit())),
        "git hash should be hex or 'unknown': {hash}"
    );
}

#[test]
fn test_version_contains_target_triple() {
    let output = wh_binary()
        .arg("--version")
        .output()
        .expect("failed to run wh");

    let stdout = String::from_utf8_lossy(&output.stdout);

    let paren_content = stdout
        .split('(')
        .nth(1)
        .and_then(|s| s.split(')').next())
        .expect("should have parenthesized content");

    let parts: Vec<&str> = paren_content.split(", ").collect();
    let target = parts.last().expect("should have target triple").trim();

    // Target triple contains at least two dashes: arch-vendor-os or arch-vendor-os-env
    let dash_count = target.chars().filter(|&c| c == '-').count();
    assert!(
        dash_count >= 2,
        "target triple should have >= 2 dashes: {target}"
    );
}

// --- AC#5: bare wh invocation ---

#[test]
fn test_no_args_prints_getting_started_hint() {
    let output = wh_binary().output().expect("failed to run wh");

    assert!(output.status.success(), "bare wh should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Wheelhouse"),
        "hint should mention Wheelhouse: {stdout}"
    );
    assert!(
        stdout.contains("wh secrets init") || stdout.contains("wh deploy"),
        "hint should contain getting-started commands: {stdout}"
    );
}

#[test]
fn test_hint_is_five_lines_or_fewer() {
    let output = wh_binary().output().expect("failed to run wh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line_count = stdout.trim().lines().count();

    assert!(
        line_count <= 5,
        "hint should be <= 5 lines, got {line_count}: {stdout}"
    );
}

// --- AC#3: release workflow validation ---

#[test]
fn test_release_workflow_is_valid_yaml() {
    let workflow_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows/release.yml"
    );
    let content = std::fs::read_to_string(workflow_path)
        .expect("release.yml should exist at .github/workflows/release.yml");

    // Basic YAML validity: contains expected top-level keys
    assert!(
        content.contains("name:"),
        "release.yml should have a name field"
    );
    assert!(
        content.contains("on:"),
        "release.yml should have an on trigger"
    );
    assert!(content.contains("jobs:"), "release.yml should have jobs");
}

#[test]
fn test_release_workflow_has_build_matrix() {
    let workflow_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows/release.yml"
    );
    let content = std::fs::read_to_string(workflow_path).expect("release.yml should exist");

    assert!(
        content.contains("x86_64-unknown-linux-musl"),
        "release.yml should target x86_64-unknown-linux-musl (AM-01)"
    );
    assert!(
        content.contains("aarch64-apple-darwin"),
        "release.yml should target aarch64-apple-darwin (AM-01)"
    );
}

#[test]
#[ignore = "SLSA attestation requires paid org plan — disabled until available"]
fn test_release_workflow_has_slsa_attestation() {
    let workflow_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows/release.yml"
    );
    let content = std::fs::read_to_string(workflow_path).expect("release.yml should exist");

    assert!(
        content.contains("attest-build-provenance"),
        "release.yml should use actions/attest-build-provenance (SA-05)"
    );
}

// --- AC#2: install script ---

#[test]
fn test_install_script_exists() {
    let script_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/install.sh");
    let metadata = std::fs::metadata(script_path).expect("scripts/install.sh should exist");

    assert!(metadata.len() > 0, "install.sh should not be empty");

    // Check it starts with a shebang
    let content = std::fs::read_to_string(script_path).unwrap();
    assert!(
        content.starts_with("#!/"),
        "install.sh should start with a shebang"
    );
}

// --- .cargo/config.toml validation ---

#[test]
fn test_cargo_config_has_musl_target() {
    let config_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.cargo/config.toml");
    let content =
        std::fs::read_to_string(config_path).expect(".cargo/config.toml should exist (AM-02)");

    assert!(
        content.contains("x86_64-unknown-linux-musl"),
        ".cargo/config.toml should configure musl target"
    );
}
