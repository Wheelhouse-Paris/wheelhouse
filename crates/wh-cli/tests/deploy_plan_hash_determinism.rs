//! Acceptance tests for Story 7.1: Operator-Driven Plan/Apply Loop (Donna Mode)
//! AC #4: plan_hash is computed over canonical (sorted, whitespace-normalized) JSON
//!
//! TDD Red Phase: These tests MUST fail until implementation is complete.

use std::process::Command;

/// AC #4: Given the `plan_hash` field is computed,
/// When two semantically identical plans are hashed,
/// Then the hash is computed over the canonical (sorted, whitespace-normalized) JSON.
#[test]
fn plan_hash_is_deterministic_for_same_topology_diff() {
    // Run plan twice on the same input
    let output1 = Command::new(env!("CARGO_BIN_EXE_wh"))
        .args(["deploy", "plan", "tests/fixtures/modified.wh", "--format", "json"])
        .output()
        .expect("failed to execute wh binary");

    let output2 = Command::new(env!("CARGO_BIN_EXE_wh"))
        .args(["deploy", "plan", "tests/fixtures/modified.wh", "--format", "json"])
        .output()
        .expect("failed to execute wh binary");

    let json1: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&output1.stdout))
        .expect("first run should produce valid JSON");
    let json2: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&output2.stdout))
        .expect("second run should produce valid JSON");

    let hash1 = json1["data"]["plan_hash"].as_str().expect("plan_hash must be a string");
    let hash2 = json2["data"]["plan_hash"].as_str().expect("plan_hash must be a string");

    assert_eq!(hash1, hash2, "plan_hash must be deterministic for identical topology diffs");
    assert!(hash1.starts_with("sha256:"), "plan_hash must use sha256 prefix");
}

/// AC #4: Hash is over canonical JSON — not raw output string.
/// Different whitespace/key ordering in source should produce same hash.
#[test]
fn plan_hash_is_canonical_independent_of_field_ordering() {
    // This test uses two .wh files that are semantically identical but may differ in YAML key order
    let output1 = Command::new(env!("CARGO_BIN_EXE_wh"))
        .args(["deploy", "plan", "tests/fixtures/modified.wh", "--format", "json"])
        .output()
        .expect("failed to execute wh binary");

    let output2 = Command::new(env!("CARGO_BIN_EXE_wh"))
        .args(["deploy", "plan", "tests/fixtures/modified_reordered.wh", "--format", "json"])
        .output()
        .expect("failed to execute wh binary");

    let json1: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&output1.stdout))
        .expect("first run should produce valid JSON");
    let json2: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&output2.stdout))
        .expect("second run should produce valid JSON");

    let hash1 = json1["data"]["plan_hash"].as_str().expect("plan_hash must be a string");
    let hash2 = json2["data"]["plan_hash"].as_str().expect("plan_hash must be a string");

    assert_eq!(hash1, hash2, "semantically identical plans must produce the same canonical hash");
}
