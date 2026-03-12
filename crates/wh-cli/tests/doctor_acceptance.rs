//! Acceptance tests for `wh doctor` command (FM-07).
//!
//! AC #3: `wh doctor` detects git repo health and warns if secrets patterns found.

use wh_cli::commands::doctor::DoctorArgs;
use wh_cli::output::OutputFormat;

/// AC #3: doctor detects missing .git directory.
#[test]
fn doctor_detects_missing_git_directory() {
    let dir = tempfile::tempdir().unwrap();
    let args = DoctorArgs {
        format: OutputFormat::Human,
        path: dir.path().to_path_buf(),
    };
    // Should return exit code 1 (failure) when .git is missing
    let exit_code = args.execute();
    assert_eq!(exit_code, 1, "doctor should fail when .git is missing");
}

/// AC #3: doctor detects missing .wh/.gitignore.
#[test]
fn doctor_detects_missing_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let temp_path = dir.path();

    // Create .git directory to pass first check
    std::fs::create_dir_all(temp_path.join(".git")).unwrap();

    let args = DoctorArgs {
        format: OutputFormat::Human,
        path: temp_path.to_path_buf(),
    };
    // Should pass (no fail checks) but with warnings
    let exit_code = args.execute();
    assert_eq!(exit_code, 0, "doctor should pass (with warnings) when .wh/.gitignore is missing");
}

/// AC #3: doctor warns when gitignore is incomplete.
#[test]
fn doctor_warns_on_incomplete_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let temp_path = dir.path();

    // Create .git and partial .wh/.gitignore
    std::fs::create_dir_all(temp_path.join(".git")).unwrap();
    let wh_dir = temp_path.join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(wh_dir.join(".gitignore"), "*.db\n").unwrap();

    let args = DoctorArgs {
        format: OutputFormat::Json,
        path: temp_path.to_path_buf(),
    };
    let exit_code = args.execute();
    assert_eq!(exit_code, 0, "incomplete gitignore is a warning, not a failure");
}

/// AC #3: doctor passes when everything is healthy.
#[test]
fn doctor_passes_on_healthy_repo() {
    let dir = tempfile::tempdir().unwrap();
    let temp_path = dir.path();

    // Set up a proper git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(temp_path)
        .output()
        .unwrap();

    // Create complete .wh/.gitignore
    wh_broker::deploy::gitignore::ensure_gitignore(temp_path).unwrap();

    let args = DoctorArgs {
        format: OutputFormat::Human,
        path: temp_path.to_path_buf(),
    };
    let exit_code = args.execute();
    assert_eq!(exit_code, 0, "doctor should pass on healthy repo");
}
