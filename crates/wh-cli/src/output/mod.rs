//! Output formatting module (SCV-05).
//!
//! All CLI output is routed through this module's format switch.
//! Never serialize JSON directly in command handlers.

pub mod error;

use error::WhError;

/// Output format selector.
///
/// Currently a library-level enum used by `render_error`. Wiring to a
/// `--format` clap flag is a future story responsibility when actual
/// commands are implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable terminal output
    Human,
    /// Machine-parseable JSON output
    Json,
}

/// Render an error in the specified output format.
///
/// - `Human`: produces `Error [WH-XXXX]: <message>` with optional context lines
/// - `Json`: produces the full JSON error envelope per ADR-014
pub fn render_error(error: &WhError, format: OutputFormat) -> String {
    match format {
        OutputFormat::Human => format!("{}", error),
        OutputFormat::Json => serde_json::to_string(error).expect("WhError serialization cannot fail"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use error::{ErrorContext, WhError};

    #[test]
    fn test_render_error_human() {
        let err = WhError::ConnectionError {
            message: "Wheelhouse not running".into(),
            context: ErrorContext::default(),
        };
        let output = render_error(&err, OutputFormat::Human);
        assert!(output.contains("WH-1001"));
        assert!(output.contains("Wheelhouse not running"));
    }

    #[test]
    fn test_render_error_json() {
        let err = WhError::StreamError {
            message: "stream 'main' not found".into(),
            context: ErrorContext::default(),
        };
        let output = render_error(&err, OutputFormat::Json);
        let v: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["status"], "error");
        assert_eq!(v["code"], 3001);
    }

    #[test]
    fn test_render_error_json_with_context() {
        let err = WhError::DeployError {
            kind: error::DeployErrorKind::LintError,
            message: "invalid field".into(),
            context: ErrorContext {
                file: Some("test.wh".into()),
                line: Some(10),
                field: Some("name".into()),
            },
        };
        let output = render_error(&err, OutputFormat::Json);
        let v: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(v["context"]["file"], "test.wh");
        assert_eq!(v["context"]["line"], 10);
        assert_eq!(v["context"]["field"], "name");
    }
}
