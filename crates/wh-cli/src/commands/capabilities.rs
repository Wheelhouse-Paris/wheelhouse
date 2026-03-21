//! `wh capabilities` — inspect the Wheelhouse capabilities manifest (ADR-031).
//!
//! Displays available features grouped by category with their status.
//! The manifest is embedded at compile time and is read-only at runtime (E12-05).

use clap::Args;
use console::style;
use serde::{Deserialize, Serialize};

use crate::output::{OutputEnvelope, OutputFormat};

/// The capabilities manifest JSON, embedded at compile time.
const CAPABILITIES_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/capabilities.json"));

/// Arguments for the `wh capabilities` command.
#[derive(Debug, Args)]
pub struct CapabilitiesArgs {
    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
}

/// A single feature entry in the capabilities manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    pub status: String,
    pub description: String,
}

/// The full capabilities manifest structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesManifest {
    pub version: String,
    pub generated: String,
    pub categories: std::collections::BTreeMap<String, Vec<Feature>>,
}

/// Parse the embedded capabilities manifest.
pub fn parse_manifest() -> Result<CapabilitiesManifest, String> {
    serde_json::from_str(CAPABILITIES_JSON)
        .map_err(|e| format!("Failed to parse capabilities manifest: {e}"))
}

/// Execute the `wh capabilities` command. Returns the process exit code.
pub fn execute(args: &CapabilitiesArgs) -> i32 {
    let manifest = match parse_manifest() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    match args.format {
        OutputFormat::Human => {
            print_human(&manifest);
        }
        OutputFormat::Json => {
            let envelope = OutputEnvelope::ok(&manifest);
            match serde_json::to_string_pretty(&envelope) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    eprintln!("Error: failed to serialize manifest: {e}");
                    return 1;
                }
            }
        }
    }
    0
}

/// Print the manifest in human-readable format, grouped by category.
fn print_human(manifest: &CapabilitiesManifest) {
    println!(
        "Wheelhouse Capabilities (v{})",
        style(&manifest.version).bold()
    );
    println!();

    for (category, features) in &manifest.categories {
        println!("  {}:", style(category).cyan().bold());
        for feature in features {
            let status_styled = match feature.status.as_str() {
                "available" => style(&feature.status).green(),
                "experimental" => style(&feature.status).yellow(),
                "deprecated" => style(&feature.status).red(),
                _ => style(&feature.status).dim(),
            };
            println!(
                "    {:<20} [{:<12}]  {}",
                feature.name, status_styled, feature.description
            );
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_successfully() {
        let manifest = parse_manifest().expect("Manifest should parse without error");
        assert!(!manifest.version.is_empty(), "Version should not be empty");
        assert!(
            !manifest.generated.is_empty(),
            "Generated timestamp should not be empty"
        );
    }

    #[test]
    fn manifest_version_matches_cargo_pkg_version() {
        let manifest = parse_manifest().expect("Manifest should parse");
        let pkg_version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            manifest.version, pkg_version,
            "Manifest version ({}) must match CARGO_PKG_VERSION ({})",
            manifest.version, pkg_version
        );
    }

    #[test]
    fn manifest_has_all_required_categories() {
        let manifest = parse_manifest().expect("Manifest should parse");
        let required = [
            "streams", "skills", "surfaces", "topology", "cron", "cli", "sdk",
        ];
        for cat in &required {
            assert!(
                manifest.categories.contains_key(*cat),
                "Missing required category: {cat}"
            );
        }
    }

    #[test]
    fn manifest_features_have_valid_status() {
        let manifest = parse_manifest().expect("Manifest should parse");
        let valid_statuses = ["available", "experimental", "deprecated"];
        for (category, features) in &manifest.categories {
            for feature in features {
                assert!(
                    valid_statuses.contains(&feature.status.as_str()),
                    "Feature '{}' in category '{}' has invalid status '{}'",
                    feature.name,
                    category,
                    feature.status
                );
            }
        }
    }

    #[test]
    fn manifest_json_output_is_valid() {
        let manifest = parse_manifest().expect("Manifest should parse");
        let envelope = OutputEnvelope::ok(&manifest);
        let json = serde_json::to_string_pretty(&envelope).expect("Should serialize to JSON");
        assert!(json.contains("\"status\": \"ok\""));
        assert!(json.contains("\"v\": 1"));
    }

    #[test]
    fn manifest_generated_is_iso8601() {
        let manifest = parse_manifest().expect("Manifest should parse");
        // Basic ISO8601 check: starts with YYYY-MM-DD
        assert!(
            manifest.generated.len() >= 19,
            "Generated timestamp too short: {}",
            manifest.generated
        );
        assert!(
            manifest.generated.contains('T'),
            "Generated timestamp missing 'T' separator: {}",
            manifest.generated
        );
    }

    #[test]
    fn manifest_each_category_has_features() {
        let manifest = parse_manifest().expect("Manifest should parse");
        for (category, features) in &manifest.categories {
            assert!(
                !features.is_empty(),
                "Category '{}' should have at least one feature",
                category
            );
        }
    }
}
