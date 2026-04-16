//! Acceptance tests for story 14-3-1: Composition Loader Template Resolution
//! and Per-Member Expansion.
//!
//! These tests validate that subsystem declarations in `.wh` topology files are
//! correctly expanded into concrete agents, streams, and volume mounts by the
//! composition loader (ADR-045).
//!
//! TDD Red Phase: All tests should FAIL until the composition loader is implemented.

use wh_broker::deploy::{parse_topology, Topology};

// ---------------------------------------------------------------------------
// AC #1: Full subsystem expansion
// ---------------------------------------------------------------------------

/// Given a topology with `subsystems: [{path: subsystems/llm-wiki/, members: [agent-a, agent-b], library_name: research}]`
/// When the composition loader processes this declaration
/// Then it generates 1 named volume, 1 librarian container, 2 EOT streams, 2 RO mounts, and env vars.
#[test]
fn ac1_subsystem_expands_into_concrete_primitives() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: lab
agents:
  - name: agent-a
    image: agent-claude:latest
    streams: [main]
  - name: agent-b
    image: agent-claude:latest
    streams: [main]
streams:
  - name: main
subsystems:
  - path: subsystems/llm-wiki/
    members: [agent-a, agent-b]
    library_name: research
"#;
    let topo = parse_topology(yaml).expect("valid topology");
    let expanded = wh_broker::deploy::composition::expand_subsystems(topo)
        .expect("subsystem expansion should succeed");

    // 1 librarian container generated
    let librarian = expanded
        .agents
        .iter()
        .find(|a| a.name == "librarian-research")
        .expect("librarian-research agent should exist");

    // Librarian has RW volume mount
    let lib_vol = librarian
        .volumes
        .iter()
        .find(|v| v.name == "wh-lab-llm-wiki-research")
        .expect("librarian should have library volume");
    assert_eq!(
        lib_vol.mount_mode.as_deref().unwrap_or("rw"),
        "rw",
        "librarian volume should be RW"
    );

    // Librarian has correct WH_STREAMS env var
    let env = librarian.env.as_ref().expect("librarian should have env");
    let wh_streams = env.get("WH_STREAMS").expect("WH_STREAMS should be set");
    assert!(
        wh_streams.contains("eot-agent-a-research"),
        "WH_STREAMS should contain eot-agent-a-research"
    );
    assert!(
        wh_streams.contains("eot-agent-b-research"),
        "WH_STREAMS should contain eot-agent-b-research"
    );

    // 2 end-of-turn streams generated
    assert!(
        expanded.streams.iter().any(|s| s.name == "eot-agent-a-research"),
        "eot-agent-a-research stream should exist"
    );
    assert!(
        expanded.streams.iter().any(|s| s.name == "eot-agent-b-research"),
        "eot-agent-b-research stream should exist"
    );

    // 2 RO volume mounts on member agents
    let agent_a = expanded
        .agents
        .iter()
        .find(|a| a.name == "agent-a")
        .expect("agent-a should exist");
    let a_vol = agent_a
        .volumes
        .iter()
        .find(|v| v.name == "wh-lab-llm-wiki-research")
        .expect("agent-a should have library volume");
    assert_eq!(
        a_vol.mount_mode.as_deref().unwrap_or("rw"),
        "ro",
        "member agent volume should be RO"
    );

    // WH_LIBRARY_WRITE_STREAM set on member agents
    let a_env = agent_a.env.as_ref().expect("agent-a should have env");
    assert!(
        a_env.contains_key("WH_LIBRARY_WRITE_STREAM"),
        "agent-a should have WH_LIBRARY_WRITE_STREAM"
    );
}

// ---------------------------------------------------------------------------
// AC #2: Default library_name falls back to topology name
// ---------------------------------------------------------------------------

/// Given the `library_name` parameter is omitted
/// When the composition loader processes the declaration
/// Then it defaults `library_name` to the topology name.
#[test]
fn ac2_default_library_name_is_topology_name() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: lab
agents:
  - name: agent-a
    image: agent-claude:latest
    streams: [main]
streams:
  - name: main
subsystems:
  - path: subsystems/llm-wiki/
    members: [agent-a]
"#;
    let topo = parse_topology(yaml).expect("valid topology");
    let expanded = wh_broker::deploy::composition::expand_subsystems(topo)
        .expect("subsystem expansion should succeed");

    // Library name defaults to "lab" (topology name)
    let librarian = expanded
        .agents
        .iter()
        .find(|a| a.name == "librarian-lab")
        .expect("librarian-lab agent should exist (library_name defaulted to topology name)");

    // Streams use the topology name
    assert!(
        expanded.streams.iter().any(|s| s.name == "eot-agent-a-lab"),
        "eot stream should use topology name as library_name"
    );

    // Volume uses the topology name
    assert!(
        librarian
            .volumes
            .iter()
            .any(|v| v.name == "wh-lab-llm-wiki-lab"),
        "volume should use topology name as library_name"
    );
}

// ---------------------------------------------------------------------------
// AC #3: Locales parameter propagation
// ---------------------------------------------------------------------------

/// Given the `locales` parameter is set to `["en", "fr", "de"]`
/// When the librarian container is configured
/// Then `WH_LIBRARIAN_LOCALES=en,fr,de` is set in its environment.
#[test]
fn ac3_locales_parameter_propagated_to_librarian() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: lab
agents:
  - name: agent-a
    image: agent-claude:latest
    streams: [main]
streams:
  - name: main
subsystems:
  - path: subsystems/llm-wiki/
    members: [agent-a]
    library_name: research
    locales: [en, fr, de]
"#;
    let topo = parse_topology(yaml).expect("valid topology");
    let expanded = wh_broker::deploy::composition::expand_subsystems(topo)
        .expect("subsystem expansion should succeed");

    let librarian = expanded
        .agents
        .iter()
        .find(|a| a.name == "librarian-research")
        .expect("librarian should exist");
    let env = librarian.env.as_ref().expect("librarian should have env");
    let locales = env
        .get("WH_LIBRARIAN_LOCALES")
        .expect("WH_LIBRARIAN_LOCALES should be set");
    assert_eq!(locales, "en,fr,de");
}

// ---------------------------------------------------------------------------
// AC #4: Duplicate name detection
// ---------------------------------------------------------------------------

/// Given expanded primitives that collide with existing topology names
/// When they are merged
/// Then a lint/expansion error is returned.
#[test]
fn ac4_duplicate_name_collision_detected() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: lab
agents:
  - name: agent-a
    image: agent-claude:latest
    streams: [main]
  - name: librarian-research
    image: some-other:latest
streams:
  - name: main
subsystems:
  - path: subsystems/llm-wiki/
    members: [agent-a]
    library_name: research
"#;
    let topo = parse_topology(yaml).expect("valid topology");
    let result = wh_broker::deploy::composition::expand_subsystems(topo);
    assert!(
        result.is_err(),
        "should fail on duplicate agent name 'librarian-research'"
    );
}

// ---------------------------------------------------------------------------
// Story 14-4-2: EROFS mount mode propagation through composition
// ---------------------------------------------------------------------------

/// Given a subsystem declaration with multiple members,
/// When the composition loader expands it,
/// Then ALL member agents have RO mounts (EROFS enforced) and the librarian has RW.
#[test]
fn erofs_all_members_get_ro_mounts() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: prod
agents:
  - name: agent-a
    image: agent-claude:latest
    streams: [main]
  - name: agent-b
    image: agent-claude:latest
    streams: [main]
  - name: agent-c
    image: agent-claude:latest
    streams: [main]
streams:
  - name: main
subsystems:
  - path: subsystems/llm-wiki/
    members: [agent-a, agent-b, agent-c]
    library_name: wiki
"#;
    let topo = parse_topology(yaml).expect("valid topology");
    let expanded = wh_broker::deploy::composition::expand_subsystems(topo)
        .expect("subsystem expansion should succeed");

    // All 3 member agents must have RO volume mounts
    for member_name in &["agent-a", "agent-b", "agent-c"] {
        let agent = expanded
            .agents
            .iter()
            .find(|a| a.name == *member_name)
            .unwrap_or_else(|| panic!("{member_name} should exist"));
        let vol = agent
            .volumes
            .iter()
            .find(|v| v.name.contains("llm-wiki"))
            .unwrap_or_else(|| panic!("{member_name} should have library volume"));
        assert_eq!(
            vol.mount_mode.as_deref().unwrap_or("rw"),
            "ro",
            "{member_name} library volume must be RO (EROFS enforced)"
        );
    }

    // Librarian must have RW mount
    let librarian = expanded
        .agents
        .iter()
        .find(|a| a.name == "librarian-wiki")
        .expect("librarian-wiki should exist");
    let lib_vol = librarian
        .volumes
        .iter()
        .find(|v| v.name.contains("llm-wiki"))
        .expect("librarian should have library volume");
    assert_eq!(
        lib_vol.mount_mode.as_deref().unwrap_or("rw"),
        "rw",
        "librarian library volume must be RW"
    );
}

/// Given a subsystem expansion,
/// When mount args are generated from the expanded volumes,
/// Then RO member volumes produce `:ro` suffix and RW librarian does not.
#[test]
fn erofs_mount_args_match_composition_modes() {
    let yaml = r#"
api_version: wheelhouse.dev/v1
name: lab
agents:
  - name: reader
    image: agent-claude:latest
    streams: [main]
streams:
  - name: main
subsystems:
  - path: subsystems/llm-wiki/
    members: [reader]
    library_name: docs
"#;
    let topo = parse_topology(yaml).expect("valid topology");
    let expanded = wh_broker::deploy::composition::expand_subsystems(topo)
        .expect("subsystem expansion should succeed");

    // Reader agent should have RO volume
    let reader = expanded
        .agents
        .iter()
        .find(|a| a.name == "reader")
        .expect("reader should exist");
    let reader_args = wh_broker::deploy::podman::build_volume_mount_args(&reader.volumes);
    assert!(
        reader_args.iter().any(|a| a.ends_with(":ro")),
        "reader volume mount args must include :ro suffix: {:?}",
        reader_args
    );

    // Librarian should have RW volume
    let librarian = expanded
        .agents
        .iter()
        .find(|a| a.name.starts_with("librarian"))
        .expect("librarian should exist");
    let librarian_args =
        wh_broker::deploy::podman::build_volume_mount_args(&librarian.volumes);
    assert!(
        !librarian_args.iter().any(|a| a.ends_with(":ro")),
        "librarian volume mount args must NOT include :ro suffix: {:?}",
        librarian_args
    );
}

