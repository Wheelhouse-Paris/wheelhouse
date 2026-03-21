use std::process::Command;

fn main() {
    // Inject git commit hash at compile time (TT-06)
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=WH_GIT_HASH={git_hash}");

    // Inject target triple at compile time (TT-06)
    // TARGET is always set by cargo during builds; fallback for edge cases
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=WH_TARGET_TRIPLE={target}");

    // Re-run if HEAD changes. Use git rev-parse to find the repo root
    // so this works regardless of crate nesting depth or worktree layout.
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
    {
        if output.status.success() {
            if let Ok(git_dir) = String::from_utf8(output.stdout) {
                let git_dir = git_dir.trim();
                println!("cargo:rerun-if-changed={git_dir}/HEAD");
                println!("cargo:rerun-if-changed={git_dir}/refs/");
            }
        }
    }

    // Generate capabilities.json at build time (ADR-031, Story 12-2)
    generate_capabilities_manifest();
}

/// Generate the capabilities manifest JSON and write it to OUT_DIR.
///
/// The manifest lists all known Wheelhouse features by category with status.
/// It is embedded in the binary via `include_str!` so `wh capabilities` works
/// without any external file (E12-05: read-only at runtime).
fn generate_capabilities_manifest() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string());
    let generated = chrono_build_timestamp();

    let manifest = format!(
        r#"{{
  "version": "{version}",
  "generated": "{generated}",
  "categories": {{
    "streams": [
      {{"name": "create", "status": "available", "description": "Create named streams with typed messages"}},
      {{"name": "publish", "status": "available", "description": "Publish protobuf-typed messages to a stream"}},
      {{"name": "subscribe", "status": "available", "description": "Subscribe to a stream with auto-reconnect"}},
      {{"name": "tail", "status": "available", "description": "Observe stream messages in real time"}},
      {{"name": "compaction", "status": "available", "description": "Atomic daily summarization of stream WAL"}},
      {{"name": "context", "status": "available", "description": "Per-stream CONTEXT.md injection into agent prompts"}}
    ],
    "skills": [
      {{"name": "define", "status": "available", "description": "Define skills with YAML metadata and git storage"}},
      {{"name": "invoke", "status": "available", "description": "Agent invokes a skill via stream protocol"}},
      {{"name": "lazy-load", "status": "available", "description": "Skills loaded from git on first invocation"}},
      {{"name": "error-contract", "status": "available", "description": "SkillResult error contract — no silent timeouts"}}
    ],
    "surfaces": [
      {{"name": "cli", "status": "available", "description": "Terminal-based interactive surface"}},
      {{"name": "telegram", "status": "available", "description": "Telegram bot surface with multi-chat routing"}},
      {{"name": "restart", "status": "available", "description": "Restart a running surface by name"}},
      {{"name": "stop", "status": "available", "description": "Stop a running surface by name"}}
    ],
    "topology": [
      {{"name": "lint", "status": "available", "description": "Validate .wh topology file syntax and semantics"}},
      {{"name": "plan", "status": "available", "description": "Preview changes before applying a topology"}},
      {{"name": "apply", "status": "available", "description": "Provision agents, streams, and surfaces from .wh file"}},
      {{"name": "destroy", "status": "available", "description": "Tear down a deployed topology"}},
      {{"name": "folder-compose", "status": "available", "description": "Merge multiple .wh files from a folder into one topology"}}
    ],
    "cron": [
      {{"name": "declare", "status": "available", "description": "Declare cron jobs in .wh topology files"}},
      {{"name": "cronevent", "status": "available", "description": "CronEvent publishing on schedule"}},
      {{"name": "skill-trigger", "status": "available", "description": "Cron triggers skill invocation end-to-end"}}
    ],
    "cli": [
      {{"name": "ps", "status": "available", "description": "List deployed components with live status"}},
      {{"name": "logs", "status": "available", "description": "Tail structured logs from a specific agent"}},
      {{"name": "secrets", "status": "available", "description": "Manage credentials and secrets"}},
      {{"name": "status", "status": "available", "description": "Check broker health and uptime"}},
      {{"name": "doctor", "status": "available", "description": "Git repository health check"}},
      {{"name": "memory", "status": "available", "description": "Manage agent MEMORY.md files"}},
      {{"name": "completion", "status": "available", "description": "Generate shell completion scripts"}},
      {{"name": "capabilities", "status": "available", "description": "Inspect the capabilities manifest"}}
    ],
    "sdk": [
      {{"name": "python-core", "status": "available", "description": "Python SDK — connect, publish, subscribe"}},
      {{"name": "custom-types", "status": "available", "description": "Custom protobuf type registration"}},
      {{"name": "mock-mode", "status": "available", "description": "Development without Podman via mock broker"}},
      {{"name": "mcp-integration", "status": "experimental", "description": "MCP server integration for agent tools"}}
    ]
  }}
}}"#
    );

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = std::path::Path::new(&out_dir).join("capabilities.json");
    std::fs::write(&dest, &manifest).expect("Failed to write capabilities.json");

    // Re-run if the Cargo.toml version changes
    println!("cargo:rerun-if-changed=Cargo.toml");
}

/// Generate an ISO8601 build timestamp without pulling in chrono as a build dependency.
fn chrono_build_timestamp() -> String {
    // Use the SOURCE_DATE_EPOCH convention for reproducible builds if set,
    // otherwise fall back to a placeholder that gets replaced at format time.
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(secs) = epoch.parse::<u64>() {
            // Convert epoch seconds to a rough ISO8601 string
            let days_since_epoch = secs / 86400;
            let remaining = secs % 86400;
            let hours = remaining / 3600;
            let minutes = (remaining % 3600) / 60;
            let seconds = remaining % 60;

            // Approximate date calculation (good enough for build metadata)
            let (year, month, day) = epoch_days_to_date(days_since_epoch);
            return format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z");
        }
    }

    // Default: use the current date/time via the `date` command
    Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

/// Convert days since Unix epoch to (year, month, day).
fn epoch_days_to_date(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
