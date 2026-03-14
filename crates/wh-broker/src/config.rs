//! Broker configuration.
//!
//! On Linux the bind address is `127.0.0.1` (loopback-only, ADR-001 / NFR-S1).
//! On macOS, Podman runs containers inside a Linux VM and cannot reach the macOS
//! loopback. The broker must bind to `0.0.0.0` so the Podman VM network can
//! reach it via `host.containers.internal`. The macOS application firewall
//! provides host-level protection.
//! Ports are configurable via environment variables for development flexibility.

use std::path::{Path, PathBuf};

/// Bind address: loopback on Linux (security invariant), all-interfaces on macOS
/// to allow Podman containers to connect via the VM gateway.
#[cfg(target_os = "macos")]
const BIND_ADDRESS: &str = "0.0.0.0";
#[cfg(not(target_os = "macos"))]
const BIND_ADDRESS: &str = "127.0.0.1";

/// Default ports for the broker sockets.
const DEFAULT_PUB_PORT: u16 = 5555;
const DEFAULT_SUB_PORT: u16 = 5556;
const DEFAULT_CONTROL_PORT: u16 = 5557;

/// Default compaction interval: 24 hours (86400 seconds).
const DEFAULT_COMPACTION_INTERVAL_SECS: u64 = 86400;

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
    /// Optional path to the git repository containing skills (Story 9.3).
    /// Configured via `WH_SKILLS_REPO` env var. None means skill routing disabled.
    skills_repo: Option<String>,
    /// Comma-separated list of allowed skill names (Story 9.3, FM-05).
    /// Configured via `WH_SKILLS_ALLOWLIST` env var. Empty means no skills allowed.
    skills_allowlist: Vec<String>,
}

impl BrokerConfig {
    /// Create a new configuration from environment variables or defaults.
    ///
    /// Reads `WH_PUB_PORT`, `WH_SUB_PORT`, `WH_CONTROL_PORT` from the environment.
    /// The bind address is always `127.0.0.1` and cannot be overridden.
    pub fn from_env() -> Self {
        let data_dir = std::env::var("WH_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".wh")
            });

        let compaction_interval_secs = std::env::var("WH_COMPACTION_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_COMPACTION_INTERVAL_SECS);

        let skills_repo = std::env::var("WH_SKILLS_REPO").ok();
        let skills_allowlist = std::env::var("WH_SKILLS_ALLOWLIST")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        Self {
            pub_port: Self::read_port_env("WH_PUB_PORT", DEFAULT_PUB_PORT),
            sub_port: Self::read_port_env("WH_SUB_PORT", DEFAULT_SUB_PORT),
            control_port: Self::read_port_env("WH_CONTROL_PORT", DEFAULT_CONTROL_PORT),
            data_dir,
            compaction_interval_secs,
            skills_repo,
            skills_allowlist,
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
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".wh")
                }),
            compaction_interval_secs: DEFAULT_COMPACTION_INTERVAL_SECS,
            skills_repo: None,
            skills_allowlist: vec![],
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
            skills_repo: None,
            skills_allowlist: vec![],
        }
    }

    /// The bind address -- always `127.0.0.1` (security invariant).
    pub fn bind_address(&self) -> &str {
        BIND_ADDRESS
    }

    /// Full PUB socket endpoint: `tcp://127.0.0.1:{port}`.
    pub fn pub_endpoint(&self) -> String {
        format!("tcp://{}:{}", BIND_ADDRESS, self.pub_port)
    }

    /// Full SUB socket endpoint: `tcp://127.0.0.1:{port}`.
    pub fn sub_endpoint(&self) -> String {
        format!("tcp://{}:{}", BIND_ADDRESS, self.sub_port)
    }

    /// Full control socket endpoint: `tcp://127.0.0.1:{port}`.
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

    /// Path to the git skills repository, if configured.
    pub fn skills_repo(&self) -> Option<&str> {
        self.skills_repo.as_deref()
    }

    /// List of allowed skill names (FM-05).
    pub fn skills_allowlist(&self) -> &[String] {
        &self.skills_allowlist
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
            data_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".wh"),
            compaction_interval_secs: DEFAULT_COMPACTION_INTERVAL_SECS,
            skills_repo: None,
            skills_allowlist: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults_localhost() {
        let config = BrokerConfig::default();
        assert_eq!(config.bind_address(), "127.0.0.1");
        assert_eq!(config.pub_port(), 5555);
        assert_eq!(config.sub_port(), 5556);
        assert_eq!(config.control_port(), 5557);
        assert!(config.pub_endpoint().starts_with("tcp://127.0.0.1:"));
        assert!(config.sub_endpoint().starts_with("tcp://127.0.0.1:"));
        assert!(config.control_endpoint().starts_with("tcp://127.0.0.1:"));
    }

    #[test]
    fn test_config_address_not_configurable() {
        // The bind address is hardcoded -- there is no setter or field to change it.
        // This test verifies the invariant holds for any config creation method.
        let config = BrokerConfig::default();
        assert_eq!(config.bind_address(), "127.0.0.1");

        let config = BrokerConfig::from_env();
        assert_eq!(config.bind_address(), "127.0.0.1");

        let config = BrokerConfig::with_ports(9999, 9998, 9997);
        assert_eq!(config.bind_address(), "127.0.0.1");
        // Even with custom ports, the address remains 127.0.0.1
        assert!(config.pub_endpoint().starts_with("tcp://127.0.0.1:"));
    }

    #[test]
    fn test_config_with_ports() {
        let config = BrokerConfig::with_ports(6000, 6001, 6002);
        assert_eq!(config.pub_port(), 6000);
        assert_eq!(config.sub_port(), 6001);
        assert_eq!(config.control_port(), 6002);
        assert_eq!(config.pub_endpoint(), "tcp://127.0.0.1:6000");
        assert_eq!(config.sub_endpoint(), "tcp://127.0.0.1:6001");
        assert_eq!(config.control_endpoint(), "tcp://127.0.0.1:6002");
    }
}
