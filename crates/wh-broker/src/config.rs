//! Broker configuration.
//!
//! The bind address is hardcoded to `127.0.0.1` as a security invariant (ADR-001, NFR-S1).
//! This is NOT configurable -- the broker only accepts localhost connections.
//! Ports are configurable via environment variables for development flexibility.

/// The localhost bind address -- security invariant, never changes.
const BIND_ADDRESS: &str = "127.0.0.1";

/// Default ports for the broker sockets.
const DEFAULT_PUB_PORT: u16 = 5555;
const DEFAULT_SUB_PORT: u16 = 5556;
const DEFAULT_CONTROL_PORT: u16 = 5557;

/// Broker configuration with localhost-only security invariant.
#[derive(Debug, Clone)]
pub struct BrokerConfig {
    pub_port: u16,
    sub_port: u16,
    control_port: u16,
}

impl BrokerConfig {
    /// Create a new configuration from environment variables or defaults.
    ///
    /// Reads `WH_PUB_PORT`, `WH_SUB_PORT`, `WH_CONTROL_PORT` from the environment.
    /// The bind address is always `127.0.0.1` and cannot be overridden.
    pub fn from_env() -> Self {
        Self {
            pub_port: Self::read_port_env("WH_PUB_PORT", DEFAULT_PUB_PORT),
            sub_port: Self::read_port_env("WH_SUB_PORT", DEFAULT_SUB_PORT),
            control_port: Self::read_port_env("WH_CONTROL_PORT", DEFAULT_CONTROL_PORT),
        }
    }

    /// Create a configuration with specific ports (for testing).
    pub fn with_ports(pub_port: u16, sub_port: u16, control_port: u16) -> Self {
        Self {
            pub_port,
            sub_port,
            control_port,
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
