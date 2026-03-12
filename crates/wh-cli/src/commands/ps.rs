//! `wh ps` — Unified Component Inspection command.
//!
//! Lists all deployed agents, streams, and surfaces with their live status.

use clap::Args;
use serde::Serialize;

use crate::client::ControlClient;
use crate::output::error::WhError;
use crate::output::json;
use crate::output::table::{self, Table};
use crate::output::OutputFormat;

/// Arguments for `wh ps`.
#[derive(Debug, Args)]
pub struct PsArgs {
    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,
}

/// Kind of component in the topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    Agent,
    Stream,
    Surface,
}

impl std::fmt::Display for ComponentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentKind::Agent => write!(f, "agent"),
            ComponentKind::Stream => write!(f, "stream"),
            ComponentKind::Surface => write!(f, "surface"),
        }
    }
}

/// Machine-readable status enum: running | stopped | degraded | unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Running,
    Stopped,
    Degraded,
    Unknown,
}

impl std::fmt::Display for ComponentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentStatus::Running => write!(f, "running"),
            ComponentStatus::Stopped => write!(f, "stopped"),
            ComponentStatus::Degraded => write!(f, "degraded"),
            ComponentStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// A single component in the topology.
///
/// This is a CLI-local display model parsed from the broker's JSON response.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentInfo {
    pub name: String,
    pub kind: ComponentKind,
    pub status: ComponentStatus,
    pub stream: String,
    pub provider: String,
    pub uptime: String,
}

/// Data payload for `wh ps` JSON output.
#[derive(Debug, Serialize)]
pub struct PsData {
    pub components: Vec<ComponentInfo>,
    pub summary: PsSummary,
}

/// Summary counters for the ps output.
#[derive(Debug, Serialize)]
pub struct PsSummary {
    pub total_agents: usize,
    pub running: usize,
    pub stopped: usize,
}

/// Execute `wh ps`.
pub async fn execute(args: &PsArgs) -> Result<(), WhError> {
    // Attempt to connect to the control socket
    let client = ControlClient::new();

    // Send ps command and parse response
    let response = client.send_command("ps").await?;

    // Parse components from response
    let components = parse_components(&response)?;

    match args.format {
        OutputFormat::Human => render_human(&components),
        OutputFormat::Json => render_json(&components)?,
    }

    Ok(())
}

/// Parse components from the broker's JSON response.
fn parse_components(response: &serde_json::Value) -> Result<Vec<ComponentInfo>, WhError> {
    let data = response
        .get("data")
        .and_then(|d| d.get("components"))
        .and_then(|c| c.as_array())
        .ok_or_else(|| WhError::Internal("Invalid response format".to_string()))?;

    let mut components = Vec::new();
    for item in data {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let kind = match item.get("kind").and_then(|v| v.as_str()) {
            Some("agent") => ComponentKind::Agent,
            Some("stream") => ComponentKind::Stream,
            Some("surface") => ComponentKind::Surface,
            _ => ComponentKind::Agent,
        };
        let status = match item.get("status").and_then(|v| v.as_str()) {
            Some("running") => ComponentStatus::Running,
            Some("stopped") => ComponentStatus::Stopped,
            Some("degraded") => ComponentStatus::Degraded,
            _ => ComponentStatus::Unknown,
        };
        let stream = item
            .get("stream")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_string();
        let provider = item
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_string();
        let uptime = item
            .get("uptime")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_string();

        components.push(ComponentInfo {
            name,
            kind,
            status,
            stream,
            provider,
            uptime,
        });
    }

    Ok(components)
}

/// Render human-readable table output.
fn render_human(components: &[ComponentInfo]) {
    let use_color = table::should_use_color();

    let mut tbl = Table::new(vec![
        "NAME".into(),
        "STATUS".into(),
        "STREAM".into(),
        "PROVIDER".into(),
        "UPTIME".into(),
    ]);

    for comp in components {
        let status_display = format_status_human(comp.status, use_color);
        tbl.add_row(vec![
            comp.name.clone(),
            status_display,
            comp.stream.clone(),
            comp.provider.clone(),
            comp.uptime.clone(),
        ]);
    }

    print!("{}", tbl.render());

    // Summary line
    let summary = compute_summary(components);
    println!(
        "{} agents \u{00B7} {} running \u{00B7} {} stopped",
        summary.total_agents, summary.running, summary.stopped
    );
}

/// Format a component status for human output with color and prefix.
///
/// Running = green, stopped = red with `!` prefix (colorblind-accessible dual encoding).
fn format_status_human(status: ComponentStatus, use_color: bool) -> String {
    match status {
        ComponentStatus::Running => {
            if use_color {
                format!(
                    "{}{}{}",
                    table::ansi::GREEN,
                    "running",
                    table::ansi::RESET
                )
            } else {
                "running".to_string()
            }
        }
        ComponentStatus::Stopped => {
            if use_color {
                format!(
                    "{}{}!stopped{}",
                    table::ansi::RED,
                    table::ansi::BOLD,
                    table::ansi::RESET
                )
            } else {
                "!stopped".to_string()
            }
        }
        ComponentStatus::Degraded => {
            if use_color {
                format!(
                    "{}degraded{}",
                    table::ansi::DIM,
                    table::ansi::RESET
                )
            } else {
                "degraded".to_string()
            }
        }
        ComponentStatus::Unknown => "unknown".to_string(),
    }
}

/// Render JSON output.
fn render_json(components: &[ComponentInfo]) -> Result<(), WhError> {
    let summary = compute_summary(components);
    let data = PsData {
        components: components.to_vec(),
        summary,
    };
    json::print_json_success(&data)
}

/// Compute summary counters from components.
fn compute_summary(components: &[ComponentInfo]) -> PsSummary {
    let agents: Vec<&ComponentInfo> = components
        .iter()
        .filter(|c| c.kind == ComponentKind::Agent)
        .collect();
    let running = agents
        .iter()
        .filter(|c| c.status == ComponentStatus::Running)
        .count();
    let stopped = agents
        .iter()
        .filter(|c| c.status == ComponentStatus::Stopped)
        .count();

    PsSummary {
        total_agents: agents.len(),
        running,
        stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_status_human_no_color() {
        assert_eq!(
            format_status_human(ComponentStatus::Running, false),
            "running"
        );
        assert_eq!(
            format_status_human(ComponentStatus::Stopped, false),
            "!stopped"
        );
        assert_eq!(
            format_status_human(ComponentStatus::Degraded, false),
            "degraded"
        );
        assert_eq!(
            format_status_human(ComponentStatus::Unknown, false),
            "unknown"
        );
    }

    #[test]
    fn test_format_status_human_with_color() {
        let running = format_status_human(ComponentStatus::Running, true);
        assert!(running.contains("\x1b[32m")); // GREEN
        assert!(running.contains("running"));

        let stopped = format_status_human(ComponentStatus::Stopped, true);
        assert!(stopped.contains("\x1b[31m")); // RED
        assert!(stopped.contains("!stopped"));
    }

    #[test]
    fn test_compute_summary() {
        let components = vec![
            ComponentInfo {
                name: "a1".into(),
                kind: ComponentKind::Agent,
                status: ComponentStatus::Running,
                stream: "main".into(),
                provider: "podman".into(),
                uptime: "1h".into(),
            },
            ComponentInfo {
                name: "a2".into(),
                kind: ComponentKind::Agent,
                status: ComponentStatus::Stopped,
                stream: "main".into(),
                provider: "podman".into(),
                uptime: "-".into(),
            },
            ComponentInfo {
                name: "s1".into(),
                kind: ComponentKind::Stream,
                status: ComponentStatus::Running,
                stream: "-".into(),
                provider: "-".into(),
                uptime: "2h".into(),
            },
        ];
        let summary = compute_summary(&components);
        assert_eq!(summary.total_agents, 2);
        assert_eq!(summary.running, 1);
        assert_eq!(summary.stopped, 1);
    }

    #[test]
    fn test_stopped_has_bang_prefix() {
        let display = format_status_human(ComponentStatus::Stopped, false);
        assert!(
            display.starts_with('!'),
            "Stopped status must have '!' prefix for colorblind accessibility"
        );
    }

    #[test]
    fn test_component_status_serialization() {
        let status = ComponentStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let status = ComponentStatus::Stopped;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"stopped\"");

        let status = ComponentStatus::Degraded;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"degraded\"");

        let status = ComponentStatus::Unknown;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"unknown\"");
    }

    #[test]
    fn test_component_kind_serialization() {
        let kind = ComponentKind::Agent;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"agent\"");
    }
}
