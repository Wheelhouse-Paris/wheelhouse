//! Control socket handler for type registration commands (ADR-010).
//!
//! JSON-over-ZMQ REQ/REP protocol. All responses include `"v": 1`.
//! Commands: `register_type`, `list_types`.

use crate::registry::TypeRegistry;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version included in all responses (ADR-010).
const PROTOCOL_VERSION: u32 = 1;

/// Control socket request envelope.
#[derive(Debug, Deserialize)]
pub struct ControlRequest {
    pub v: u32,
    pub command: String,
    #[serde(default)]
    pub data: Value,
}

/// Control socket response envelope. All responses include `"v": 1` (ADR-010).
#[derive(Debug, Serialize)]
pub struct ControlResponse {
    pub v: u32,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ControlResponse {
    /// Create a success response with data.
    pub fn ok(data: Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            status: "ok".to_string(),
            data: Some(data),
            code: None,
            message: None,
        }
    }

    /// Create an error response with code and message (ADR-014 JSON contract).
    pub fn error(code: &str, message: &str) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            status: "error".to_string(),
            data: None,
            code: Some(code.to_string()),
            message: Some(message.to_string()),
        }
    }
}

/// Register type request data.
#[derive(Debug, Deserialize)]
pub struct RegisterTypeData {
    pub type_name: String,
    #[serde(default)]
    pub descriptor_bytes: Option<String>,
}

/// Handle a control socket request by dispatching to the appropriate handler.
pub fn handle_request(registry: &mut TypeRegistry, request_json: &str) -> String {
    let request: ControlRequest = match serde_json::from_str(request_json) {
        Ok(req) => req,
        Err(e) => {
            let resp = ControlResponse::error("INVALID_REQUEST", &format!("Invalid JSON: {e}"));
            return serde_json::to_string(&resp).unwrap_or_default();
        }
    };

    let response = match request.command.as_str() {
        "register_type" => handle_register_type(registry, request.data),
        "list_types" => handle_list_types(registry),
        _ => ControlResponse::error(
            "UNKNOWN_COMMAND",
            &format!("Unknown command: {}", request.command),
        ),
    };

    serde_json::to_string(&response).unwrap_or_default()
}

/// Handle `register_type` command.
fn handle_register_type(registry: &mut TypeRegistry, data: Value) -> ControlResponse {
    let request_data: RegisterTypeData = match serde_json::from_value(data) {
        Ok(d) => d,
        Err(e) => {
            return ControlResponse::error(
                "INVALID_REQUEST",
                &format!("Invalid register_type data: {e}"),
            );
        }
    };

    match registry.register(&request_data.type_name, request_data.descriptor_bytes) {
        Ok(entry) => ControlResponse::ok(serde_json::json!({
            "type_name": entry.type_name,
            "namespace": entry.namespace,
            "registered_at": entry.registered_at,
        })),
        Err(e) => ControlResponse::error(e.error_code(), &e.to_string()),
    }
}

/// Handle `list_types` command.
fn handle_list_types(registry: &TypeRegistry) -> ControlResponse {
    let types: Vec<Value> = registry
        .list()
        .iter()
        .map(|entry| {
            serde_json::json!({
                "type_name": entry.type_name,
                "namespace": entry.namespace,
                "short_name": entry.short_name,
                "registered_at": entry.registered_at,
            })
        })
        .collect();

    ControlResponse::ok(serde_json::json!({
        "types": types,
        "total_count": types.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryConfig;
    use tempfile::TempDir;

    fn test_registry() -> (TypeRegistry, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("type_registry.json");
        let registry = TypeRegistry::new(path, RegistryConfig::default());
        (registry, dir)
    }

    #[test]
    fn register_type_via_control_socket() {
        let (mut registry, _dir) = test_registry();
        let request = r#"{"v": 1, "command": "register_type", "data": {"type_name": "biotech.MoleculeObject"}}"#;
        let response_str = handle_request(&mut registry, request);
        let response: Value = serde_json::from_str(&response_str).unwrap();

        assert_eq!(response["v"], 1);
        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["type_name"],
            "biotech.MoleculeObject"
        );
        assert_eq!(response["data"]["namespace"], "biotech");
        assert!(registry.contains("biotech.MoleculeObject"));
    }

    #[test]
    fn register_type_with_descriptor_bytes() {
        let (mut registry, _dir) = test_registry();
        let request = r#"{"v": 1, "command": "register_type", "data": {"type_name": "myns.MyType", "descriptor_bytes": "AQID"}}"#;
        let response_str = handle_request(&mut registry, request);
        let response: Value = serde_json::from_str(&response_str).unwrap();

        assert_eq!(response["status"], "ok");
        let entry = registry.get("myns.MyType").unwrap();
        assert_eq!(entry.descriptor_bytes.as_deref(), Some("AQID"));
    }

    #[test]
    fn register_type_rejects_reserved_namespace() {
        let (mut registry, _dir) = test_registry();
        let request = r#"{"v": 1, "command": "register_type", "data": {"type_name": "wheelhouse.Custom"}}"#;
        let response_str = handle_request(&mut registry, request);
        let response: Value = serde_json::from_str(&response_str).unwrap();

        assert_eq!(response["v"], 1);
        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "RESERVED_NAMESPACE");
    }

    #[test]
    fn register_type_rejects_invalid_name() {
        let (mut registry, _dir) = test_registry();
        let request =
            r#"{"v": 1, "command": "register_type", "data": {"type_name": "NoNamespace"}}"#;
        let response_str = handle_request(&mut registry, request);
        let response: Value = serde_json::from_str(&response_str).unwrap();

        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "INVALID_TYPE_NAME");
    }

    #[test]
    fn list_types_via_control_socket() {
        let (mut registry, _dir) = test_registry();
        registry.register("biotech.Molecule", None).unwrap();
        registry.register("pharma.Drug", None).unwrap();

        let request = r#"{"v": 1, "command": "list_types"}"#;
        let response_str = handle_request(&mut registry, request);
        let response: Value = serde_json::from_str(&response_str).unwrap();

        assert_eq!(response["v"], 1);
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["total_count"], 2);

        let types = response["data"]["types"].as_array().unwrap();
        assert_eq!(types.len(), 2);
    }

    #[test]
    fn list_types_empty_registry() {
        let (mut registry, _dir) = test_registry();
        let request = r#"{"v": 1, "command": "list_types"}"#;
        let response_str = handle_request(&mut registry, request);
        let response: Value = serde_json::from_str(&response_str).unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["total_count"], 0);
    }

    #[test]
    fn unknown_command_returns_error() {
        let (mut registry, _dir) = test_registry();
        let request = r#"{"v": 1, "command": "nonexistent"}"#;
        let response_str = handle_request(&mut registry, request);
        let response: Value = serde_json::from_str(&response_str).unwrap();

        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "UNKNOWN_COMMAND");
    }

    #[test]
    fn invalid_json_returns_error() {
        let (mut registry, _dir) = test_registry();
        let response_str = handle_request(&mut registry, "not valid json{");
        let response: Value = serde_json::from_str(&response_str).unwrap();

        assert_eq!(response["v"], 1);
        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "INVALID_REQUEST");
    }

    #[test]
    fn all_responses_include_v1() {
        let (mut registry, _dir) = test_registry();

        // Success response
        let request = r#"{"v": 1, "command": "register_type", "data": {"type_name": "ns.Type"}}"#;
        let response: Value =
            serde_json::from_str(&handle_request(&mut registry, request)).unwrap();
        assert_eq!(response["v"], 1);

        // Error response
        let request = r#"{"v": 1, "command": "register_type", "data": {"type_name": "bad"}}"#;
        let response: Value =
            serde_json::from_str(&handle_request(&mut registry, request)).unwrap();
        assert_eq!(response["v"], 1);

        // List response
        let request = r#"{"v": 1, "command": "list_types"}"#;
        let response: Value =
            serde_json::from_str(&handle_request(&mut registry, request)).unwrap();
        assert_eq!(response["v"], 1);
    }

    #[test]
    fn all_json_fields_are_snake_case() {
        let (mut registry, _dir) = test_registry();

        // Register
        let request = r#"{"v": 1, "command": "register_type", "data": {"type_name": "ns.Type"}}"#;
        let response_str = handle_request(&mut registry, request);
        // Verify no camelCase in response
        assert!(!response_str.contains("typeName"));
        assert!(!response_str.contains("registeredAt"));
        assert!(response_str.contains("type_name"));
        assert!(response_str.contains("registered_at"));

        // List
        let request = r#"{"v": 1, "command": "list_types"}"#;
        let response_str = handle_request(&mut registry, request);
        assert!(!response_str.contains("totalCount"));
        assert!(!response_str.contains("shortName"));
        assert!(response_str.contains("total_count"));
        assert!(response_str.contains("short_name"));
    }
}
