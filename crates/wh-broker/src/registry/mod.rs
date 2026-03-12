//! Type registry for custom Protobuf type registration (ADR-004).
//!
//! The registry is broker-owned and persisted as a JSON file.
//! Core `wheelhouse.*` types are hard-coded and cannot be overridden.
//! Registration happens via the ZMQ control socket.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Reserved namespace prefix — cannot be registered via the API (ADR-004).
const RESERVED_NAMESPACE: &str = "wheelhouse";

/// Default per-namespace registration limit (RT-05).
const DEFAULT_PER_NAMESPACE_LIMIT: usize = 100;

/// Default total registry limit (RT-05).
const DEFAULT_TOTAL_LIMIT: usize = 10_000;

/// Registry error types following ADR-014 typed error hierarchy.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("RESERVED_NAMESPACE: namespace '{0}' is reserved and cannot be registered")]
    ReservedNamespace(String),

    #[error("INVALID_TYPE_NAME: type name '{0}' must be in format '<namespace>.<TypeName>'")]
    InvalidTypeName(String),

    #[error("REGISTRY_FULL: namespace '{0}' has reached the per-namespace limit of {1} types")]
    NamespaceFull(String, usize),

    #[error("REGISTRY_FULL: total registry limit of {0} types reached")]
    TotalLimitReached(usize),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Error code string for JSON responses (SCREAMING_SNAKE_CASE per SCV-01).
impl RegistryError {
    pub fn error_code(&self) -> &'static str {
        match self {
            RegistryError::ReservedNamespace(_) => "RESERVED_NAMESPACE",
            RegistryError::InvalidTypeName(_) => "INVALID_TYPE_NAME",
            RegistryError::NamespaceFull(_, _) => "REGISTRY_FULL",
            RegistryError::TotalLimitReached(_) => "REGISTRY_FULL",
            RegistryError::Io(_) => "IO_ERROR",
            RegistryError::Json(_) => "SERIALIZATION_ERROR",
        }
    }
}

/// A registered custom type entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeEntry {
    /// Fully qualified type name: `<namespace>.<TypeName>`
    pub type_name: String,
    /// Namespace extracted from type_name
    pub namespace: String,
    /// Short type name extracted from type_name
    pub short_name: String,
    /// Base64-encoded FileDescriptorProto bytes for self-description
    #[serde(default)]
    pub descriptor_bytes: Option<String>,
    /// Timestamp of registration (ISO 8601)
    pub registered_at: String,
}

/// Persistent registry state (serialized to JSON file).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistryState {
    /// Schema version for forward compatibility
    pub version: u32,
    /// All registered types keyed by fully-qualified name
    pub types: HashMap<String, TypeEntry>,
}

/// Configuration for registry limits.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub per_namespace_limit: usize,
    pub total_limit: usize,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            per_namespace_limit: DEFAULT_PER_NAMESPACE_LIMIT,
            total_limit: DEFAULT_TOTAL_LIMIT,
        }
    }
}

/// Broker-owned type registry (ADR-004).
///
/// Persists as JSON in the broker data directory. Loaded on start, written on registration.
#[derive(Debug)]
pub struct TypeRegistry {
    state: RegistryState,
    config: RegistryConfig,
    persistence_path: PathBuf,
}

impl TypeRegistry {
    /// Create a new empty registry that will persist to the given path.
    pub fn new(persistence_path: PathBuf, config: RegistryConfig) -> Self {
        Self {
            state: RegistryState {
                version: 1,
                types: HashMap::new(),
            },
            config,
            persistence_path,
        }
    }

    /// Load registry from JSON file. Returns empty registry if file doesn't exist.
    pub fn load(persistence_path: PathBuf, config: RegistryConfig) -> Result<Self, RegistryError> {
        if persistence_path.exists() {
            let contents = std::fs::read_to_string(&persistence_path)?;
            let state: RegistryState = serde_json::from_str(&contents)?;
            Ok(Self {
                state,
                config,
                persistence_path,
            })
        } else {
            Ok(Self::new(persistence_path, config))
        }
    }

    /// Validate and parse a fully-qualified type name into (namespace, short_name).
    pub fn parse_type_name(type_name: &str) -> Result<(&str, &str), RegistryError> {
        let dot_pos = type_name
            .find('.')
            .ok_or_else(|| RegistryError::InvalidTypeName(type_name.to_string()))?;

        let namespace = &type_name[..dot_pos];
        let short_name = &type_name[dot_pos + 1..];

        // Namespace must be non-empty
        if namespace.is_empty() {
            return Err(RegistryError::InvalidTypeName(type_name.to_string()));
        }

        // Short name must be non-empty
        if short_name.is_empty() {
            return Err(RegistryError::InvalidTypeName(type_name.to_string()));
        }

        // No nested dots allowed in short name (single level namespace)
        if short_name.contains('.') {
            return Err(RegistryError::InvalidTypeName(type_name.to_string()));
        }

        // Check reserved namespace (ADR-004 security invariant)
        if namespace == RESERVED_NAMESPACE {
            return Err(RegistryError::ReservedNamespace(namespace.to_string()));
        }

        Ok((namespace, short_name))
    }

    /// Register a custom type. Validates namespace, enforces limits, persists to disk.
    ///
    /// Idempotent: re-registering an existing type_name updates the entry without
    /// counting against limits. This is required for CM-07 (reconnect re-registration).
    pub fn register(
        &mut self,
        type_name: &str,
        descriptor_bytes: Option<String>,
    ) -> Result<&TypeEntry, RegistryError> {
        let (namespace, short_name) = Self::parse_type_name(type_name)?;

        let is_update = self.state.types.contains_key(type_name);

        if !is_update {
            // Check total limit (RT-05) — only for new registrations
            if self.state.types.len() >= self.config.total_limit {
                return Err(RegistryError::TotalLimitReached(self.config.total_limit));
            }

            // Check per-namespace limit (RT-05) — only for new registrations
            let namespace_count = self
                .state
                .types
                .values()
                .filter(|e| e.namespace == namespace)
                .count();
            if namespace_count >= self.config.per_namespace_limit {
                return Err(RegistryError::NamespaceFull(
                    namespace.to_string(),
                    self.config.per_namespace_limit,
                ));
            }
        }

        let entry = TypeEntry {
            type_name: type_name.to_string(),
            namespace: namespace.to_string(),
            short_name: short_name.to_string(),
            descriptor_bytes,
            registered_at: chrono_now_iso(),
        };

        self.state.types.insert(type_name.to_string(), entry);

        // Persist to disk (write-on-registration per ADR-004)
        self.persist()?;

        Ok(self.state.types.get(type_name).unwrap())
    }

    /// Check if a type is registered.
    pub fn contains(&self, type_name: &str) -> bool {
        self.state.types.contains_key(type_name)
    }

    /// Get a registered type entry.
    pub fn get(&self, type_name: &str) -> Option<&TypeEntry> {
        self.state.types.get(type_name)
    }

    /// List all registered types.
    pub fn list(&self) -> Vec<&TypeEntry> {
        self.state.types.values().collect()
    }

    /// List types by namespace.
    pub fn list_by_namespace(&self, namespace: &str) -> Vec<&TypeEntry> {
        self.state
            .types
            .values()
            .filter(|e| e.namespace == namespace)
            .collect()
    }

    /// Total number of registered types.
    pub fn len(&self) -> usize {
        self.state.types.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.state.types.is_empty()
    }

    /// Persist registry to JSON file.
    fn persist(&self) -> Result<(), RegistryError> {
        // Ensure parent directory exists
        if let Some(parent) = self.persistence_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.state)?;
        std::fs::write(&self.persistence_path, json)?;
        Ok(())
    }

    /// Get the persistence path.
    pub fn persistence_path(&self) -> &Path {
        &self.persistence_path
    }
}

/// ISO 8601 UTC timestamp without external dependency.
///
/// Format: "YYYY-MM-DDTHH:MM:SSZ" (approximate — uses epoch arithmetic).
/// For production, consider using the `time` or `chrono` crate.
fn chrono_now_iso() -> String {
    use std::time::SystemTime;
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Approximate UTC breakdown (no leap seconds — acceptable for registration timestamps)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since epoch to Y-M-D (simplified Gregorian)
    let mut y = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let year_days = if is_leap_year(y) { 366 } else { 365 };
        if remaining_days < year_days {
            break;
        }
        remaining_days -= year_days;
        y += 1;
    }
    let leap = is_leap_year(y);
    let month_days: [i64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut m = 0usize;
    while m < 12 && remaining_days >= month_days[m] {
        remaining_days -= month_days[m];
        m += 1;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        remaining_days + 1,
        hours,
        minutes,
        seconds
    )
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_registry() -> (TypeRegistry, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("type_registry.json");
        let registry = TypeRegistry::new(path, RegistryConfig::default());
        (registry, dir)
    }

    #[test]
    fn registers_custom_type_under_namespace() {
        let (mut registry, _dir) = test_registry();
        let entry = registry
            .register("biotech.MoleculeObject", None)
            .unwrap();
        assert_eq!(entry.namespace, "biotech");
        assert_eq!(entry.short_name, "MoleculeObject");
        assert_eq!(entry.type_name, "biotech.MoleculeObject");
        assert!(registry.contains("biotech.MoleculeObject"));
    }

    #[test]
    fn rejects_wheelhouse_namespace() {
        let (mut registry, _dir) = test_registry();
        let result = registry.register("wheelhouse.CustomType", None);
        assert!(result.is_err());
        match result.unwrap_err() {
            RegistryError::ReservedNamespace(ns) => assert_eq!(ns, "wheelhouse"),
            other => panic!("Expected ReservedNamespace, got: {other}"),
        }
    }

    #[test]
    fn rejects_invalid_type_name_no_dot() {
        let result = TypeRegistry::parse_type_name("InvalidName");
        assert!(result.is_err());
        match result.unwrap_err() {
            RegistryError::InvalidTypeName(name) => assert_eq!(name, "InvalidName"),
            other => panic!("Expected InvalidTypeName, got: {other}"),
        }
    }

    #[test]
    fn rejects_empty_namespace() {
        let result = TypeRegistry::parse_type_name(".TypeName");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_empty_short_name() {
        let result = TypeRegistry::parse_type_name("namespace.");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_nested_dots_in_short_name() {
        let result = TypeRegistry::parse_type_name("ns.Type.Sub");
        assert!(result.is_err());
    }

    #[test]
    fn two_namespaces_same_type_name_coexist() {
        let (mut registry, _dir) = test_registry();
        registry
            .register("biotech.MoleculeObject", None)
            .unwrap();
        registry
            .register("pharma.MoleculeObject", None)
            .unwrap();

        assert!(registry.contains("biotech.MoleculeObject"));
        assert!(registry.contains("pharma.MoleculeObject"));
        assert_eq!(registry.len(), 2);

        let biotech = registry.list_by_namespace("biotech");
        assert_eq!(biotech.len(), 1);
        assert_eq!(biotech[0].namespace, "biotech");

        let pharma = registry.list_by_namespace("pharma");
        assert_eq!(pharma.len(), 1);
        assert_eq!(pharma[0].namespace, "pharma");
    }

    #[test]
    fn enforces_per_namespace_limit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("type_registry.json");
        let config = RegistryConfig {
            per_namespace_limit: 3,
            total_limit: 10_000,
        };
        let mut registry = TypeRegistry::new(path, config);

        registry.register("test.Type1", None).unwrap();
        registry.register("test.Type2", None).unwrap();
        registry.register("test.Type3", None).unwrap();

        let result = registry.register("test.Type4", None);
        assert!(result.is_err());
        match result.unwrap_err() {
            RegistryError::NamespaceFull(ns, limit) => {
                assert_eq!(ns, "test");
                assert_eq!(limit, 3);
            }
            other => panic!("Expected NamespaceFull, got: {other}"),
        }
    }

    #[test]
    fn enforces_total_registry_limit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("type_registry.json");
        let config = RegistryConfig {
            per_namespace_limit: 100,
            total_limit: 3,
        };
        let mut registry = TypeRegistry::new(path, config);

        registry.register("ns1.Type1", None).unwrap();
        registry.register("ns2.Type2", None).unwrap();
        registry.register("ns3.Type3", None).unwrap();

        let result = registry.register("ns4.Type4", None);
        assert!(result.is_err());
        match result.unwrap_err() {
            RegistryError::TotalLimitReached(limit) => assert_eq!(limit, 3),
            other => panic!("Expected TotalLimitReached, got: {other}"),
        }
    }

    #[test]
    fn persists_and_loads_registry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("type_registry.json");

        // Register types
        {
            let mut registry = TypeRegistry::new(path.clone(), RegistryConfig::default());
            registry
                .register("biotech.MoleculeObject", Some("AQID".to_string()))
                .unwrap();
            registry.register("pharma.DrugCompound", None).unwrap();
        }

        // Verify file exists
        assert!(path.exists());
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("biotech.MoleculeObject"));

        // Load and verify
        let loaded = TypeRegistry::load(path, RegistryConfig::default()).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains("biotech.MoleculeObject"));
        assert!(loaded.contains("pharma.DrugCompound"));

        let entry = loaded.get("biotech.MoleculeObject").unwrap();
        assert_eq!(entry.descriptor_bytes.as_deref(), Some("AQID"));
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let registry = TypeRegistry::load(path, RegistryConfig::default()).unwrap();
        assert!(registry.is_empty());
    }

    #[test]
    fn error_codes_are_screaming_snake_case() {
        let err = RegistryError::ReservedNamespace("wheelhouse".to_string());
        assert_eq!(err.error_code(), "RESERVED_NAMESPACE");

        let err = RegistryError::InvalidTypeName("bad".to_string());
        assert_eq!(err.error_code(), "INVALID_TYPE_NAME");

        let err = RegistryError::NamespaceFull("test".to_string(), 100);
        assert_eq!(err.error_code(), "REGISTRY_FULL");

        let err = RegistryError::TotalLimitReached(10_000);
        assert_eq!(err.error_code(), "REGISTRY_FULL");
    }

    #[test]
    fn register_with_descriptor_bytes() {
        let (mut registry, _dir) = test_registry();
        let entry = registry
            .register("myns.MyType", Some("base64data".to_string()))
            .unwrap();
        assert_eq!(entry.descriptor_bytes.as_deref(), Some("base64data"));
    }

    #[test]
    fn list_returns_all_types() {
        let (mut registry, _dir) = test_registry();
        registry.register("ns1.Type1", None).unwrap();
        registry.register("ns2.Type2", None).unwrap();
        let all = registry.list();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn re_registration_is_idempotent() {
        let (mut registry, _dir) = test_registry();
        registry.register("ns.MyType", None).unwrap();
        assert_eq!(registry.len(), 1);

        // Re-register same type — should succeed and not increase count
        registry
            .register("ns.MyType", Some("updated_descriptor".to_string()))
            .unwrap();
        assert_eq!(registry.len(), 1);

        // Verify descriptor was updated
        let entry = registry.get("ns.MyType").unwrap();
        assert_eq!(
            entry.descriptor_bytes.as_deref(),
            Some("updated_descriptor")
        );
    }

    #[test]
    fn re_registration_does_not_count_against_limit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("type_registry.json");
        let config = RegistryConfig {
            per_namespace_limit: 2,
            total_limit: 10_000,
        };
        let mut registry = TypeRegistry::new(path, config);

        registry.register("test.Type1", None).unwrap();
        registry.register("test.Type2", None).unwrap();

        // At limit — new type should fail
        let result = registry.register("test.Type3", None);
        assert!(result.is_err());

        // Re-register existing — should succeed (idempotent for CM-07)
        registry.register("test.Type1", None).unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn timestamp_is_iso_format() {
        let ts = chrono_now_iso();
        // Should match YYYY-MM-DDTHH:MM:SSZ pattern
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
        assert_eq!(ts.len(), 20); // "2026-03-12T10:30:00Z"
    }
}
