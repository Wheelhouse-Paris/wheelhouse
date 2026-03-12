//! Lint step of the deploy pipeline.
//!
//! Produces a `LintedFile` typestate token that must be consumed by `plan()`.
//! This enforces the FM-03 transaction order at compile time (5W-03).

use std::path::{Path, PathBuf};

use crate::deploy::{load_topology, DeployError, Topology};

/// A successfully linted topology file.
///
/// This typestate token proves that the topology file has been parsed and
/// validated. It must be consumed by `plan()` — the compiler enforces this
/// via `#[must_use]`.
#[must_use = "a LintedFile must be passed to plan() — do not discard"]
#[derive(Debug)]
pub struct LintedFile {
    pub(crate) topology: Topology,
    pub(crate) source_path: PathBuf,
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
}

/// Lint a `.wh` topology file: parse, validate structure, and return a `LintedFile` token.
///
/// This is the entry point of the deploy pipeline typestate chain.
#[tracing::instrument(skip_all, fields(path = %path.as_ref().display()))]
#[must_use = "a LintedFile must be passed to plan() — do not discard"]
pub fn lint(path: impl AsRef<Path>) -> Result<LintedFile, DeployError> {
    let path = path.as_ref();
    let topology = load_topology(path)?;

    Ok(LintedFile {
        topology,
        source_path: path.to_path_buf(),
    })
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
}
