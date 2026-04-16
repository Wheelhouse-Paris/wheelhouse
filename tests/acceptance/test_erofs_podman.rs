//! Podman EROFS integration test (Story 14-4-2, AC #1).
//!
//! Verifies that a Podman container with a `:ro` volume mount rejects
//! write attempts at the kernel level (EROFS). This test requires a
//! running Podman installation and is gated with `#[ignore]`.
//!
//! Run manually: `cargo test -p wh-broker --test test_erofs_podman -- --ignored`
//! Or in CI with Podman: remove the `#[ignore]` gate.

use std::process::Command;

/// Helper: check if `podman` is available on PATH.
fn podman_available() -> bool {
    Command::new("podman")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Given a Podman volume mounted with `:ro`,
/// When a process inside the container attempts to write a file,
/// Then the write fails with a non-zero exit code (EROFS kernel denial).
#[test]
#[ignore] // Requires real Podman — run with `--ignored` in Podman-enabled CI
fn erofs_write_rejected_on_ro_volume() {
    if !podman_available() {
        eprintln!("SKIP: podman not available");
        return;
    }

    // Create a temporary volume
    let vol_name = format!("wh-test-erofs-{}", std::process::id());
    let create_result = Command::new("podman")
        .args(["volume", "create", &vol_name])
        .output()
        .expect("podman volume create should run");
    assert!(
        create_result.status.success(),
        "volume create failed: {}",
        String::from_utf8_lossy(&create_result.stderr)
    );

    // Run a container with :ro mount and attempt to write
    let run_result = Command::new("podman")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{vol_name}:/mnt/lib:ro"),
            "alpine:3.19",
            "sh",
            "-c",
            "touch /mnt/lib/attack.md",
        ])
        .output()
        .expect("podman run should execute");

    // Cleanup: remove the volume
    let _ = Command::new("podman")
        .args(["volume", "rm", "-f", &vol_name])
        .output();

    // The write should have failed
    assert!(
        !run_result.status.success(),
        "write to :ro mount should fail with non-zero exit code"
    );
    let stderr = String::from_utf8_lossy(&run_result.stderr);
    // The error message should mention read-only filesystem
    assert!(
        stderr.contains("Read-only file system")
            || stderr.contains("EROFS")
            || stderr.contains("read-only"),
        "stderr should indicate read-only filesystem: {}",
        stderr
    );
}

/// Given a Podman volume mounted with `:ro`,
/// When a process reads from the mount,
/// Then the read succeeds (RO does not block reads).
#[test]
#[ignore] // Requires real Podman
fn ro_volume_allows_reads() {
    if !podman_available() {
        eprintln!("SKIP: podman not available");
        return;
    }

    let vol_name = format!("wh-test-erofs-read-{}", std::process::id());

    // Create volume and seed a file using an RW container
    let _ = Command::new("podman")
        .args(["volume", "create", &vol_name])
        .output()
        .expect("volume create");
    let seed = Command::new("podman")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{vol_name}:/mnt/lib"),
            "alpine:3.19",
            "sh",
            "-c",
            "echo hello > /mnt/lib/test.md",
        ])
        .output()
        .expect("seed run");
    assert!(seed.status.success(), "seeding file should succeed");

    // Read via RO mount
    let read_result = Command::new("podman")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{vol_name}:/mnt/lib:ro"),
            "alpine:3.19",
            "cat",
            "/mnt/lib/test.md",
        ])
        .output()
        .expect("read run");

    // Cleanup
    let _ = Command::new("podman")
        .args(["volume", "rm", "-f", &vol_name])
        .output();

    assert!(
        read_result.status.success(),
        "reading from :ro mount should succeed"
    );
    let stdout = String::from_utf8_lossy(&read_result.stdout);
    assert!(stdout.contains("hello"), "should read seeded content");
}
