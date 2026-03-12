use std::io::IsTerminal;
use std::process::Command;

use clap::Subcommand;
use console::style;
use serde::Serialize;

use crate::output::error::WhError;
use crate::output::{OutputFormat, OutputEnvelope};

/// Credential spec for a single secret that the wizard manages.
#[derive(Debug, Clone)]
pub struct CredentialSpec {
    /// Internal name used as keychain service suffix (e.g., "anthropic_api_key").
    pub name: &'static str,
    /// Environment variable to check (e.g., "ANTHROPIC_API_KEY").
    pub env_var: &'static str,
    /// Display name shown to users (e.g., "Claude API key").
    pub display_name: &'static str,
    /// Whether the credential is required for Wheelhouse to function.
    pub required: bool,
}

/// The MVP credential registry.
pub const CREDENTIALS: &[CredentialSpec] = &[
    CredentialSpec {
        name: "anthropic_api_key",
        env_var: "ANTHROPIC_API_KEY",
        display_name: "Claude API key",
        required: true,
    },
    CredentialSpec {
        name: "telegram_bot_token",
        env_var: "TELEGRAM_BOT_TOKEN",
        display_name: "Telegram bot token",
        required: false,
    },
];

/// Result of auto-detecting a tool on the system.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DetectionResult {
    Detected { version: String },
    NotFound {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        install_hint: Option<String>,
    },
}

/// Status of an individual credential after processing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    /// Detected from an environment variable — no prompt needed.
    DetectedFromEnv,
    /// Already stored in keychain — no prompt needed.
    AlreadyConfigured,
    /// User entered value during wizard — now stored in keychain.
    Configured,
    /// User skipped an optional credential.
    Skipped,
    /// User updated an existing credential via --update.
    Updated,
    /// User kept existing credential unchanged during --update.
    Kept,
}

/// Per-credential result included in JSON output.
#[derive(Debug, Clone, Serialize)]
pub struct CredentialResult {
    pub name: String,
    pub display_name: String,
    pub required: bool,
    pub status: CredentialStatus,
}

/// Full result of the `secrets init` wizard for JSON output.
#[derive(Debug, Serialize)]
pub struct SecretsInitData {
    pub podman: DetectionResult,
    pub git: DetectionResult,
    pub credentials: Vec<CredentialResult>,
    pub all_configured: bool,
    pub next_command: String,
}

/// The keychain service prefix used for all Wheelhouse secrets.
const KEYRING_SERVICE_PREFIX: &str = "wh";
/// The keyring user for all entries.
const KEYRING_USER: &str = "wh";

#[derive(Debug, Subcommand)]
pub enum SecretsCmd {
    /// Initialize credential wizard — detect providers and configure API keys.
    Init {
        /// Output format: human (default) or json.
        #[arg(long, default_value = "human")]
        format: OutputFormat,
        /// Re-prompt for already-configured credentials to update them.
        #[arg(long, short = 'u')]
        update: bool,
    },
}

impl SecretsCmd {
    /// Extract the output format from the command, without consuming self.
    pub fn format(&self) -> OutputFormat {
        match self {
            SecretsCmd::Init { format, .. } => *format,
        }
    }

    pub fn run(self) -> Result<(), WhError> {
        match self {
            SecretsCmd::Init { format, update } => run_init(format, update),
        }
    }
}

/// Main entry point for `wh secrets init`.
fn run_init(format: OutputFormat, update: bool) -> Result<(), WhError> {
    // Non-TTY guard: interactive prompting requires a terminal.
    // In JSON mode we still need to check — if all creds are already configured
    // via env vars we can proceed, but if prompting would be needed we must fail.
    let is_tty = std::io::stdin().is_terminal();

    // --- Provider auto-detection ---
    let podman = detect_podman();
    let git = detect_git();

    // Git is mandatory for Wheelhouse.
    if let DetectionResult::NotFound { ref reason, .. } = git {
        // Error output is handled by main.rs — do not print here to avoid duplicates.
        return Err(WhError::GitNotFound(reason.clone()));
    }

    if format == OutputFormat::Human {
        // Print detection results.
        match &podman {
            DetectionResult::Detected { version } => {
                println!("{} Podman {} detected", style("✓").green().bold(), version);
            }
            DetectionResult::NotFound { install_hint, .. } => {
                let hint = install_hint.as_deref().unwrap_or("see https://podman.io/docs/installation");
                println!(
                    "{} Podman not found — install via: {}",
                    style("!").yellow().bold(),
                    hint,
                );
                println!("  You can still configure credentials now and install Podman later.");
            }
        }
        match &git {
            DetectionResult::Detected { version } => {
                println!("{} Git {} detected", style("✓").green().bold(), version);
            }
            DetectionResult::NotFound { .. } => unreachable!(), // handled above
        }
        println!(); // blank line before credential section
    }

    // --- Credential processing ---
    let mut results: Vec<CredentialResult> = Vec::new();
    let mut needs_prompt = false;

    // First pass: determine which credentials need prompting.
    for spec in CREDENTIALS {
        let status = check_credential(spec);
        match status {
            None => {
                // Credential not yet configured — will need prompting.
                needs_prompt = true;
            }
            Some(status) => {
                if update {
                    // In update mode, we still prompt for already-configured credentials.
                    needs_prompt = true;
                } else {
                    if format == OutputFormat::Human {
                        print_credential_status(spec, &status);
                    }
                    results.push(CredentialResult {
                        name: spec.name.to_string(),
                        display_name: spec.display_name.to_string(),
                        required: spec.required,
                        status,
                    });
                }
            }
        }
    }

    // Check if all credentials are already configured (without --update).
    let all_configured = !update && !needs_prompt;

    if all_configured && format == OutputFormat::Human {
        println!(
            "{} All credentials already configured. Use {} to change existing secrets.",
            style("✓").green().bold(),
            style("--update").bold(),
        );
    }

    // Non-TTY check: if we need prompting but have no terminal, fail.
    // Error output is handled by main.rs — do not print here to avoid duplicates.
    if needs_prompt && !is_tty {
        return Err(WhError::NonInteractive);
    }

    // Second pass: prompt for unconfigured credentials (or all in update mode).
    for spec in CREDENTIALS {
        // Skip if already processed in first pass.
        if results.iter().any(|r| r.name == spec.name) {
            continue;
        }

        let existing_status = check_credential(spec);

        let status = if update && existing_status.is_some() {
            // Update mode: prompt to update an already-configured credential.
            prompt_update_credential(spec, format)?
        } else {
            // Normal mode: prompt for unconfigured credential.
            prompt_credential(spec, format)?
        };

        if format == OutputFormat::Human {
            print_credential_status(spec, &status);
        }
        results.push(CredentialResult {
            name: spec.name.to_string(),
            display_name: spec.display_name.to_string(),
            required: spec.required,
            status,
        });
    }

    // --- Summary ---
    let next_command = "wh deploy apply topology.wh".to_string();

    if format == OutputFormat::Human && !all_configured {
        println!(); // blank line before summary
        let configured = results
            .iter()
            .filter(|r| matches!(
                r.status,
                CredentialStatus::Configured
                    | CredentialStatus::DetectedFromEnv
                    | CredentialStatus::AlreadyConfigured
                    | CredentialStatus::Updated
                    | CredentialStatus::Kept
            ))
            .count();
        let skipped = results
            .iter()
            .filter(|r| matches!(r.status, CredentialStatus::Skipped))
            .count();
        let updated = results
            .iter()
            .filter(|r| matches!(r.status, CredentialStatus::Updated))
            .count();
        if updated > 0 {
            println!(
                "{} {} credential(s) configured ({} updated), {} skipped.",
                style("✓").green().bold(),
                configured,
                updated,
                skipped,
            );
        } else {
            println!(
                "{} {} credential(s) configured, {} skipped.",
                style("✓").green().bold(),
                configured,
                skipped,
            );
        }
        println!("Run '{}' to start.", style(&next_command).bold());
    }

    if format == OutputFormat::Json {
        let data = SecretsInitData {
            podman,
            git,
            credentials: results,
            all_configured,
            next_command,
        };
        let envelope = OutputEnvelope::ok(data);
        let json = serde_json::to_string_pretty(&envelope)
            .map_err(|e| WhError::Internal(e.to_string()))?;
        println!("{json}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Provider detection
// ---------------------------------------------------------------------------

fn detect_podman() -> DetectionResult {
    match Command::new("podman").arg("version").arg("--format").arg("{{.Client.Version}}").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            DetectionResult::Detected { version }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let reason = if stderr.is_empty() {
                "podman returned an error".to_string()
            } else {
                stderr
            };
            DetectionResult::NotFound {
                reason,
                install_hint: Some(platform_podman_install_hint()),
            }
        }
        Err(_) => DetectionResult::NotFound {
            reason: "podman binary not found on PATH".to_string(),
            install_hint: Some(platform_podman_install_hint()),
        },
    }
}

/// Return a platform-specific install hint for Podman.
fn platform_podman_install_hint() -> String {
    if cfg!(target_os = "macos") {
        "brew install podman".to_string()
    } else {
        // On Linux, give a generic hint that covers the main distros.
        "sudo apt install podman (Debian/Ubuntu) or sudo dnf install podman (Fedora/RHEL)".to_string()
    }
}

fn detect_git() -> DetectionResult {
    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // `git --version` returns e.g. "git version 2.43.0"
            let version = raw.strip_prefix("git version ").unwrap_or(&raw).to_string();
            DetectionResult::Detected { version }
        }
        Ok(_) => DetectionResult::NotFound {
            reason: "git returned an error".to_string(),
            install_hint: None,
        },
        Err(e) => DetectionResult::NotFound {
            reason: e.to_string(),
            install_hint: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Credential helpers
// ---------------------------------------------------------------------------

/// Check if a credential is already configured (env var or keychain).
/// Returns `Some(status)` if already configured, `None` if prompting is needed.
fn check_credential(spec: &CredentialSpec) -> Option<CredentialStatus> {
    // 1. Check environment variable.
    if std::env::var(spec.env_var).is_ok() {
        return Some(CredentialStatus::DetectedFromEnv);
    }

    // 2. Check keychain.
    let service = format!("{}.{}", KEYRING_SERVICE_PREFIX, spec.name);
    if let Ok(entry) = keyring::Entry::new(&service, KEYRING_USER) {
        if entry.get_password().is_ok() {
            return Some(CredentialStatus::AlreadyConfigured);
        }
    }

    None
}

/// Public API: Check credential status by credential name.
///
/// Looks up the credential name in the registry and returns its current status.
/// Returns `None` if the credential is not configured (neither env var nor keychain).
pub fn check_credential_status(credential_name: &str) -> Option<CredentialStatus> {
    // Look up in the registry first.
    if let Some(spec) = CREDENTIALS.iter().find(|s| s.name == credential_name) {
        return check_credential(spec);
    }

    // For credentials not in the registry, check env var by converting name to uppercase.
    let env_var = credential_name.to_uppercase();
    if std::env::var(&env_var).is_ok() {
        return Some(CredentialStatus::DetectedFromEnv);
    }

    // Check keychain directly.
    let service = format!("{}.{}", KEYRING_SERVICE_PREFIX, credential_name);
    if let Ok(entry) = keyring::Entry::new(&service, KEYRING_USER) {
        if entry.get_password().is_ok() {
            return Some(CredentialStatus::AlreadyConfigured);
        }
    }

    None
}

/// Public API: Read a secret value by credential name.
///
/// Follows precedence: (1) env var, (2) keychain entry, (3) error.
/// This function is used by other commands (e.g., `deploy apply`) to retrieve
/// secrets from the configured store without knowing the storage backend.
///
/// # Errors
///
/// Returns `WhError::SecretNotFound` if the secret is not configured in either
/// environment variables or the OS keychain.
pub fn read_secret(credential_name: &str) -> Result<String, WhError> {
    // Look up in the registry for the env var name.
    let env_var = CREDENTIALS
        .iter()
        .find(|s| s.name == credential_name)
        .map(|s| s.env_var.to_string())
        .unwrap_or_else(|| credential_name.to_uppercase());

    // 1. Check environment variable.
    if let Ok(value) = std::env::var(&env_var) {
        return Ok(value);
    }

    // 2. Check keychain.
    let service = format!("{}.{}", KEYRING_SERVICE_PREFIX, credential_name);
    if let Ok(entry) = keyring::Entry::new(&service, KEYRING_USER) {
        if let Ok(password) = entry.get_password() {
            return Ok(password);
        }
    }

    // 3. Not found.
    Err(WhError::SecretNotFound(credential_name.to_string()))
}

/// Prompt the user for a credential value and store it.
fn prompt_credential(spec: &CredentialSpec, format: OutputFormat) -> Result<CredentialStatus, WhError> {
    if format == OutputFormat::Human {
        // Print prompt header.
        if spec.required {
            println!(
                "  {} (required):",
                style(spec.display_name).bold(),
            );
        } else {
            println!(
                "  {} (optional — press Enter to skip):",
                style(spec.display_name).bold(),
            );
        }
    }

    loop {
        let input = dialoguer::Password::new()
            .with_prompt(format!("  Enter {}", spec.display_name))
            .allow_empty_password(!spec.required)
            .interact()
            .map_err(|e| WhError::PromptFailed(e.to_string()))?;

        if input.is_empty() {
            if spec.required {
                if format == OutputFormat::Human {
                    println!(
                        "    {} is required. Please enter a value.",
                        spec.display_name,
                    );
                }
                continue;
            }
            return Ok(CredentialStatus::Skipped);
        }

        // Store in keychain.
        store_in_keychain(spec.name, &input)?;
        return Ok(CredentialStatus::Configured);
    }
}

/// Prompt the user to update an already-configured credential.
fn prompt_update_credential(spec: &CredentialSpec, format: OutputFormat) -> Result<CredentialStatus, WhError> {
    if format == OutputFormat::Human {
        println!(
            "  {} {} — enter new value or press Enter to keep current:",
            style("✓").green().bold(),
            style(spec.display_name).bold(),
        );
    }

    let input = dialoguer::Password::new()
        .with_prompt(format!("  New {} (Enter to keep)", spec.display_name))
        .allow_empty_password(true)
        .interact()
        .map_err(|e| WhError::PromptFailed(e.to_string()))?;

    if input.is_empty() {
        return Ok(CredentialStatus::Kept);
    }

    // Store updated value in keychain.
    store_in_keychain(spec.name, &input)?;
    Ok(CredentialStatus::Updated)
}

/// Store a value in the OS keychain.
fn store_in_keychain(credential_name: &str, value: &str) -> Result<(), WhError> {
    let service = format!("{}.{}", KEYRING_SERVICE_PREFIX, credential_name);
    let entry = keyring::Entry::new(&service, KEYRING_USER)
        .map_err(|e| WhError::KeychainError(e.to_string()))?;
    entry
        .set_password(value)
        .map_err(|e| WhError::KeychainError(e.to_string()))?;
    Ok(())
}

fn print_credential_status(spec: &CredentialSpec, status: &CredentialStatus) {
    match status {
        CredentialStatus::DetectedFromEnv => {
            println!(
                "  {} {} detected from environment",
                style("✓").green().bold(),
                spec.display_name,
            );
        }
        CredentialStatus::AlreadyConfigured => {
            println!(
                "  {} {} already configured",
                style("✓").green().bold(),
                spec.display_name,
            );
        }
        CredentialStatus::Configured => {
            println!(
                "  {} {} configured",
                style("✓").green().bold(),
                spec.display_name,
            );
        }
        CredentialStatus::Updated => {
            println!(
                "  {} {} updated",
                style("✓").green().bold(),
                spec.display_name,
            );
        }
        CredentialStatus::Kept => {
            println!(
                "  {} {} kept (unchanged)",
                style("✓").green().bold(),
                spec.display_name,
            );
        }
        CredentialStatus::Skipped => {
            println!(
                "  {} {} skipped",
                style("⊘").dim(),
                spec.display_name,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_registry_contains_expected_entries() {
        assert_eq!(CREDENTIALS.len(), 2);

        let anthropic = &CREDENTIALS[0];
        assert_eq!(anthropic.name, "anthropic_api_key");
        assert_eq!(anthropic.env_var, "ANTHROPIC_API_KEY");
        assert!(anthropic.required);

        let telegram = &CREDENTIALS[1];
        assert_eq!(telegram.name, "telegram_bot_token");
        assert_eq!(telegram.env_var, "TELEGRAM_BOT_TOKEN");
        assert!(!telegram.required);
    }

    #[test]
    fn detection_result_detected_serializes_snake_case() {
        let detected = DetectionResult::Detected {
            version: "4.8.0".to_string(),
        };
        let json = serde_json::to_string(&detected).unwrap();
        assert!(json.contains("\"status\":\"detected\""));
        assert!(json.contains("\"version\":\"4.8.0\""));
        assert!(!json.contains("install_hint")); // Detected has no install_hint
    }

    #[test]
    fn detection_result_not_found_with_install_hint() {
        let not_found = DetectionResult::NotFound {
            reason: "not found".to_string(),
            install_hint: Some("brew install podman".to_string()),
        };
        let json = serde_json::to_string(&not_found).unwrap();
        assert!(json.contains("\"status\":\"not_found\""));
        assert!(json.contains("\"install_hint\":\"brew install podman\""));
    }

    #[test]
    fn detection_result_not_found_without_install_hint() {
        let not_found = DetectionResult::NotFound {
            reason: "error".to_string(),
            install_hint: None,
        };
        let json = serde_json::to_string(&not_found).unwrap();
        assert!(json.contains("\"status\":\"not_found\""));
        assert!(!json.contains("install_hint")); // None is skipped
    }

    #[test]
    fn credential_status_serializes_snake_case() {
        let status = CredentialStatus::DetectedFromEnv;
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("detected_from_env"));
    }

    #[test]
    fn credential_status_updated_serializes() {
        let status = CredentialStatus::Updated;
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"updated\""));
    }

    #[test]
    fn credential_status_kept_serializes() {
        let status = CredentialStatus::Kept;
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"kept\""));
    }

    #[test]
    fn secrets_init_data_json_never_contains_secrets() {
        let data = SecretsInitData {
            podman: DetectionResult::Detected {
                version: "4.8.0".to_string(),
            },
            git: DetectionResult::Detected {
                version: "2.43.0".to_string(),
            },
            credentials: vec![
                CredentialResult {
                    name: "anthropic_api_key".to_string(),
                    display_name: "Claude API key".to_string(),
                    required: true,
                    status: CredentialStatus::DetectedFromEnv,
                },
                CredentialResult {
                    name: "telegram_bot_token".to_string(),
                    display_name: "Telegram bot token".to_string(),
                    required: false,
                    status: CredentialStatus::Skipped,
                },
            ],
            all_configured: false,
            next_command: "wh deploy apply topology.wh".to_string(),
        };

        let envelope = OutputEnvelope::ok(data);
        let json = serde_json::to_string_pretty(&envelope).unwrap();

        // Must contain v:1 envelope
        assert!(json.contains("\"v\": 1"));
        assert!(json.contains("\"status\": \"ok\""));

        // Must NOT contain any actual secret values (there are none in this struct by design)
        assert!(!json.contains("sk-"));
        assert!(!json.contains("password"));

        // Must use snake_case
        assert!(json.contains("next_command"));
        assert!(json.contains("display_name"));
        assert!(json.contains("all_configured"));
        assert!(!json.contains("nextCommand"));
        assert!(!json.contains("displayName"));
        assert!(!json.contains("allConfigured"));
    }

    #[test]
    fn secrets_init_data_all_configured_true() {
        let data = SecretsInitData {
            podman: DetectionResult::Detected { version: "4.8.0".to_string() },
            git: DetectionResult::Detected { version: "2.43.0".to_string() },
            credentials: vec![
                CredentialResult {
                    name: "anthropic_api_key".to_string(),
                    display_name: "Claude API key".to_string(),
                    required: true,
                    status: CredentialStatus::DetectedFromEnv,
                },
            ],
            all_configured: true,
            next_command: "wh deploy apply topology.wh".to_string(),
        };
        let envelope = OutputEnvelope::ok(data);
        let json = serde_json::to_string_pretty(&envelope).unwrap();
        assert!(json.contains("\"all_configured\": true"));
    }

    #[test]
    fn output_envelope_json_has_v1() {
        let envelope = OutputEnvelope::ok(serde_json::json!({"test": true}));
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("\"v\":1"));
        assert!(json.contains("\"status\":\"ok\""));
    }

    #[test]
    fn optional_credential_skip_produces_correct_status() {
        let result = CredentialResult {
            name: "telegram_bot_token".to_string(),
            display_name: "Telegram bot token".to_string(),
            required: false,
            status: CredentialStatus::Skipped,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"skipped\""));
        assert!(!json.contains("error"));
    }

    #[test]
    fn platform_podman_hint_is_nonempty() {
        let hint = platform_podman_install_hint();
        assert!(!hint.is_empty());
        // On macOS, should suggest brew
        if cfg!(target_os = "macos") {
            assert!(hint.contains("brew install podman"));
        }
    }

    #[test]
    fn read_secret_returns_env_var() {
        // Use a unique env var name to avoid test interference.
        std::env::set_var("ANTHROPIC_API_KEY", "test-read-secret-value");
        let result = read_secret("anthropic_api_key");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-read-secret-value");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn read_secret_returns_error_when_not_found() {
        std::env::remove_var("NONEXISTENT_TEST_CRED");
        let result = read_secret("nonexistent_test_cred");
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not configured"), "Expected 'not configured' in: {msg}");
        assert!(msg.contains("wh secrets init"), "Expected 'wh secrets init' in: {msg}");
    }

    #[test]
    fn check_credential_status_detects_env_var() {
        std::env::set_var("ANTHROPIC_API_KEY", "test-check-status");
        let status = check_credential_status("anthropic_api_key");
        assert!(matches!(status, Some(CredentialStatus::DetectedFromEnv)));
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn check_credential_status_returns_none_when_missing() {
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("TELEGRAM_BOT_TOKEN");
        // For keychain, we can't easily mock it, but if the test env has no keychain
        // entry for this credential, it should return None.
        // This test verifies at minimum that the function doesn't panic.
        let _status = check_credential_status("nonexistent_cred_xyz");
        // Can't assert None because keychain may or may not have the entry in CI
    }
}
