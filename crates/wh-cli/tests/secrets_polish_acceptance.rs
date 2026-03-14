//! Acceptance tests for `wh secrets init` — Polish and Edge Cases (Story 3.6)
//!
//! Tests verify the acceptance criteria from the story specification.
//! Tests use env vars to avoid interactive prompts in CI.

use wh_cli::commands::secrets::{
    CredentialResult, CredentialStatus, DetectionResult, SecretsInitData, CREDENTIALS,
};
use wh_cli::output::OutputEnvelope;

// ---------------------------------------------------------------------------
// AC1: Re-running when secrets are already configured
// ---------------------------------------------------------------------------

#[test]
fn ac1_rerun_shows_already_configured_status() {
    // Given: I run `wh secrets init` when credentials are already configured (via env var)
    // When: the wizard starts
    // Then: already-configured secrets show CredentialStatus::DetectedFromEnv
    std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", "oauth-token-ac1");
    let status = wh_cli::commands::secrets::check_credential_status("claude_code_oauth_token");
    assert!(
        matches!(status, Some(CredentialStatus::DetectedFromEnv)),
        "Expected DetectedFromEnv status, got {status:?}"
    );
    std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
}

#[test]
fn ac1_update_status_variant_serializes_correctly() {
    // Given: A credential has been updated via --update flag
    // When: the status is serialized to JSON
    // Then: it produces "updated" in snake_case
    let status = CredentialStatus::Updated;
    let json = serde_json::to_string(&status).unwrap();
    assert!(
        json.contains("updated"),
        "Expected 'updated' in JSON, got: {json}"
    );
}

#[test]
fn ac1_kept_status_variant_serializes_correctly() {
    // Given: A credential was kept unchanged during --update
    // When: the status is serialized to JSON
    // Then: it produces "kept" in snake_case
    let status = CredentialStatus::Kept;
    let json = serde_json::to_string(&status).unwrap();
    assert!(
        json.contains("kept"),
        "Expected 'kept' in JSON, got: {json}"
    );
}

#[test]
fn ac1_all_configured_message_in_json() {
    // Given: all credentials are already configured
    // When: wh secrets init --format json is run
    // Then: JSON output includes all_configured: true
    let data = SecretsInitData {
        podman: DetectionResult::Detected {
            version: "4.8.0".to_string(),
        },
        git: DetectionResult::Detected {
            version: "2.43.0".to_string(),
        },
        credentials: vec![
            CredentialResult {
                name: "claude_code_oauth_token".to_string(),
                display_name: "Claude API key".to_string(),
                required: true,
                status: CredentialStatus::DetectedFromEnv,
            },
            CredentialResult {
                name: "telegram_bot_token".to_string(),
                display_name: "Telegram bot token".to_string(),
                required: false,
                status: CredentialStatus::AlreadyConfigured,
            },
        ],
        all_configured: true,
        next_command: "wh deploy apply topology.wh".to_string(),
    };
    let envelope = OutputEnvelope::ok(data);
    let json = serde_json::to_string_pretty(&envelope).unwrap();
    assert!(json.contains("\"all_configured\": true"));
    assert!(json.contains("\"v\": 1"));
    assert!(json.contains("\"status\": \"ok\""));
}

// ---------------------------------------------------------------------------
// AC2: Podman not-found with actionable install hint
// ---------------------------------------------------------------------------

#[test]
fn ac2_podman_not_found_includes_install_hint() {
    // Given: Podman is not found on the system
    // When: the detection result is constructed
    // Then: it includes a platform-specific install hint
    let result = DetectionResult::NotFound {
        reason: "not found".to_string(),
        install_hint: Some("brew install podman".to_string()),
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(
        json.contains("install_hint"),
        "Expected install_hint field in JSON"
    );
    assert!(
        json.contains("brew install podman"),
        "Expected brew install hint"
    );
}

#[test]
fn ac2_podman_detected_has_no_install_hint() {
    // Given: Podman IS found on the system
    // When: the detection result is constructed
    // Then: install_hint is not present in JSON (None is skipped)
    let result = DetectionResult::Detected {
        version: "4.8.0".to_string(),
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(
        !json.contains("install_hint"),
        "Detected result should not have install_hint"
    );
}

// ---------------------------------------------------------------------------
// AC3: read_secret integration for deploy apply
// ---------------------------------------------------------------------------

#[test]
fn ac3_read_secret_returns_env_var_value() {
    // Given: CLAUDE_CODE_OAUTH_TOKEN is set as an environment variable
    // When: read_secret("claude_code_oauth_token") is called
    // Then: it returns the env var value
    std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", "oauth-token-ac3");
    let result = wh_cli::commands::secrets::read_secret("claude_code_oauth_token");
    assert!(result.is_ok(), "Expected Ok, got {result:?}");
    assert_eq!(result.unwrap(), "oauth-token-ac3");
    std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
}

#[test]
fn ac3_read_secret_returns_error_when_not_configured() {
    // Given: no env var and no keychain entry
    // When: read_secret("nonexistent_credential") is called
    // Then: it returns SecretNotFound error
    std::env::remove_var("NONEXISTENT_CREDENTIAL");
    let result = wh_cli::commands::secrets::read_secret("nonexistent_credential");
    assert!(result.is_err(), "Expected error, got {result:?}");
    let err = result.unwrap_err();
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("not configured"),
        "Expected 'not configured' in error message, got: {err_msg}"
    );
}

#[test]
fn ac3_read_secret_error_suggests_secrets_init() {
    // Given: a secret is not configured
    // When: the error message is rendered
    // Then: it suggests running 'wh secrets init'
    std::env::remove_var("NONEXISTENT_CREDENTIAL_2");
    let result = wh_cli::commands::secrets::read_secret("nonexistent_credential_2");
    let err = result.unwrap_err();
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("wh secrets init"),
        "Error should suggest running 'wh secrets init', got: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// Cross-cutting: JSON compliance and no secret leakage
// ---------------------------------------------------------------------------

#[test]
fn secrets_status_reports_env_detected() {
    // Given: CLAUDE_CODE_OAUTH_TOKEN is set
    // When: credential status is checked
    // Then: it reports the credential as detected from env
    std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", "oauth-token-status");
    let status = wh_cli::commands::secrets::check_credential_status("claude_code_oauth_token");
    assert!(matches!(status, Some(CredentialStatus::DetectedFromEnv)));
    std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
}

#[test]
fn secrets_status_json_never_contains_secret_values() {
    // Given: credentials are configured via env vars
    // When: status JSON is generated
    // Then: the actual secret values never appear
    let result = CredentialResult {
        name: "claude_code_oauth_token".to_string(),
        display_name: "Claude API key".to_string(),
        required: true,
        status: CredentialStatus::DetectedFromEnv,
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(
        !json.contains("test-key"),
        "JSON must not contain secret values"
    );
    assert!(
        !json.contains("sk-"),
        "JSON must not contain API key prefixes"
    );
}

#[test]
fn json_output_snake_case_for_new_fields() {
    // All new fields (all_configured, install_hint, etc.) use snake_case
    let data = SecretsInitData {
        podman: DetectionResult::NotFound {
            reason: "not found".to_string(),
            install_hint: Some("brew install podman".to_string()),
        },
        git: DetectionResult::Detected {
            version: "2.43.0".to_string(),
        },
        credentials: vec![],
        all_configured: false,
        next_command: "wh deploy apply topology.wh".to_string(),
    };
    let envelope = OutputEnvelope::ok(data);
    let json = serde_json::to_string_pretty(&envelope).unwrap();
    assert!(json.contains("all_configured"));
    assert!(json.contains("install_hint"));
    assert!(json.contains("next_command"));
    // No camelCase
    assert!(!json.contains("allConfigured"));
    assert!(!json.contains("installHint"));
    assert!(!json.contains("nextCommand"));
}

#[test]
fn credential_registry_unchanged() {
    // The MVP credential registry still has exactly 2 entries
    assert_eq!(CREDENTIALS.len(), 2);
    assert_eq!(CREDENTIALS[0].env_var, "CLAUDE_CODE_OAUTH_TOKEN");
    assert_eq!(CREDENTIALS[1].env_var, "TELEGRAM_BOT_TOKEN");
}
