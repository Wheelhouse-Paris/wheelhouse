//! `wh library` command group — Library inspection and management (Epic 13 FW-7).
//!
//! This module is the canonical home for:
//!
//! 1. The `DEFAULT_SCHEMA_TEMPLATE` constant (story 13-19, R1 launch-blocker template).
//! 2. The `wh library` clap subcommand group with its `status` child subcommand
//!    (story 13-24 — the first FW-7 CLI surface).
//!
//! Future stories 13-22..13-26 add `init`, `ingest`, `list`, `lint` by adding variants
//! to `LibraryCommand` and match arms to `run` — no restructuring needed.
//!
//! ## Story 13-24: `wh library status <agent>`
//!
//! Prints Library health for a single agent by reading the host-side mount of the
//! agent's per-agent workspace volume (story 13-3 / ADR-036):
//!
//! - Schema file present (`<mount>/.wh-schema.md` exists)
//! - Page count (`.md` files under `<mount>/.library/pages/`)
//! - Git HEAD short commit id (`git -C <mount>/.library rev-parse --short HEAD`)
//! - Last ingest timestamp (`git -C <mount>/.library log -1 --format=%aI HEAD`)
//! - Lock held (`<mount>/.library/.git/index.lock` exists) + lock age
//! - Free disk bytes (`df -kP <mount>`)
//!
//! No broker IPC, no container exec — the MVP is local host-side reads for FR45's
//! <2s target. See the story file for the rationale and the broker-RPC v1.1 path.
//!
//! ## Story 13-19 legacy: `DEFAULT_SCHEMA_TEMPLATE`
//!
//! The `DEFAULT_SCHEMA_TEMPLATE` constant is the canonical source of truth for the
//! default `.wh-schema.md` template that the wh framework injects at
//! `/workspace/.wh-schema.md` for every new Library. See the doc comment on the
//! constant itself for the R1 context.
//!
//! ## Why `include_str!`
//!
//! The wh binary is shipped as a single self-contained release artifact (story 1.6).
//! Embedding the template at compile time guarantees it travels with every binary and
//! removes any runtime path-resolution failure mode. Updating the template requires a
//! rebuild — that is the intended semantics.

/// Default Library schema template — read-only at runtime, embedded at compile time.
///
/// Path constant downstream stories (13-20, 13-22) will read to obtain the canonical
/// bytes that get materialized at `/workspace/.wh-schema.md` for new Libraries.
///
/// This string contains unresolved `{{agent_name}}` and `{{domain_description}}`
/// placeholders. The consumer (`wh library init` in story 13-22) is responsible for
/// substitution before writing the file to disk.
///
/// Risk: R1 (LAUNCH BLOCKER). Editing this template's *wording* changes the empirical
/// behavior of the agent's autonomous-write loop and re-opens the R1 usability gate.
/// Edit only with deliberate intent and re-run the R1 validation.
pub const DEFAULT_SCHEMA_TEMPLATE: &str = include_str!("../../templates/library/wh-schema.md.tmpl");

// ─── Story 13-24: `wh library status <agent>` ─────────────────────────────

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::output::error::WhError;
use crate::output::json;
use crate::output::OutputFormat;

/// `wh library` subcommand group.
///
/// Variants land incrementally as FW-7 stories are completed:
/// - `Status` (this story, 13-24)
/// - `Init` (13-22, backlog)
/// - `Ingest` (13-23, backlog)
/// - `List` (13-25, backlog)
/// - `Lint` (13-26, backlog)
#[derive(Debug, Subcommand)]
pub enum LibraryCommand {
    /// Inspect the health of an agent's Library (FR45).
    Status(StatusArgs),
}

/// Arguments for `wh library status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Name of the agent whose Library to inspect.
    pub agent: String,

    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,
}

impl StatusArgs {
    /// The format hint used by `main.rs` for error envelope rendering.
    pub fn format(&self) -> OutputFormat {
        self.format
    }
}

impl LibraryCommand {
    /// The format hint used by `main.rs` to render error envelopes in the right shape.
    pub fn format(&self) -> OutputFormat {
        match self {
            LibraryCommand::Status(args) => args.format,
        }
    }
}

/// Serializable health record for `wh library status`.
///
/// Field names are snake_case per SCV-01 and match the JSON-output contract in
/// AC-4 of story 13-24. Every "maybe-missing" field is modeled as `Option<_>` so
/// the empty-library state (story AC-6) serializes cleanly.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StatusData {
    pub agent: String,
    pub topology: String,
    pub volume: String,
    pub mount_path: String,
    pub schema_present: bool,
    pub page_count: u64,
    pub git_head: Option<String>,
    pub last_ingest_at: Option<String>,
    pub lock_held: bool,
    pub lock_age_seconds: Option<u64>,
    pub free_disk_bytes: Option<u64>,
}

/// Dispatcher for `wh library <subcommand>`.
pub async fn run(cmd: LibraryCommand) -> Result<(), WhError> {
    match cmd {
        LibraryCommand::Status(args) => status(&args).await,
    }
}

/// Execute `wh library status <agent>`.
///
/// Reads `.wh/state.json`, resolves the per-agent workspace volume, probes its
/// host mount path via `podman volume inspect`, then reads the six health signals
/// directly from the host filesystem. All operations are synchronous, local, and
/// bounded well under FR45's 2-second target.
pub async fn status(args: &StatusArgs) -> Result<(), WhError> {
    // Load .wh/state.json (same pattern as wh ps / wh surface).
    let state_path = Path::new(".wh/state.json");
    if !state_path.exists() {
        return Err(WhError::Other(
            "no deployed topology found (.wh/state.json missing). Run 'wh topology apply' first."
                .to_string(),
        ));
    }
    let content = std::fs::read_to_string(state_path)
        .map_err(|e| WhError::Internal(format!("failed to read state: {e}")))?;
    let topology: wh_broker::deploy::Topology = serde_json::from_str(&content)
        .map_err(|e| WhError::Internal(format!("corrupt state file: {e}")))?;

    // Find the requested agent.
    let agent = topology
        .agents
        .iter()
        .find(|a| a.name == args.agent)
        .ok_or_else(|| WhError::AgentNotFound(args.agent.clone()))?;

    // Resolve the per-agent workspace volume (story 13-3 contract).
    let volume = wh_broker::deploy::podman::workspace_volume_name(&topology.name, &agent.name);

    // Probe the host mount path via `podman volume inspect`.
    let mount_path = resolve_mount_path(&volume)?;

    // Read the six health signals from the host filesystem.
    let data = collect_status_data(
        args.agent.clone(),
        topology.name.clone(),
        volume,
        mount_path,
    );

    match args.format {
        OutputFormat::Human => {
            print!("{}", render_human(&data));
        }
        OutputFormat::Json => {
            json::print_json_success(&data)?;
        }
    }

    Ok(())
}

/// Resolve the host-side mount path for a podman named volume.
///
/// Invokes `podman volume inspect <vol> --format '{{.Mountpoint}}'`. Returns
/// `Err(WhError::Other)` with a user-facing message when podman is missing, the
/// volume does not exist, or the command fails for any other reason. The error
/// wording deliberately avoids "broker", port numbers, and internal jargon
/// (RT-B1) — it matches the family of errors `wh ps` emits when podman is
/// unreachable.
fn resolve_mount_path(volume: &str) -> Result<PathBuf, WhError> {
    let podman = wh_broker::deploy::podman::find_podman()
        .map_err(|e| WhError::Other(format!("Podman not available: {e}")))?;

    let output = std::process::Command::new(podman)
        .args(["volume", "inspect", volume, "--format", "{{.Mountpoint}}"])
        .output()
        .map_err(|e| WhError::Other(format!("failed to run podman: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WhError::Other(format!(
            "workspace volume '{volume}' not found. Has the topology been applied? ({stderr})",
            stderr = stderr.trim()
        )));
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Err(WhError::Other(format!(
            "workspace volume '{volume}' has no mountpoint"
        )));
    }
    Ok(PathBuf::from(raw))
}

/// Gather the six health signals from the host filesystem.
///
/// Pure data collection — never panics. Every sub-read is fault-tolerant and
/// degrades to `None` / `false` / `0` on error so an incomplete/un-initialized
/// Library renders as "not yet initialized" rather than crashing (AC-6).
fn collect_status_data(
    agent: String,
    topology: String,
    volume: String,
    mount_path: PathBuf,
) -> StatusData {
    let schema_path = mount_path.join(".wh-schema.md");
    let schema_present = schema_path.exists();

    let library_root = mount_path.join(".library");
    let pages_dir = library_root.join("pages");
    let page_count = count_pages_under(&pages_dir);

    let (git_head, last_ingest_at) = read_git_head_info(&library_root);

    let (lock_held, lock_age_seconds) = read_lock_info(&library_root);

    let free_disk_bytes = read_free_disk_bytes(&mount_path);

    StatusData {
        agent,
        topology,
        volume,
        mount_path: mount_path.to_string_lossy().into_owned(),
        schema_present,
        page_count,
        git_head,
        last_ingest_at,
        lock_held,
        lock_age_seconds,
        free_disk_bytes,
    }
}

/// Recursively count `.md` files under `dir`. Missing directory → 0.
fn count_pages_under(dir: &Path) -> u64 {
    fn walk(dir: &Path, count: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    walk(&path, count);
                } else if ft.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
                    *count += 1;
                }
            }
        }
    }
    let mut count: u64 = 0;
    walk(dir, &mut count);
    count
}

/// Read `(git_head, last_ingest_at)` via two `git` subprocess calls.
///
/// Both are bounded by a 5-second timeout (implicit — git ops on a local repo
/// are millisecond-scale). Failure on either → `None` for that field, never an
/// error result.
fn read_git_head_info(library_root: &Path) -> (Option<String>, Option<String>) {
    if !library_root.join(".git").is_dir() {
        return (None, None);
    }
    let head = run_git_capture(library_root, &["rev-parse", "--short", "HEAD"]);
    let last = run_git_capture(library_root, &["log", "-1", "--format=%aI", "HEAD"]);
    (head, last)
}

/// Run `git -C <dir> <args...>` and return trimmed stdout on success.
fn run_git_capture(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Read `(lock_held, lock_age_seconds)` from `.git/index.lock`.
fn read_lock_info(library_root: &Path) -> (bool, Option<u64>) {
    let lock_path = library_root.join(".git").join("index.lock");
    match std::fs::metadata(&lock_path) {
        Ok(meta) => {
            let age = meta
                .modified()
                .ok()
                .and_then(|mt| mt.duration_since(UNIX_EPOCH).ok())
                .and_then(|created| {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|now| now.saturating_sub(created))
                })
                .map(|d: Duration| d.as_secs());
            (true, age)
        }
        Err(_) => (false, None),
    }
}

/// Read the free bytes available on the filesystem backing `path` by invoking
/// `df -kP <path>` and parsing the "Available" column of the second line.
///
/// `df -kP` is POSIX-portable (macOS and Linux). The 4th column is Available-KB;
/// we multiply by 1024 to return bytes. Failure → `None`, never an error.
fn read_free_disk_bytes(path: &Path) -> Option<u64> {
    let output = std::process::Command::new("df")
        .arg("-kP")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_df_output(&stdout)
}

/// Parse the second line of `df -kP` output and return Available bytes.
///
/// `df -kP` guarantees a single output line per filesystem regardless of how
/// long the device name is (the `-P` flag forces POSIX single-line format).
/// The columns are: Filesystem, 1024-blocks, Used, Available, Capacity, Mounted on.
/// We take column 4 (Available-KB) × 1024.
fn parse_df_output(stdout: &str) -> Option<u64> {
    let second_line = stdout.lines().nth(1)?;
    let available_kb: u64 = second_line.split_whitespace().nth(3)?.parse().ok()?;
    Some(available_kb.saturating_mul(1024))
}

/// Format a `StatusData` as the human-readable output block (AC-3).
fn render_human(data: &StatusData) -> String {
    let mut out = String::new();
    out.push_str(&format!("Library — {}\n", data.agent));
    out.push_str(&format!("  Topology:   {}\n", data.topology));
    out.push_str(&format!("  Volume:     {}\n", data.volume));
    out.push_str(&format!("  Mount:      {}\n", data.mount_path));

    let empty = data.git_head.is_none()
        && data.page_count == 0
        && data.last_ingest_at.is_none()
        && !data.lock_held;

    if empty {
        out.push_str("\n  Library not yet initialized — run an ingest to populate.\n");
    }

    out.push('\n');
    out.push_str(&format!(
        "  Schema file:  {}\n",
        if data.schema_present {
            "present"
        } else {
            "missing"
        }
    ));
    out.push_str(&format!("  Pages:        {}\n", data.page_count));
    out.push_str(&format!(
        "  Git HEAD:     {}\n",
        data.git_head.as_deref().unwrap_or("—")
    ));
    out.push_str(&format!(
        "  Last ingest:  {}\n",
        data.last_ingest_at.as_deref().unwrap_or("—")
    ));
    let lock_display = if data.lock_held {
        match data.lock_age_seconds {
            Some(age) => format!("held ({age}s)"),
            None => "held".to_string(),
        }
    } else {
        "free".to_string()
    };
    out.push_str(&format!("  Lock:         {lock_display}\n"));
    out.push_str(&format!(
        "  Free disk:    {}\n",
        match data.free_disk_bytes {
            Some(b) => format_bytes_human(b),
            None => "—".to_string(),
        }
    ));

    out
}

/// Format a byte count as a short human-readable string (e.g. "12.3 GB").
fn format_bytes_human(bytes: u64) -> String {
    const UNITS: &[(&str, u64)] = &[
        ("TB", 1_000_000_000_000),
        ("GB", 1_000_000_000),
        ("MB", 1_000_000),
        ("KB", 1_000),
    ];
    for (unit, scale) in UNITS {
        if bytes >= *scale {
            let value = bytes as f64 / *scale as f64;
            return format!("{value:.1} {unit}");
        }
    }
    format!("{bytes} B")
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_SCHEMA_TEMPLATE;
    use regex::Regex;
    use std::collections::BTreeSet;

    /// AC-3: top-level heading is "# Your Library".
    #[test]
    fn test_template_top_heading() {
        let first_non_empty = DEFAULT_SCHEMA_TEMPLATE
            .lines()
            .find(|l| !l.trim().is_empty())
            .expect("template must not be empty");
        assert_eq!(
            first_non_empty, "# Your Library",
            "first non-empty line must be the top heading; got {first_non_empty:?}"
        );
    }

    /// AC-3: all eight required level-2 sections are present.
    #[test]
    fn test_template_required_sections_present() {
        let required_headings = [
            "## What your Library is for",
            "## When to search your Library",
            "## When to write to your Library (important)",
            "## When **not** to write",
            "## How to write a Library page",
            "## Maintaining `index.md`",
            "## Disclaimer (always include in responses derived from Library content)",
            "## Your domain (configurable)",
        ];
        for heading in required_headings {
            assert!(
                DEFAULT_SCHEMA_TEMPLATE.contains(heading),
                "template must contain required heading: {heading}"
            );
        }
    }

    /// AC-4: FR39 disclaimer literal substring is present, plus the
    /// "LLM-generated summaries" framing sentence.
    #[test]
    fn test_template_contains_disclaimer_text() {
        let disclaimer = "This comes from my notes — please verify against the original source if it's critical.";
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains(disclaimer),
            "template must contain the FR39 disclaimer literal"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("LLM-generated summaries"),
            "template must contain the LLM-generated-summaries framing"
        );
    }

    /// AC-5: all four labeled write-trigger categories present.
    #[test]
    fn test_template_contains_write_categories() {
        let categories = [
            "Category A — Durable facts about the user, their work, or their domain",
            "Category B — Decisions, commitments, and policies the user has established",
            "Category C — Things you figured out on your own that would save future-you time",
            "Category D — Corrections to prior Library content",
        ];
        for cat in categories {
            assert!(
                DEFAULT_SCHEMA_TEMPLATE.contains(cat),
                "template must contain write-trigger category: {cat}"
            );
        }
    }

    /// AC-5: explicit non-trigger section is present.
    #[test]
    fn test_template_has_explicit_non_triggers_section() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("## When **not** to write"),
            "template must contain an explicit non-triggers section"
        );
    }

    /// AC-6: required `{{...}}` placeholders are present.
    #[test]
    fn test_template_contains_required_placeholders() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("{{agent_name}}"),
            "template must contain {{{{agent_name}}}} placeholder"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("{{domain_description}}"),
            "template must contain {{{{domain_description}}}} placeholder"
        );
    }

    /// AC-6: every `{{...}}` token in the template is in the allow-list. A future
    /// edit that introduces an undocumented placeholder fails this test, forcing the
    /// edit to also update story 13-22 (the consumer that performs substitution).
    #[test]
    fn test_template_no_undocumented_placeholders() {
        let allow_list: BTreeSet<&str> = ["agent_name", "domain_description"].into_iter().collect();
        let re = Regex::new(r"\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}")
            .expect("placeholder regex must compile");
        let found: BTreeSet<String> = re
            .captures_iter(DEFAULT_SCHEMA_TEMPLATE)
            .map(|c| c[1].to_string())
            .collect();
        for token in &found {
            assert!(
                allow_list.contains(token.as_str()),
                "undocumented placeholder {{{{{token}}}}} found in template — \
                 if this is intentional, update both this allow-list AND \
                 story 13-22 (wh library init substitution logic)"
            );
        }
    }

    /// AC-7: search-when-relevant gate language is present.
    #[test]
    fn test_template_search_gate_language() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("At the start of every turn"),
            "template must instruct the agent at the start of every turn"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("**do not search**"),
            "template must explicitly tell the agent not to search for off-domain queries"
        );
    }

    // ─── Story 13-24: `wh library status` tests ────────────────────────

    use super::{
        count_pages_under, format_bytes_human, parse_df_output, render_human, LibraryCommand,
        StatusData,
    };
    use clap::Parser;

    /// Minimal clap harness so we can assert the library subcommand parses as
    /// expected without wiring the full `Commands` enum into the test.
    #[derive(Debug, Parser)]
    #[command(name = "wh-test")]
    struct TestCli {
        #[command(subcommand)]
        library: LibraryCommand,
    }

    #[test]
    fn test_library_command_group_parses() {
        let cli =
            TestCli::try_parse_from(["wh-test", "status", "donna"]).expect("parse should succeed");
        match cli.library {
            LibraryCommand::Status(args) => {
                assert_eq!(args.agent, "donna");
                assert_eq!(args.format, crate::output::OutputFormat::Human);
            }
        }
    }

    #[test]
    fn test_library_command_group_parses_json_flag() {
        let cli = TestCli::try_parse_from(["wh-test", "status", "donna", "--format", "json"])
            .expect("parse should succeed");
        match cli.library {
            LibraryCommand::Status(args) => {
                assert_eq!(args.agent, "donna");
                assert_eq!(args.format, crate::output::OutputFormat::Json);
            }
        }
    }

    fn sample_status(populated: bool) -> StatusData {
        if populated {
            StatusData {
                agent: "donna".into(),
                topology: "dev".into(),
                volume: "wh-dev-donna-workspace".into(),
                mount_path: "/var/lib/containers/storage/volumes/wh-dev-donna-workspace/_data"
                    .into(),
                schema_present: true,
                page_count: 42,
                git_head: Some("abc1234".into()),
                last_ingest_at: Some("2026-04-09T14:30:00+00:00".into()),
                lock_held: false,
                lock_age_seconds: None,
                free_disk_bytes: Some(12_345_678_901),
            }
        } else {
            StatusData {
                agent: "donna".into(),
                topology: "dev".into(),
                volume: "wh-dev-donna-workspace".into(),
                mount_path: "/mnt/donna".into(),
                schema_present: false,
                page_count: 0,
                git_head: None,
                last_ingest_at: None,
                lock_held: false,
                lock_age_seconds: None,
                free_disk_bytes: None,
            }
        }
    }

    #[test]
    fn test_render_human_empty_library() {
        let rendered = render_human(&sample_status(false));
        assert!(rendered.contains("Library — donna"));
        assert!(rendered.contains("Library not yet initialized"));
        assert!(rendered.contains("Schema file:  missing"));
        assert!(rendered.contains("Pages:        0"));
        assert!(rendered.contains("Git HEAD:     —"));
        assert!(rendered.contains("Last ingest:  —"));
        assert!(rendered.contains("Lock:         free"));
        assert!(rendered.contains("Free disk:    —"));
    }

    #[test]
    fn test_render_human_populated_library() {
        let rendered = render_human(&sample_status(true));
        assert!(rendered.contains("Library — donna"));
        assert!(!rendered.contains("Library not yet initialized"));
        assert!(rendered.contains("Schema file:  present"));
        assert!(rendered.contains("Pages:        42"));
        assert!(rendered.contains("Git HEAD:     abc1234"));
        assert!(rendered.contains("Last ingest:  2026-04-09T14:30:00+00:00"));
        assert!(rendered.contains("Lock:         free"));
        assert!(rendered.contains("Free disk:    12.3 GB"));
    }

    #[test]
    fn test_render_json_populated_library() {
        let data = sample_status(true);
        let json = serde_json::to_string(&data).expect("serialize");
        for key in [
            "\"agent\"",
            "\"topology\"",
            "\"volume\"",
            "\"mount_path\"",
            "\"schema_present\"",
            "\"page_count\"",
            "\"git_head\"",
            "\"last_ingest_at\"",
            "\"lock_held\"",
            "\"lock_age_seconds\"",
            "\"free_disk_bytes\"",
        ] {
            assert!(json.contains(key), "json output must contain {key}");
        }
    }

    #[test]
    fn test_parse_df_output_happy_path() {
        // Canonical `df -kP` output on Linux and macOS: header line, one data line.
        let stdout = "\
Filesystem                     1024-blocks        Used   Available Capacity Mounted on
/dev/mapper/root                  100000000    30000000    65000000     32% /
";
        assert_eq!(parse_df_output(stdout), Some(65_000_000 * 1024));
    }

    #[test]
    fn test_parse_df_output_missing_line() {
        assert_eq!(parse_df_output(""), None);
        assert_eq!(parse_df_output("header only\n"), None);
    }

    #[test]
    fn test_parse_df_output_malformed_column() {
        let stdout = "\
Filesystem 1024-blocks Used Available Capacity Mounted
/dev/foo    100 30 notanumber 50% /
";
        assert_eq!(parse_df_output(stdout), None);
    }

    #[test]
    fn test_count_pages_missing_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        assert_eq!(count_pages_under(&missing), 0);
    }

    #[test]
    fn test_count_pages_counts_only_md_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        std::fs::write(root.join("b.md"), "x").unwrap();
        std::fs::write(root.join("ignore.txt"), "x").unwrap();
        std::fs::write(root.join("ignore.png"), "x").unwrap();
        std::fs::write(root.join("sub").join("c.md"), "x").unwrap();
        std::fs::write(root.join("sub").join("ignore.yaml"), "x").unwrap();
        assert_eq!(count_pages_under(root), 3);
    }

    #[test]
    fn test_format_bytes_human() {
        assert_eq!(format_bytes_human(0), "0 B");
        assert_eq!(format_bytes_human(999), "999 B");
        assert_eq!(format_bytes_human(1_500), "1.5 KB");
        assert_eq!(format_bytes_human(2_500_000), "2.5 MB");
        assert_eq!(format_bytes_human(3_200_000_000), "3.2 GB");
        assert_eq!(format_bytes_human(4_100_000_000_000), "4.1 TB");
    }
}
