//! Broker configuration.
//!
//! The broker runs inside a Podman container on an isolated topology network
//! (ADR-025, supersedes ADR-001). It binds `0.0.0.0` inside the container —
//! this is safe because the topology network provides isolation and ports are
//! published on `127.0.0.1` only on the host.
//!
//! Ports are configurable via environment variables for development flexibility.
//! Default data directory is `/data` (container convention, mounted from a
//! named Podman volume).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Bind address: always `0.0.0.0` inside the container (ADR-025).
///
/// The broker runs on an isolated Podman network. Security is enforced by
/// network isolation (ADR-024) and `127.0.0.1`-only port publishing on the host.
const BIND_ADDRESS: &str = "0.0.0.0";

/// Default ports for the broker sockets.
const DEFAULT_PUB_PORT: u16 = 5555;
const DEFAULT_SUB_PORT: u16 = 5556;
const DEFAULT_CONTROL_PORT: u16 = 5557;

/// Default compaction interval: 24 hours (86400 seconds).
const DEFAULT_COMPACTION_INTERVAL_SECS: u64 = 86400;

/// Default topology directory inside the broker container (ADR-035, FR79).
const DEFAULT_TOPOLOGY_DIR: &str = "/topology";

/// Broker configuration with localhost-only security invariant.
#[derive(Debug, Clone)]
pub struct BrokerConfig {
    pub_port: u16,
    sub_port: u16,
    control_port: u16,
    /// Data directory for WAL files and stream registry.
    /// Configured via `WH_DATA_DIR` env var, default `$HOME/.wh/`.
    data_dir: PathBuf,
    /// Compaction interval in seconds.
    /// Configured via `WH_COMPACTION_INTERVAL_SECS` env var, default 86400 (24h).
    compaction_interval_secs: u64,
    /// Optional filesystem path to skill definitions (volume mount point).
    /// Configured via `WH_SKILLS_PATH` env var. None means skill routing disabled.
    skills_path: Option<String>,
    /// Comma-separated list of allowed skill names (Story 9.3, FM-05).
    /// Configured via `WH_SKILLS_ALLOWLIST` env var. Empty means no skills allowed.
    skills_allowlist: Vec<String>,
    /// Topology folder path — working directory for `wh-cli` commands (ADR-035, FR79).
    /// Configured via `WH_TOPOLOGY_DIR` env var, default `/topology`.
    topology_dir: PathBuf,
    /// Agent permissions map: agent_name -> topology_edit (ADR-035, FR77).
    /// Configured via `WH_AGENT_PERMISSIONS` env var as comma-separated
    /// `agent_name:topology_edit` pairs (e.g., `"donna:true,researcher:false"`).
    agent_permissions: HashMap<String, bool>,
}

impl BrokerConfig {
    /// Create a new configuration from environment variables or defaults.
    ///
    /// Reads `WH_PUB_PORT`, `WH_SUB_PORT`, `WH_CONTROL_PORT` from the environment.
    /// The bind address is always `0.0.0.0` (container context, ADR-025).
    /// Default data directory is `/data` (container volume mount point).
    pub fn from_env() -> Self {
        let data_dir = std::env::var("WH_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/data"));

        let compaction_interval_secs = std::env::var("WH_COMPACTION_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_COMPACTION_INTERVAL_SECS);

        let skills_path = std::env::var("WH_SKILLS_PATH").ok();
        let skills_allowlist = std::env::var("WH_SKILLS_ALLOWLIST")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let topology_dir = std::env::var("WH_TOPOLOGY_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_TOPOLOGY_DIR));

        let agent_permissions = Self::parse_agent_permissions(
            &std::env::var("WH_AGENT_PERMISSIONS").unwrap_or_default(),
        );

        Self {
            pub_port: Self::read_port_env("WH_PUB_PORT", DEFAULT_PUB_PORT),
            sub_port: Self::read_port_env("WH_SUB_PORT", DEFAULT_SUB_PORT),
            control_port: Self::read_port_env("WH_CONTROL_PORT", DEFAULT_CONTROL_PORT),
            data_dir,
            compaction_interval_secs,
            skills_path,
            skills_allowlist,
            topology_dir,
            agent_permissions,
        }
    }

    /// Create a configuration with specific ports (for testing).
    pub fn with_ports(pub_port: u16, sub_port: u16, control_port: u16) -> Self {
        Self {
            pub_port,
            sub_port,
            control_port,
            data_dir: std::env::var("WH_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/data")),
            compaction_interval_secs: DEFAULT_COMPACTION_INTERVAL_SECS,
            skills_path: None,
            skills_allowlist: vec![],
            topology_dir: PathBuf::from(DEFAULT_TOPOLOGY_DIR),
            agent_permissions: HashMap::new(),
        }
    }

    /// Create a configuration with specific ports and data directory (for testing).
    pub fn with_ports_and_data_dir(
        pub_port: u16,
        sub_port: u16,
        control_port: u16,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            pub_port,
            sub_port,
            control_port,
            data_dir,
            compaction_interval_secs: DEFAULT_COMPACTION_INTERVAL_SECS,
            skills_path: None,
            skills_allowlist: vec![],
            topology_dir: PathBuf::from(DEFAULT_TOPOLOGY_DIR),
            agent_permissions: HashMap::new(),
        }
    }

    /// The bind address — always `0.0.0.0` inside the container (ADR-025).
    pub fn bind_address(&self) -> &str {
        BIND_ADDRESS
    }

    /// Full PUB socket endpoint: `tcp://0.0.0.0:{port}`.
    pub fn pub_endpoint(&self) -> String {
        format!("tcp://{}:{}", BIND_ADDRESS, self.pub_port)
    }

    /// Full SUB socket endpoint: `tcp://0.0.0.0:{port}`.
    pub fn sub_endpoint(&self) -> String {
        format!("tcp://{}:{}", BIND_ADDRESS, self.sub_port)
    }

    /// Full control socket endpoint: `tcp://0.0.0.0:{port}`.
    pub fn control_endpoint(&self) -> String {
        format!("tcp://{}:{}", BIND_ADDRESS, self.control_port)
    }

    /// PUB port number.
    pub fn pub_port(&self) -> u16 {
        self.pub_port
    }

    /// SUB port number.
    pub fn sub_port(&self) -> u16 {
        self.sub_port
    }

    /// Control port number.
    pub fn control_port(&self) -> u16 {
        self.control_port
    }

    /// Data directory for WAL files and stream registry.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Compaction interval in seconds.
    pub fn compaction_interval_secs(&self) -> u64 {
        self.compaction_interval_secs
    }

    /// Filesystem path to skill definitions, if configured.
    pub fn skills_path(&self) -> Option<&str> {
        self.skills_path.as_deref()
    }

    /// List of allowed skill names (FM-05).
    pub fn skills_allowlist(&self) -> &[String] {
        &self.skills_allowlist
    }

    /// Topology folder path — working directory for `wh-cli` commands (ADR-035, FR79).
    pub fn topology_dir(&self) -> &Path {
        &self.topology_dir
    }

    /// Agent permissions map (ADR-035, FR77).
    pub fn agent_permissions(&self) -> &HashMap<String, bool> {
        &self.agent_permissions
    }

    /// Parse `WH_AGENT_PERMISSIONS` env var value into a HashMap.
    ///
    /// Format: comma-separated `agent_name:topology_edit` pairs.
    /// Example: `"donna:true,researcher:false"`.
    /// Invalid entries are silently skipped.
    fn parse_agent_permissions(value: &str) -> HashMap<String, bool> {
        value
            .split(',')
            .filter_map(|pair| {
                let pair = pair.trim();
                if pair.is_empty() {
                    return None;
                }
                let mut parts = pair.splitn(2, ':');
                let name = parts.next()?.trim();
                let perm = parts.next()?.trim();
                if name.is_empty() {
                    return None;
                }
                let topology_edit = perm.eq_ignore_ascii_case("true");
                Some((name.to_string(), topology_edit))
            })
            .collect()
    }

    fn read_port_env(var: &str, default: u16) -> u16 {
        std::env::var(var)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self {
            pub_port: DEFAULT_PUB_PORT,
            sub_port: DEFAULT_SUB_PORT,
            control_port: DEFAULT_CONTROL_PORT,
            data_dir: PathBuf::from("/data"),
            compaction_interval_secs: DEFAULT_COMPACTION_INTERVAL_SECS,
            skills_path: None,
            skills_allowlist: vec![],
            topology_dir: PathBuf::from(DEFAULT_TOPOLOGY_DIR),
            agent_permissions: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults_container() {
        let config = BrokerConfig::default();
        assert_eq!(config.bind_address(), "0.0.0.0");
        assert_eq!(config.pub_port(), 5555);
        assert_eq!(config.sub_port(), 5556);
        assert_eq!(config.control_port(), 5557);
        assert!(config.pub_endpoint().starts_with("tcp://0.0.0.0:"));
        assert!(config.sub_endpoint().starts_with("tcp://0.0.0.0:"));
        assert!(config.control_endpoint().starts_with("tcp://0.0.0.0:"));
        assert_eq!(config.data_dir(), std::path::Path::new("/data"));
    }

    #[test]
    fn test_config_bind_address_always_all_interfaces() {
        // ADR-025: broker runs inside a container on an isolated network.
        // Bind address is always 0.0.0.0 — security is via network isolation.
        let config = BrokerConfig::default();
        assert_eq!(config.bind_address(), "0.0.0.0");

        let config = BrokerConfig::from_env();
        assert_eq!(config.bind_address(), "0.0.0.0");

        let config = BrokerConfig::with_ports(9999, 9998, 9997);
        assert_eq!(config.bind_address(), "0.0.0.0");
        assert!(config.pub_endpoint().starts_with("tcp://0.0.0.0:"));
    }

    #[test]
    fn test_config_with_ports() {
        let config = BrokerConfig::with_ports(6000, 6001, 6002);
        assert_eq!(config.pub_port(), 6000);
        assert_eq!(config.sub_port(), 6001);
        assert_eq!(config.control_port(), 6002);
        assert_eq!(config.pub_endpoint(), "tcp://0.0.0.0:6000");
        assert_eq!(config.sub_endpoint(), "tcp://0.0.0.0:6001");
        assert_eq!(config.control_endpoint(), "tcp://0.0.0.0:6002");
    }

    #[test]
    fn test_config_default_topology_dir() {
        let config = BrokerConfig::default();
        assert_eq!(config.topology_dir(), std::path::Path::new("/topology"));
    }

    #[test]
    fn test_config_default_agent_permissions_empty() {
        let config = BrokerConfig::default();
        assert!(config.agent_permissions().is_empty());
    }

    #[test]
    fn test_parse_agent_permissions_valid() {
        let perms = BrokerConfig::parse_agent_permissions("donna:true,researcher:false");
        assert_eq!(perms.len(), 2);
        assert_eq!(perms.get("donna"), Some(&true));
        assert_eq!(perms.get("researcher"), Some(&false));
    }

    #[test]
    fn test_parse_agent_permissions_with_spaces() {
        let perms = BrokerConfig::parse_agent_permissions(" donna : true , researcher : false ");
        assert_eq!(perms.len(), 2);
        assert_eq!(perms.get("donna"), Some(&true));
        assert_eq!(perms.get("researcher"), Some(&false));
    }

    #[test]
    fn test_parse_agent_permissions_empty_string() {
        let perms = BrokerConfig::parse_agent_permissions("");
        assert!(perms.is_empty());
    }

    #[test]
    fn test_parse_agent_permissions_invalid_entries_skipped() {
        let perms = BrokerConfig::parse_agent_permissions("donna:true,invalid,researcher:false");
        assert_eq!(perms.len(), 2);
        assert_eq!(perms.get("donna"), Some(&true));
        assert_eq!(perms.get("researcher"), Some(&false));
    }

    #[test]
    fn test_parse_agent_permissions_case_insensitive() {
        let perms = BrokerConfig::parse_agent_permissions("donna:TRUE,bob:True");
        assert_eq!(perms.get("donna"), Some(&true));
        assert_eq!(perms.get("bob"), Some(&true));
    }
}
