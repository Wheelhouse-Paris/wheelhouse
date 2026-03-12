//! wh-cli — Wheelhouse CLI library.
//!
//! Unified control plane for operators and agents.
//! Public modules for testability. The binary entry point is `main.rs`.

pub mod client;
pub mod commands;
pub mod lint;
pub mod model;
pub mod output;
pub mod reconnect;

use clap::{Parser, Subcommand};

use commands::compact::CompactArgs;
use commands::completion::CompletionArgs;
use commands::deploy::DeployCommand;
use commands::doctor::DoctorArgs;
use commands::logs::LogsArgs;
use commands::memory::MemoryCommand;
use commands::ps::PsArgs;
use commands::secrets::SecretsCmd;
use commands::stream::StreamCommand;
use commands::surface::SurfaceCommand;
use output::OutputFormat;

/// wh — the Wheelhouse CLI.
///
/// Unified control plane for operators and agents.
/// Concise getting-started hint shown when wh is invoked with no arguments (AC#5).
pub const GETTING_STARTED_HINT: &str = "\
Wheelhouse — local-first agent orchestration
  wh secrets init    Configure credentials
  wh deploy apply    Deploy your topology
  wh --help          Show all commands";

#[derive(Debug, Parser)]
#[command(name = "wh", version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("WH_GIT_HASH"), ", ", env!("WH_TARGET_TRIPLE"), ")"), about, subcommand_required = false)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Top-level CLI subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// List all deployed components with their live status.
    Ps(PsArgs),
    /// Tail structured logs from a specific agent.
    Logs(LogsArgs),
    /// Manage Wheelhouse secrets and credentials.
    Secrets {
        #[command(subcommand)]
        cmd: SecretsCmd,
    },
    /// Manage topology deployment.
    Deploy {
        #[command(subcommand)]
        command: DeployCommand,
    },
    /// Manage surfaces (CLI, Telegram, etc.).
    Surface {
        #[command(subcommand)]
        command: SurfaceCommand,
    },
    /// Manage and observe streams.
    Stream {
        #[command(subcommand)]
        command: StreamCommand,
    },
    /// Check Wheelhouse health and status.
    Status {
        /// Output format: human (default) or json.
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },
    /// Manage agent memory (MEMORY.md) files.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Compact a stream — produce a daily summary and truncate WAL.
    Compact(CompactArgs),
    /// Generate shell completion scripts.
    Completion(CompletionArgs),
    /// Check git repository health and secrets exclusion (FM-07).
    Doctor(DoctorArgs),
}
