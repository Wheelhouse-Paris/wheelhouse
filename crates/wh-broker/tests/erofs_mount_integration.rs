//! Integration tests for EROFS mount argument generation (Story 14-4-2).
//!
//! Validates that the deploy pipeline correctly generates Podman volume
//! mount arguments with `:ro` suffix for read-only mounts, enforcing
//! kernel-level EROFS on write attempts (ADR-043, NFR-14).

use wh_broker::deploy::podman::build_volume_mount_args;
use wh_broker::deploy::VolumeMount;

// ---------------------------------------------------------------------------
// build_volume_mount_args() — direct tests for the low-level function
// ---------------------------------------------------------------------------

/// RO mount should produce `-v name:path:ro` (EROFS enforcement).
#[test]
fn ro_mount_produces_ro_suffix() {
    let volumes = vec![VolumeMount {
        name: "wh-lab-llm-wiki-research".to_string(),
        mount: "/workspace/.library".to_string(),
        mount_mode: Some("ro".to_string()),
    }];
    let args = build_volume_mount_args(&volumes);
    assert_eq!(args.len(), 2, "should produce -v and mount spec");
    assert_eq!(args[0], "-v");
    assert_eq!(
        args[1], "wh-lab-llm-wiki-research:/workspace/.library:ro",
        "RO mount must include :ro suffix for EROFS enforcement"
    );
}

/// RW mount should NOT include `:ro` suffix.
#[test]
fn rw_mount_has_no_ro_suffix() {
    let volumes = vec![VolumeMount {
        name: "wh-lab-llm-wiki-research".to_string(),
        mount: "/workspace/.library".to_string(),
        mount_mode: Some("rw".to_string()),
    }];
    let args = build_volume_mount_args(&volumes);
    assert_eq!(args.len(), 2);
    assert_eq!(args[0], "-v");
    assert_eq!(
        args[1], "wh-lab-llm-wiki-research:/workspace/.library",
        "RW mount must NOT include :ro suffix"
    );
}

/// Default mount mode (None) should behave as RW — no `:ro` suffix.
#[test]
fn default_mount_mode_is_rw() {
    let volumes = vec![VolumeMount {
        name: "wh-lab-llm-wiki-research".to_string(),
        mount: "/workspace/.library".to_string(),
        mount_mode: None,
    }];
    let args = build_volume_mount_args(&volumes);
    assert_eq!(args.len(), 2);
    assert_eq!(
        args[1], "wh-lab-llm-wiki-research:/workspace/.library",
        "default (None) mount_mode should produce RW behavior"
    );
}

/// Mixed RO and RW volumes on the same agent produce correct suffixes.
#[test]
fn mixed_ro_rw_volumes_correct_suffixes() {
    let volumes = vec![
        VolumeMount {
            name: "lib-vol".to_string(),
            mount: "/workspace/.library".to_string(),
            mount_mode: Some("ro".to_string()),
        },
        VolumeMount {
            name: "data-vol".to_string(),
            mount: "/data".to_string(),
            mount_mode: Some("rw".to_string()),
        },
        VolumeMount {
            name: "shared-vol".to_string(),
            mount: "/shared".to_string(),
            mount_mode: None,
        },
    ];
    let args = build_volume_mount_args(&volumes);
    // 3 volumes x 2 args each = 6 args
    assert_eq!(args.len(), 6, "3 volumes should produce 6 args");

    // RO volume
    assert_eq!(args[1], "lib-vol:/workspace/.library:ro");
    // RW volume
    assert_eq!(args[3], "data-vol:/data");
    // Default (None) volume
    assert_eq!(args[5], "shared-vol:/shared");
}

/// Empty volumes list produces no arguments.
#[test]
fn empty_volumes_produces_no_args() {
    let args = build_volume_mount_args(&[]);
    assert!(args.is_empty(), "empty volumes should produce no args");
}
