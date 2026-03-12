//! `wh completion` — shell completion script generation.
//!
//! Generates completion scripts for bash, zsh, fish, elvish, and powershell.
//! Works fully offline — uses only clap metadata, no broker connection required.

use clap::{CommandFactory, Parser};
use clap_complete::{generate, Shell};

/// Arguments for the `wh completion` command.
#[derive(Debug, Parser)]
pub struct CompletionArgs {
    /// Shell to generate completions for (bash, zsh, fish, elvish, powershell).
    pub shell: Shell,
}

/// Generate shell completion script and write to stdout.
pub fn execute(args: &CompletionArgs) {
    let mut cmd = crate::Cli::command();
    generate(args.shell, &mut cmd, "wh", &mut std::io::stdout());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_bash_produces_output() {
        let mut cmd = crate::Cli::command();
        let mut buf = Vec::new();
        generate(Shell::Bash, &mut cmd, "wh", &mut buf);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(!output.is_empty(), "Bash completion should produce output");
        assert!(output.contains("wh"), "Bash completion should reference 'wh'");
    }

    #[test]
    fn completion_zsh_produces_output() {
        let mut cmd = crate::Cli::command();
        let mut buf = Vec::new();
        generate(Shell::Zsh, &mut cmd, "wh", &mut buf);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(!output.is_empty(), "Zsh completion should produce output");
        assert!(
            output.contains("#compdef") || output.contains("wh"),
            "Zsh completion should contain markers"
        );
    }

    #[test]
    fn completion_fish_produces_output() {
        let mut cmd = crate::Cli::command();
        let mut buf = Vec::new();
        generate(Shell::Fish, &mut cmd, "wh", &mut buf);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(!output.is_empty(), "Fish completion should produce output");
        assert!(
            output.contains("complete") || output.contains("wh"),
            "Fish completion should contain markers"
        );
    }

    #[test]
    fn completion_args_parses_valid_shells() {
        // Verify Shell enum accepts known shell names
        let shells = ["bash", "zsh", "fish", "elvish", "powershell"];
        for shell_name in &shells {
            let parsed: Result<Shell, _> = shell_name.parse();
            assert!(
                parsed.is_ok(),
                "Should parse shell name: {shell_name}"
            );
        }
    }

    #[test]
    fn cli_help_renders_without_panic() {
        let cmd = crate::Cli::command();
        // Rendering help text should not panic
        let mut buf = Vec::new();
        cmd.clone().write_help(&mut buf).expect("help should render");
        let help = String::from_utf8(buf).expect("valid utf8");
        assert!(help.contains("wh"), "Help should mention 'wh'");
        assert!(help.contains("completion"), "Help should list 'completion'");
    }

    #[test]
    fn exit_code_constants() {
        use crate::output::error::{EXIT_ERROR, EXIT_PLAN_CHANGE, EXIT_SUCCESS};
        assert_eq!(EXIT_SUCCESS, 0);
        assert_eq!(EXIT_ERROR, 1);
        assert_eq!(EXIT_PLAN_CHANGE, 2);
    }
}
