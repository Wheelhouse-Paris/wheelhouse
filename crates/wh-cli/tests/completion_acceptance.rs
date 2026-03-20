//! Acceptance tests for `wh completion` and help/exit code behavior (Story 3.4).
//!
//! These tests verify shell completion generation, offline help, and semantic exit codes.

use std::process::Command;

/// Helper to get the path to the `wh` binary.
fn wh_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wh"));
    cmd.current_dir(std::env::temp_dir());
    cmd.env("NO_COLOR", "1");
    cmd
}

// ============================================================
// AC #1: Shell completion — `wh completion <shell>` generates scripts
// ============================================================

/// AC #1: `wh completion bash` generates a bash completion script.
#[test]
fn completion_bash_generates_script() {
    let output = wh_bin()
        .args(["completion", "bash"])
        .output()
        .expect("failed to execute wh");

    assert!(
        output.status.success(),
        "Expected exit 0 for completion bash"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "Expected non-empty completion script output"
    );
    // Bash completion scripts reference the command name
    assert!(
        stdout.contains("wh"),
        "Expected bash completion to reference 'wh', got: {stdout}"
    );
}

/// AC #1: `wh completion zsh` generates a zsh completion script.
#[test]
fn completion_zsh_generates_script() {
    let output = wh_bin()
        .args(["completion", "zsh"])
        .output()
        .expect("failed to execute wh");

    assert!(
        output.status.success(),
        "Expected exit 0 for completion zsh"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "Expected non-empty completion script output"
    );
    // Zsh completion scripts contain #compdef
    assert!(
        stdout.contains("#compdef") || stdout.contains("wh"),
        "Expected zsh completion markers, got: {stdout}"
    );
}

/// AC #1: `wh completion fish` generates a fish completion script.
#[test]
fn completion_fish_generates_script() {
    let output = wh_bin()
        .args(["completion", "fish"])
        .output()
        .expect("failed to execute wh");

    assert!(
        output.status.success(),
        "Expected exit 0 for completion fish"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.is_empty(),
        "Expected non-empty completion script output"
    );
    // Fish completions use `complete -c <command>`
    assert!(
        stdout.contains("complete") || stdout.contains("wh"),
        "Expected fish completion markers, got: {stdout}"
    );
}

/// AC #1: `wh completion` without a shell argument prints usage error.
#[test]
fn completion_requires_shell_argument() {
    let output = wh_bin()
        .args(["completion"])
        .output()
        .expect("failed to execute wh");

    assert!(
        !output.status.success(),
        "Expected non-zero exit when shell not specified"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Usage") || stderr.contains("usage") || stderr.contains("required"),
        "Expected usage help in stderr, got: {stderr}"
    );
}

// ============================================================
// AC #2: Offline help — `wh <command> --help` works without broker
// ============================================================

/// AC #2: `wh --help` works offline and lists subcommands.
#[test]
fn wh_help_works_offline() {
    let output = wh_bin()
        .args(["--help"])
        .output()
        .expect("failed to execute wh");

    assert!(output.status.success(), "Expected exit 0 for --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should list known subcommands
    assert!(stdout.contains("ps"), "Expected 'ps' in help output");
    assert!(stdout.contains("logs"), "Expected 'logs' in help output");
    assert!(
        stdout.contains("topology"),
        "Expected 'deploy' in help output"
    );
    assert!(
        stdout.contains("completion"),
        "Expected 'completion' in help output"
    );
}

/// AC #2: `wh topology --help` works offline and lists deploy subcommands.
#[test]
fn wh_topology_help_works_offline() {
    let output = wh_bin()
        .args(["topology", "--help"])
        .output()
        .expect("failed to execute wh");

    assert!(output.status.success(), "Expected exit 0 for topology --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("lint"), "Expected 'lint' in topology help");
    assert!(stdout.contains("plan"), "Expected 'plan' in topology help");
    assert!(stdout.contains("apply"), "Expected 'apply' in topology help");
}

/// AC #2: `wh ps --help` works offline and shows format flag.
#[test]
fn wh_ps_help_works_offline() {
    let output = wh_bin()
        .args(["ps", "--help"])
        .output()
        .expect("failed to execute wh");

    assert!(output.status.success(), "Expected exit 0 for ps --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--format"),
        "Expected '--format' in ps help, got: {stdout}"
    );
}

/// AC #2: `wh completion --help` works offline.
#[test]
fn wh_completion_help_works_offline() {
    let output = wh_bin()
        .args(["completion", "--help"])
        .output()
        .expect("failed to execute wh");

    assert!(
        output.status.success(),
        "Expected exit 0 for completion --help"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("bash") || stdout.contains("zsh") || stdout.contains("completion"),
        "Expected shell names in completion help, got: {stdout}"
    );
}

// ============================================================
// AC #3 & #4: Semantic exit codes
// ============================================================

/// AC #3: `wh topology plan` with a topology exits with 0 (no change) or 2 (change detected).
/// This test uses the unchanged fixture which produces a valid plan.
#[test]
fn topology_plan_exits_zero_or_two() {
    // Use the existing fixture which is a valid .wh topology
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unchanged.wh");

    let output = Command::new(env!("CARGO_BIN_EXE_wh"))
        .args(["topology", "plan", &fixture.to_string_lossy()])
        .current_dir(std::env::temp_dir())
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to execute wh");

    let code = output.status.code().unwrap_or(-1);
    // With no existing state, this topology produces changes (exit 2) or no changes (exit 0).
    // Error (exit 1) would indicate a regression in the plan command.
    assert!(
        code == 0 || code == 2,
        "Expected exit 0 (no change) or 2 (change), got: {code}. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// AC #4: Error commands exit with code 1.
#[test]
fn topology_plan_invalid_file_exits_one() {
    let output = Command::new(env!("CARGO_BIN_EXE_wh"))
        .args(["topology", "plan", "/nonexistent/file.wh"])
        .current_dir(std::env::temp_dir())
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to execute wh");

    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 1, "Expected exit code 1 for error, got: {code}");
}

/// AC #4: `wh topology apply` with error exits 1.
#[test]
fn topology_apply_error_exits_one() {
    let output = Command::new(env!("CARGO_BIN_EXE_wh"))
        .args(["topology", "apply", "/nonexistent/file.wh", "--yes"])
        .current_dir(std::env::temp_dir())
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to execute wh");

    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 1, "Expected exit code 1 for error, got: {code}");
}
