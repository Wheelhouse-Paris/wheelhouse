//! Lint step of the deploy pipeline.
//!
//! Produces a `LintedFile` typestate token that must be consumed by `plan()`.
//! This enforces the FM-03 transaction order at compile time (5W-03).

use std::path::{Path, PathBuf};

use crate::deploy::{
    load_topology, load_topology_from_path, ComponentSourceMap, DeployError, Topology,
};

/// A successfully linted topology file (or folder).
///
/// This typestate token proves that the topology file has been parsed and
/// validated. It must be consumed by `plan()` — the compiler enforces this
/// via `#[must_use]`.
#[must_use = "a LintedFile must be passed to plan() — do not discard"]
#[derive(Debug)]
pub struct LintedFile {
    pub(crate) topology: Topology,
    pub(crate) source_path: PathBuf,
    /// Component-to-source-file mapping (populated for folder-based composition).
    pub(crate) source_map: ComponentSourceMap,
}

impl LintedFile {
    /// Returns a reference to the parsed topology.
    pub fn topology(&self) -> &Topology {
        &self.topology
    }

    /// Returns the source file path.
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Returns the component source map (E12-03).
    pub fn source_map(&self) -> &ComponentSourceMap {
        &self.source_map
    }
}

/// Lint a `.wh` topology path (file or folder): parse, validate structure,
/// and return a `LintedFile` token.
///
/// When `path` is a directory, all `*.wh` files are discovered, parsed
/// independently, merged, and validated for cross-file consistency (ADR-030).
///
/// This is the entry point of the deploy pipeline typestate chain.
///
/// **Known limitation**: This broker-side lint only validates YAML structure via serde
/// deserialization (`parse_topology()`). It does NOT validate field-level rules for
/// surfaces (e.g., `kind` must be "telegram"/"cli", `stream` must reference a declared
/// stream). Those validations live in the CLI lint layer (`wh-cli/src/lint.rs:validate_surfaces()`).
/// If the broker is called directly (not via CLI), surfaces with invalid field values
/// will be accepted. This mirrors the pre-existing pattern for agents and streams.
#[tracing::instrument(skip_all, fields(path = %path.as_ref().display()))]
#[must_use = "a LintedFile must be passed to plan() — do not discard"]
pub fn lint(path: impl AsRef<Path>) -> Result<LintedFile, DeployError> {
    let path = path.as_ref();

    if path.is_dir() {
        let (topology, source_map) = load_topology_from_path(path)?;
        Ok(LintedFile {
            topology,
            source_path: path.to_path_buf(),
            source_map,
        })
    } else {
        let topology = load_topology(path)?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let source_map = ComponentSourceMap::from_topology(&topology, &filename);
        Ok(LintedFile {
            topology,
            source_path: path.to_path_buf(),
            source_map,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn lint_valid_file_returns_linted_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(
            tmp,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\nstreams: []\n"
        )
        .unwrap();

        let linted = lint(tmp.path()).unwrap();
        assert_eq!(linted.topology().name, "dev");
    }

    #[test]
    fn lint_missing_file_returns_error() {
        let result = lint("/nonexistent/path/topology.wh");
        assert!(result.is_err());
    }

    #[test]
    fn lint_invalid_yaml_returns_error() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "not valid yaml: {{{{").unwrap();

        let result = lint(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn lint_folder_merges_multiple_files() {
        let dir = tempfile::tempdir().unwrap();

        // File 1: base topology with agents
        std::fs::write(
            dir.path().join("01-base.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: researcher:latest\n",
        )
        .unwrap();

        // File 2: streams
        std::fs::write(
            dir.path().join("02-streams.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nstreams:\n  - name: main\n",
        )
        .unwrap();

        let linted = lint(dir.path()).unwrap();
        assert_eq!(linted.topology().name, "dev");
        assert_eq!(linted.topology().agents.len(), 1);
        assert_eq!(linted.topology().streams.len(), 1);
        assert_eq!(linted.topology().agents[0].name, "researcher");
        assert_eq!(linted.topology().streams[0].name, "main");
    }

    #[test]
    fn lint_folder_detects_duplicate_agent_names() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("a.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n",
        )
        .unwrap();

        std::fs::write(
            dir.path().join("b.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r2:latest\n",
        )
        .unwrap();

        let err = lint(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate agent name 'researcher'"),
            "got: {msg}"
        );
    }

    #[test]
    fn lint_folder_detects_api_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("a.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\n",
        )
        .unwrap();

        // This will fail at parse_topology level since v2 is unsupported,
        // but the error message should indicate the problem.
        std::fs::write(
            dir.path().join("b.wh"),
            "api_version: wheelhouse.dev/v2\nname: dev\n",
        )
        .unwrap();

        let err = lint(dir.path()).unwrap_err();
        assert!(err.to_string().contains("api_version") || err.to_string().contains("apiVersion"));
    }

    #[test]
    fn lint_folder_empty_directory_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = lint(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("no .wh files found"),
            "got: {}",
            err
        );
    }

    #[test]
    fn lint_folder_source_map_populated() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("agents.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: donna\n    image: d:latest\n",
        )
        .unwrap();

        std::fs::write(
            dir.path().join("streams.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nstreams:\n  - name: main\n",
        )
        .unwrap();

        let linted = lint(dir.path()).unwrap();
        assert_eq!(
            linted.source_map().source_file("agent:donna"),
            Some("agents.wh")
        );
        assert_eq!(
            linted.source_map().source_file("stream:main"),
            Some("streams.wh")
        );
    }

    #[test]
    fn lint_single_file_backward_compatible() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(
            tmp,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\nstreams:\n  - name: main\n"
        )
        .unwrap();

        let linted = lint(tmp.path()).unwrap();
        assert_eq!(linted.topology().name, "dev");
        assert_eq!(linted.topology().agents.len(), 1);
    }
}
