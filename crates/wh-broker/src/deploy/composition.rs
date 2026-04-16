//! Composition loader for subsystem expansion (ADR-045).
//!
//! Expands subsystem declarations into concrete topology primitives:
//! agents, streams, volume mounts, and environment variables. The first
//! supported subsystem is `llm-wiki` (ADR-045), which generates a librarian
//! agent, end-of-turn streams, and per-member RO volume mounts.

use std::collections::BTreeMap;

use crate::deploy::{Agent, DeployError, Stream, SubsystemDecl, Topology, VolumeMount};

/// Default librarian container image (ADR-041).
const LIBRARIAN_IMAGE: &str = "ghcr.io/wheelhouse-paris/wh-librarian:latest";

/// Mount path for the Library volume inside containers (ADR-045).
const LIBRARY_MOUNT_PATH: &str = "/workspace/.library";

/// Expand all subsystem declarations in a topology into concrete primitives.
///
/// For each subsystem declaration:
/// 1. Resolves template variables (`library_name` defaults to topology name)
/// 2. Generates a librarian agent with RW volume mount
/// 3. Generates end-of-turn streams for each member
/// 4. Augments member agents with RO volume mounts and env vars
/// 5. Validates no duplicate names after expansion
///
/// Returns a new `Topology` with subsystems expanded and the `subsystems` vec emptied.
pub fn expand_subsystems(mut topology: Topology) -> Result<Topology, DeployError> {
    if topology.subsystems.is_empty() {
        return Ok(topology);
    }

    let subsystems = std::mem::take(&mut topology.subsystems);

    for decl in &subsystems {
        expand_llm_wiki(&mut topology, decl)?;
    }

    Ok(topology)
}

/// Expand a single `llm-wiki` subsystem declaration.
fn expand_llm_wiki(topology: &mut Topology, decl: &SubsystemDecl) -> Result<(), DeployError> {
    // Validate path is a recognized subsystem
    let path = decl.path.trim_end_matches('/');
    if path != "subsystems/llm-wiki" {
        return Err(DeployError::SubsystemExpansionFailed(format!(
            "unknown subsystem path '{}', expected 'subsystems/llm-wiki/'",
            decl.path
        )));
    }

    // Validate members is non-empty
    if decl.members.is_empty() {
        return Err(DeployError::SubsystemExpansionFailed(
            "subsystem 'llm-wiki' requires at least one member agent".to_string(),
        ));
    }

    // Resolve library_name — default to topology name
    let library_name = decl
        .library_name
        .as_deref()
        .unwrap_or(&topology.name)
        .to_string();

    let topo_name = &topology.name;

    // Validate all members exist in the topology's agent list
    for member in &decl.members {
        if !topology.agents.iter().any(|a| &a.name == member) {
            return Err(DeployError::SubsystemExpansionFailed(format!(
                "member '{member}' not found in topology agents"
            )));
        }
    }

    // Build volume name: wh-<topo>-llm-wiki-<library_name>
    let volume_name = format!("wh-{topo_name}-llm-wiki-{library_name}");

    // Check for duplicate agent name
    let librarian_name = format!("librarian-{library_name}");
    if topology.agents.iter().any(|a| a.name == librarian_name) {
        return Err(DeployError::SubsystemExpansionFailed(format!(
            "duplicate agent name '{librarian_name}' — already exists in topology"
        )));
    }

    // Generate end-of-turn streams for each member
    let mut eot_stream_names = Vec::new();
    for member in &decl.members {
        let stream_name = format!("eot-{member}-{library_name}");
        // Check for duplicate stream name
        if topology.streams.iter().any(|s| s.name == stream_name) {
            return Err(DeployError::SubsystemExpansionFailed(format!(
                "duplicate stream name '{stream_name}' — already exists in topology"
            )));
        }
        eot_stream_names.push(stream_name);
    }

    // Create the end-of-turn streams
    for stream_name in &eot_stream_names {
        topology.streams.push(Stream {
            name: stream_name.clone(),
            retention: None,
            description: Some(format!(
                "End-of-turn events for llm-wiki subsystem '{library_name}'"
            )),
        });
    }

    // Build librarian env vars
    let mut librarian_env = BTreeMap::new();
    librarian_env.insert("WH_STREAMS".to_string(), eot_stream_names.join(","));
    librarian_env.insert(
        "WH_LIBRARY_PATH".to_string(),
        LIBRARY_MOUNT_PATH.to_string(),
    );
    librarian_env.insert("WH_LIBRARY_ID".to_string(), library_name.clone());
    librarian_env.insert("WH_AGENT_NAME".to_string(), librarian_name.clone());

    // Locales
    if let Some(locales) = &decl.locales {
        librarian_env.insert("WH_LIBRARIAN_LOCALES".to_string(), locales.join(","));
    }

    // Decision prompt
    if let Some(prompt) = &decl.decision_prompt {
        librarian_env.insert("WH_DECISION_PROMPT".to_string(), prompt.clone());
    }

    // Create librarian agent with RW volume mount
    let librarian = Agent {
        name: librarian_name,
        image: LIBRARIAN_IMAGE.to_string(),
        replicas: 1,
        streams: eot_stream_names.clone(),
        persona: None,
        skills: None,
        topology_edit: None,
        volumes: vec![VolumeMount {
            name: volume_name.clone(),
            mount: LIBRARY_MOUNT_PATH.to_string(),
            mount_mode: Some("rw".to_string()),
        }],
        env: Some(librarian_env),
    };
    topology.agents.push(librarian);

    // Augment member agents: add RO volume mount + WH_LIBRARY_WRITE_STREAM env var
    for (i, member) in decl.members.iter().enumerate() {
        let agent = topology
            .agents
            .iter_mut()
            .find(|a| &a.name == member)
            .expect("member validated above");

        // Add RO volume mount
        agent.volumes.push(VolumeMount {
            name: volume_name.clone(),
            mount: LIBRARY_MOUNT_PATH.to_string(),
            mount_mode: Some("ro".to_string()),
        });

        // Add WH_LIBRARY_WRITE_STREAM env var
        let env = agent.env.get_or_insert_with(BTreeMap::new);
        env.insert(
            "WH_LIBRARY_WRITE_STREAM".to_string(),
            eot_stream_names[i].clone(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::parse_topology;

    /// Helper to build a minimal topology with agents and a subsystem.
    fn make_topology_with_subsystem(
        library_name: Option<&str>,
        locales: Option<Vec<String>>,
        members: Vec<&str>,
    ) -> Topology {
        let agents: Vec<Agent> = members
            .iter()
            .map(|name| Agent {
                name: name.to_string(),
                image: "agent-claude:latest".to_string(),
                replicas: 1,
                streams: vec!["main".to_string()],
                persona: None,
                skills: None,
                topology_edit: None,
                volumes: vec![],
                env: None,
            })
            .collect();

        let streams = vec![Stream {
            name: "main".to_string(),
            retention: None,
            description: None,
        }];

        let subsystem = SubsystemDecl {
            path: "subsystems/llm-wiki/".to_string(),
            members: members.iter().map(|s| s.to_string()).collect(),
            library_name: library_name.map(|s| s.to_string()),
            locales,
            decision_prompt: None,
        };

        Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "lab".to_string(),
            broker: None,
            skills_repo: None,
            agents,
            streams,
            surfaces: vec![],
            guardrails: None,
            subsystems: vec![subsystem],
        }
    }

    #[test]
    fn expand_full_subsystem() {
        let topo = make_topology_with_subsystem(Some("research"), None, vec!["agent-a", "agent-b"]);
        let expanded = expand_subsystems(topo).unwrap();

        // Subsystems vec is now empty
        assert!(expanded.subsystems.is_empty());

        // Librarian agent generated
        let librarian = expanded
            .agents
            .iter()
            .find(|a| a.name == "librarian-research")
            .expect("librarian agent");
        assert_eq!(librarian.image, LIBRARIAN_IMAGE);

        // Librarian has RW volume
        assert_eq!(librarian.volumes.len(), 1);
        assert_eq!(librarian.volumes[0].name, "wh-lab-llm-wiki-research");
        assert_eq!(librarian.volumes[0].mount_mode.as_deref(), Some("rw"));

        // Librarian env vars
        let env = librarian.env.as_ref().unwrap();
        assert!(env
            .get("WH_STREAMS")
            .unwrap()
            .contains("eot-agent-a-research"));
        assert!(env
            .get("WH_STREAMS")
            .unwrap()
            .contains("eot-agent-b-research"));
        assert_eq!(env.get("WH_LIBRARY_PATH").unwrap(), LIBRARY_MOUNT_PATH);
        assert_eq!(env.get("WH_LIBRARY_ID").unwrap(), "research");
        assert_eq!(env.get("WH_AGENT_NAME").unwrap(), "librarian-research");

        // 2 EOT streams
        assert!(expanded
            .streams
            .iter()
            .any(|s| s.name == "eot-agent-a-research"));
        assert!(expanded
            .streams
            .iter()
            .any(|s| s.name == "eot-agent-b-research"));

        // Member agents augmented with RO volume
        let agent_a = expanded
            .agents
            .iter()
            .find(|a| a.name == "agent-a")
            .unwrap();
        let vol = agent_a
            .volumes
            .iter()
            .find(|v| v.name == "wh-lab-llm-wiki-research")
            .unwrap();
        assert_eq!(vol.mount_mode.as_deref(), Some("ro"));
        let a_env = agent_a.env.as_ref().unwrap();
        assert_eq!(
            a_env.get("WH_LIBRARY_WRITE_STREAM").unwrap(),
            "eot-agent-a-research"
        );

        let agent_b = expanded
            .agents
            .iter()
            .find(|a| a.name == "agent-b")
            .unwrap();
        let vol_b = agent_b
            .volumes
            .iter()
            .find(|v| v.name == "wh-lab-llm-wiki-research")
            .unwrap();
        assert_eq!(vol_b.mount_mode.as_deref(), Some("ro"));
    }

    #[test]
    fn default_library_name_to_topology_name() {
        let topo = make_topology_with_subsystem(None, None, vec!["agent-a"]);
        let expanded = expand_subsystems(topo).unwrap();

        // Library name should be "lab" (topology name)
        assert!(expanded.agents.iter().any(|a| a.name == "librarian-lab"));
        assert!(expanded.streams.iter().any(|s| s.name == "eot-agent-a-lab"));

        let librarian = expanded
            .agents
            .iter()
            .find(|a| a.name == "librarian-lab")
            .unwrap();
        assert!(librarian
            .volumes
            .iter()
            .any(|v| v.name == "wh-lab-llm-wiki-lab"));
    }

    #[test]
    fn locales_propagated() {
        let topo = make_topology_with_subsystem(
            Some("research"),
            Some(vec!["en".to_string(), "fr".to_string(), "de".to_string()]),
            vec!["agent-a"],
        );
        let expanded = expand_subsystems(topo).unwrap();

        let librarian = expanded
            .agents
            .iter()
            .find(|a| a.name == "librarian-research")
            .unwrap();
        let env = librarian.env.as_ref().unwrap();
        assert_eq!(env.get("WH_LIBRARIAN_LOCALES").unwrap(), "en,fr,de");
    }

    #[test]
    fn duplicate_agent_name_rejected() {
        let mut topo = make_topology_with_subsystem(Some("research"), None, vec!["agent-a"]);
        // Add an agent that conflicts with the librarian name
        topo.agents.push(Agent {
            name: "librarian-research".to_string(),
            image: "some:image".to_string(),
            replicas: 1,
            streams: vec![],
            persona: None,
            skills: None,
            topology_edit: None,
            volumes: vec![],
            env: None,
        });

        let result = expand_subsystems(topo);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate agent name"), "error: {err}");
    }

    #[test]
    fn duplicate_stream_name_rejected() {
        let mut topo = make_topology_with_subsystem(Some("research"), None, vec!["agent-a"]);
        // Add a stream that conflicts
        topo.streams.push(Stream {
            name: "eot-agent-a-research".to_string(),
            retention: None,
            description: None,
        });

        let result = expand_subsystems(topo);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate stream name"), "error: {err}");
    }

    #[test]
    fn unknown_subsystem_path_rejected() {
        let mut topo = make_topology_with_subsystem(Some("research"), None, vec!["agent-a"]);
        topo.subsystems[0].path = "subsystems/unknown/".to_string();

        let result = expand_subsystems(topo);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown subsystem path"), "error: {err}");
    }

    #[test]
    fn member_not_in_agents_rejected() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "lab".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![Agent {
                name: "agent-a".to_string(),
                image: "agent-claude:latest".to_string(),
                replicas: 1,
                streams: vec![],
                persona: None,
                skills: None,
                topology_edit: None,
                volumes: vec![],
                env: None,
            }],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
            subsystems: vec![SubsystemDecl {
                path: "subsystems/llm-wiki/".to_string(),
                members: vec!["agent-a".to_string(), "agent-missing".to_string()],
                library_name: Some("research".to_string()),
                locales: None,
                decision_prompt: None,
            }],
        };

        let result = expand_subsystems(topo);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("agent-missing"), "error: {err}");
    }

    #[test]
    fn empty_subsystems_is_noop() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "lab".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
            subsystems: vec![],
        };
        let expanded = expand_subsystems(topo.clone()).unwrap();
        assert_eq!(expanded.agents.len(), 0);
        assert_eq!(expanded.streams.len(), 0);
    }

    #[test]
    fn decision_prompt_propagated() {
        let mut topo = make_topology_with_subsystem(Some("research"), None, vec!["agent-a"]);
        topo.subsystems[0].decision_prompt = Some("/custom/prompt.md".to_string());

        let expanded = expand_subsystems(topo).unwrap();
        let librarian = expanded
            .agents
            .iter()
            .find(|a| a.name == "librarian-research")
            .unwrap();
        let env = librarian.env.as_ref().unwrap();
        assert_eq!(env.get("WH_DECISION_PROMPT").unwrap(), "/custom/prompt.md");
    }

    // ── Story 14-4-2: EROFS propagation verification ──

    /// All members get RO mounts (EROFS enforced), librarian gets RW — with 3 members.
    #[test]
    fn erofs_all_members_ro_librarian_rw() {
        let topo =
            make_topology_with_subsystem(Some("wiki"), None, vec!["agent-a", "agent-b", "agent-c"]);
        let expanded = expand_subsystems(topo).unwrap();

        // Every member agent must have mount_mode = "ro"
        for name in &["agent-a", "agent-b", "agent-c"] {
            let agent = expanded
                .agents
                .iter()
                .find(|a| a.name == *name)
                .unwrap_or_else(|| panic!("{name} should exist"));
            let vol = agent
                .volumes
                .iter()
                .find(|v| v.name.contains("llm-wiki"))
                .unwrap_or_else(|| panic!("{name} should have library volume"));
            assert_eq!(
                vol.mount_mode.as_deref(),
                Some("ro"),
                "{name} must have RO mount (EROFS enforced)"
            );
        }

        // Librarian must have mount_mode = "rw"
        let librarian = expanded
            .agents
            .iter()
            .find(|a| a.name == "librarian-wiki")
            .expect("librarian-wiki should exist");
        assert_eq!(
            librarian.volumes[0].mount_mode.as_deref(),
            Some("rw"),
            "librarian must have RW mount"
        );
    }

    /// Mount args generated from expanded composition produce correct :ro/:rw suffixes.
    #[test]
    fn erofs_mount_args_from_composition() {
        let topo = make_topology_with_subsystem(Some("docs"), None, vec!["reader"]);
        let expanded = expand_subsystems(topo).unwrap();

        let reader = expanded.agents.iter().find(|a| a.name == "reader").unwrap();
        let reader_args = super::super::podman::build_volume_mount_args(&reader.volumes);
        assert!(
            reader_args.iter().any(|a| a.ends_with(":ro")),
            "reader mount args must include :ro suffix: {reader_args:?}"
        );

        let librarian = expanded
            .agents
            .iter()
            .find(|a| a.name.starts_with("librarian"))
            .unwrap();
        let lib_args = super::super::podman::build_volume_mount_args(&librarian.volumes);
        assert!(
            !lib_args.iter().any(|a| a.ends_with(":ro")),
            "librarian mount args must NOT include :ro suffix: {lib_args:?}"
        );
    }

    #[test]
    fn parse_topology_with_subsystems() {
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
    locales: [en, fr]
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.subsystems.len(), 1);
        assert_eq!(topo.subsystems[0].path, "subsystems/llm-wiki/");
        assert_eq!(topo.subsystems[0].members, vec!["agent-a"]);
        assert_eq!(
            topo.subsystems[0].library_name,
            Some("research".to_string())
        );
        assert_eq!(
            topo.subsystems[0].locales,
            Some(vec!["en".to_string(), "fr".to_string()])
        );
    }
}
