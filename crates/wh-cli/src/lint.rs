//! Lint validation engine for `.wh` topology files.
//!
//! Validates file format, required fields, semantic constraints, and
//! produces compiler-style diagnostics.

use std::collections::HashSet;
use std::path::Path;

use crate::model::WhFile;
use crate::output::LintError;

/// The only supported API version in MVP.
const SUPPORTED_API_VERSION: &str = "wheelhouse.dev/v1";

/// The only supported stream provider in MVP (FR12).
const SUPPORTED_PROVIDERS: &[&str] = &["local"];

/// Severity level for lint diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Error,
    Warning,
}

impl std::fmt::Display for DiagnosticLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticLevel::Error => write!(f, "error"),
            DiagnosticLevel::Warning => write!(f, "warning"),
        }
    }
}

/// A single lint diagnostic with compiler-style formatting.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LintDiagnostic {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub level: DiagnosticLevel,
    pub message: String,
    pub hint: String,
}

impl std::fmt::Display for LintDiagnostic {
    /// Compiler-style format: `{file}:{line}: {level}: {message} — {hint}`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.file)?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        write!(f, ": {}: {} \u{2014} {}", self.level, self.message, self.hint)
    }
}

/// Result of linting a `.wh` file. Collects all diagnostics before reporting.
#[must_use]
#[derive(Debug, Clone, serde::Serialize)]
pub struct LintResult {
    pub errors: Vec<LintDiagnostic>,
    pub warnings: Vec<LintDiagnostic>,
}

impl LintResult {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// A validated `.wh` file — typestate token.
///
/// Can only be constructed by a successful `lint()` call (zero errors).
/// First step of the deploy pipeline typestate:
/// `LintedFile -> PlanOutput -> CommittedPlan -> apply()` (5W-03).
pub struct LintedFile {
    file: WhFile,
    path: std::path::PathBuf,
}

impl LintedFile {
    /// Only constructible from within this module.
    pub(crate) fn new(file: WhFile, path: std::path::PathBuf) -> Self {
        Self { file, path }
    }

    pub fn file(&self) -> &WhFile {
        &self.file
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Parse and lint a `.wh` file at the given path.
///
/// Returns `Ok((LintResult, Option<LintedFile>))`:
/// - `LintedFile` is `Some` only if there are zero errors.
/// - Warnings do not prevent `LintedFile` construction.
pub fn lint_file(path: &Path) -> Result<(LintResult, Option<LintedFile>), LintError> {
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    // Read file
    let content = std::fs::read_to_string(path).map_err(LintError::FileReadError)?;

    // Parse YAML
    let wh_file: WhFile = match serde_yaml::from_str(&content) {
        Ok(f) => f,
        Err(e) => {
            let line = e.location().map(|loc| loc.line());
            return Err(LintError::YamlParseError(format!(
                "{}{}: {e}",
                filename,
                line.map(|l| format!(":{l}")).unwrap_or_default(),
            )));
        }
    };

    // Run semantic validation
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    validate_api_version(&wh_file, &filename, &mut errors);
    validate_agents(&wh_file, &filename, &mut errors);
    validate_streams(&wh_file, &filename, &mut errors, &mut warnings);
    validate_stream_references(&wh_file, &filename, &mut errors);

    let result = LintResult {
        errors,
        warnings,
    };

    let linted = if result.has_errors() {
        None
    } else {
        Some(LintedFile::new(wh_file, path.to_path_buf()))
    };

    Ok((result, linted))
}

fn validate_api_version(wh_file: &WhFile, filename: &str, errors: &mut Vec<LintDiagnostic>) {
    match &wh_file.api_version {
        None => {
            errors.push(LintDiagnostic {
                file: filename.to_string(),
                line: None,
                level: DiagnosticLevel::Error,
                message: "field 'apiVersion' is required".to_string(),
                hint: format!("add apiVersion: {SUPPORTED_API_VERSION}"),
            });
        }
        Some(version) if version != SUPPORTED_API_VERSION => {
            errors.push(LintDiagnostic {
                file: filename.to_string(),
                line: None,
                level: DiagnosticLevel::Error,
                message: format!("unsupported apiVersion '{version}'"),
                hint: format!("use apiVersion: {SUPPORTED_API_VERSION}"),
            });
        }
        _ => {}
    }
}

fn validate_agents(wh_file: &WhFile, filename: &str, errors: &mut Vec<LintDiagnostic>) {
    let agents = match &wh_file.agents {
        Some(agents) => agents,
        None => return,
    };

    let mut seen_names = HashSet::new();

    for (i, agent) in agents.iter().enumerate() {
        let agent_label = agent
            .name
            .as_deref()
            .map(|n| format!("agent '{n}'"))
            .unwrap_or_else(|| format!("agent at index {i}"));

        // Validate name
        match &agent.name {
            None => {
                errors.push(LintDiagnostic {
                    file: filename.to_string(),
                    line: None,
                    level: DiagnosticLevel::Error,
                    message: format!("field 'name' is required on {agent_label}"),
                    hint: "add name to agent definition".to_string(),
                });
            }
            Some(name) => {
                if !seen_names.insert(name.clone()) {
                    errors.push(LintDiagnostic {
                        file: filename.to_string(),
                        line: None,
                        level: DiagnosticLevel::Error,
                        message: format!("duplicate agent name '{name}'"),
                        hint: "each agent must have a unique name".to_string(),
                    });
                }
            }
        }

        // Validate image
        if agent.image.is_none() {
            errors.push(LintDiagnostic {
                file: filename.to_string(),
                line: None,
                level: DiagnosticLevel::Error,
                message: format!("field 'image' is required on {agent_label}"),
                hint: "add image to agent definition (e.g., my-org/agent:latest)".to_string(),
            });
        }

        // Validate max_replicas
        if agent.max_replicas.is_none() {
            errors.push(LintDiagnostic {
                file: filename.to_string(),
                line: None,
                level: DiagnosticLevel::Error,
                message: format!("field 'max_replicas' is required on {agent_label}"),
                hint: "add max_replicas to prevent unconstrained scaling".to_string(),
            });
        }
    }
}

fn validate_streams(
    wh_file: &WhFile,
    filename: &str,
    errors: &mut Vec<LintDiagnostic>,
    warnings: &mut Vec<LintDiagnostic>,
) {
    let streams = match &wh_file.streams {
        Some(streams) => streams,
        None => return,
    };

    let mut seen_names = HashSet::new();

    for (i, stream) in streams.iter().enumerate() {
        let stream_label = stream
            .name
            .as_deref()
            .map(|n| format!("stream '{n}'"))
            .unwrap_or_else(|| format!("stream at index {i}"));

        // Validate name
        match &stream.name {
            None => {
                errors.push(LintDiagnostic {
                    file: filename.to_string(),
                    line: None,
                    level: DiagnosticLevel::Error,
                    message: format!("field 'name' is required on {stream_label}"),
                    hint: "add name to stream definition".to_string(),
                });
            }
            Some(name) => {
                if !seen_names.insert(name.clone()) {
                    errors.push(LintDiagnostic {
                        file: filename.to_string(),
                        line: None,
                        level: DiagnosticLevel::Error,
                        message: format!("duplicate stream name '{name}'"),
                        hint: "each stream must have a unique name".to_string(),
                    });
                }
            }
        }

        // Validate provider
        if let Some(provider) = &stream.provider {
            if !SUPPORTED_PROVIDERS.contains(&provider.as_str()) {
                errors.push(LintDiagnostic {
                    file: filename.to_string(),
                    line: None,
                    level: DiagnosticLevel::Error,
                    message: format!(
                        "unsupported provider '{provider}' on {stream_label}"
                    ),
                    hint: format!(
                        "use a supported provider: {}",
                        SUPPORTED_PROVIDERS.join(", ")
                    ),
                });
            }
        }

        // Warn on missing compaction cron (FM-06)
        if stream.compaction_cron.is_none() {
            if let Some(name) = &stream.name {
                warnings.push(LintDiagnostic {
                    file: filename.to_string(),
                    line: None,
                    level: DiagnosticLevel::Warning,
                    message: format!(
                        "stream '{name}' has no compaction cron declared"
                    ),
                    hint: "WAL will grow unbounded".to_string(),
                });
            }
        }
    }
}

fn validate_stream_references(
    wh_file: &WhFile,
    filename: &str,
    errors: &mut Vec<LintDiagnostic>,
) {
    let declared_streams: HashSet<String> = wh_file
        .streams
        .as_ref()
        .map(|streams| {
            streams
                .iter()
                .filter_map(|s| s.name.clone())
                .collect()
        })
        .unwrap_or_default();

    let agents = match &wh_file.agents {
        Some(agents) => agents,
        None => return,
    };

    for agent in agents {
        let agent_name = agent
            .name
            .as_deref()
            .unwrap_or("unknown");

        if let Some(stream_refs) = &agent.streams {
            for stream_ref in stream_refs {
                if !declared_streams.contains(stream_ref) {
                    errors.push(LintDiagnostic {
                        file: filename.to_string(),
                        line: None,
                        level: DiagnosticLevel::Error,
                        message: format!(
                            "agent '{agent_name}' references undeclared stream '{stream_ref}'"
                        ),
                        hint: format!(
                            "declare stream '{stream_ref}' in the streams section or fix the reference"
                        ),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_wh(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::with_suffix(".wh").unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn valid_file_produces_no_errors() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
    streams: [main]
streams:
  - name: main
    provider: local
    compaction_cron: "0 2 * * *"
"#,
        );
        let (result, linted) = lint_file(f.path()).unwrap();
        assert!(!result.has_errors(), "errors: {:?}", result.errors);
        assert!(result.warnings.is_empty(), "warnings: {:?}", result.warnings);
        assert!(linted.is_some(), "LintedFile should be produced");
    }

    #[test]
    fn missing_api_version_produces_error() {
        let f = write_wh(
            r#"
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
streams:
  - name: main
"#,
        );
        let (result, linted) = lint_file(f.path()).unwrap();
        assert!(result.has_errors());
        assert!(linted.is_none());
        assert!(
            result.errors.iter().any(|e| e.message.contains("apiVersion")),
            "should mention apiVersion: {:?}",
            result.errors
        );
    }

    #[test]
    fn wrong_api_version_produces_error() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v2
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
streams:
  - name: main
"#,
        );
        let (result, _) = lint_file(f.path()).unwrap();
        assert!(result.has_errors());
        assert!(result.errors.iter().any(|e| e.message.contains("v2")));
    }

    #[test]
    fn missing_max_replicas_produces_error() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
streams:
  - name: main
    compaction_cron: "0 2 * * *"
"#,
        );
        let (result, _) = lint_file(f.path()).unwrap();
        assert!(result.has_errors());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.message.contains("max_replicas")),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn stream_without_compaction_cron_produces_warning() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
streams:
  - name: main
    provider: local
"#,
        );
        let (result, linted) = lint_file(f.path()).unwrap();
        assert!(!result.has_errors(), "should have no errors");
        assert!(linted.is_some(), "should produce LintedFile (warnings only)");
        assert!(
            result.warnings.iter().any(|w| w.message.contains("compaction")
                && w.message.contains("main")),
            "warnings: {:?}",
            result.warnings
        );
        assert!(
            result.warnings.iter().any(|w| w.hint.contains("WAL will grow unbounded")),
            "hint should mention WAL growth"
        );
    }

    #[test]
    fn provider_local_is_valid() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
streams:
  - name: main
    provider: local
    compaction_cron: "0 2 * * *"
"#,
        );
        let (result, _) = lint_file(f.path()).unwrap();
        assert!(!result.has_errors());
    }

    #[test]
    fn provider_aws_produces_error() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
streams:
  - name: main
    provider: aws
    compaction_cron: "0 2 * * *"
"#,
        );
        let (result, _) = lint_file(f.path()).unwrap();
        assert!(result.has_errors());
        assert!(result.errors.iter().any(|e| e.message.contains("aws")));
    }

    #[test]
    fn duplicate_agent_names_produce_error() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
  - name: researcher
    image: my-org/researcher:v2
    max_replicas: 1
streams:
  - name: main
    compaction_cron: "0 2 * * *"
"#,
        );
        let (result, _) = lint_file(f.path()).unwrap();
        assert!(result.has_errors());
        assert!(result.errors.iter().any(|e| e.message.contains("duplicate")));
    }

    #[test]
    fn duplicate_stream_names_produce_error() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
streams:
  - name: main
    compaction_cron: "0 2 * * *"
  - name: main
    provider: local
"#,
        );
        let (result, _) = lint_file(f.path()).unwrap();
        assert!(result.has_errors());
        assert!(result.errors.iter().any(|e| e.message.contains("duplicate")));
    }

    #[test]
    fn agent_referencing_undeclared_stream_produces_error() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
    streams: [nonexistent]
streams:
  - name: main
    compaction_cron: "0 2 * * *"
"#,
        );
        let (result, _) = lint_file(f.path()).unwrap();
        assert!(result.has_errors());
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.message.contains("nonexistent"))
        );
    }

    #[test]
    fn json_output_format_has_correct_fields() {
        let diagnostic = LintDiagnostic {
            file: "test.wh".to_string(),
            line: Some(5),
            level: DiagnosticLevel::Error,
            message: "test error".to_string(),
            hint: "fix it".to_string(),
        };
        let json = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(json["file"], "test.wh");
        assert_eq!(json["line"], 5);
        assert_eq!(json["level"], "error");
        assert_eq!(json["message"], "test error");
        assert_eq!(json["hint"], "fix it");
        // Verify snake_case field names (SCV-01)
        assert!(json.get("File").is_none(), "field names must be snake_case");
    }

    #[test]
    fn empty_topology_is_valid() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
"#,
        );
        let (result, linted) = lint_file(f.path()).unwrap();
        assert!(!result.has_errors());
        assert!(linted.is_some());
    }

    #[test]
    fn provider_absent_defaults_to_local() {
        let f = write_wh(
            r#"
apiVersion: wheelhouse.dev/v1
agents:
  - name: researcher
    image: my-org/researcher:latest
    max_replicas: 3
streams:
  - name: main
    compaction_cron: "0 2 * * *"
"#,
        );
        let (result, _) = lint_file(f.path()).unwrap();
        assert!(
            !result.has_errors(),
            "absent provider should be valid (defaults to local)"
        );
    }
}
