//! Cron job declaration, parsing, persistence, and scheduling.
//!
//! Implements ADR-012: Cron scheduler embedded in broker process.
//! Uses `cron` crate for expression parsing + `tokio::time::interval` for tick loop.
//! Missed fires are silently skipped in MVP.

pub mod dispatcher;
pub mod handler;

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Configuration for a single cron job, as declared in a `.wh` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronJobConfig {
    /// Name of the cron job (e.g. "daily-compaction")
    pub name: String,
    /// Cron schedule expression (standard 5-field or 7-field)
    pub schedule: String,
    /// Target stream name to publish CronEvent to
    pub target: String,
    /// Action type: "compact" or "event"
    pub action: String,
    /// Optional key-value payload
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<HashMap<String, String>>,
}

/// A published CronEvent with all fields needed for stream publishing.
#[derive(Debug, Clone)]
pub struct CronEventMessage {
    pub job_name: String,
    pub action: String,
    pub schedule: String,
    pub triggered_at: prost_types::Timestamp,
    pub payload: HashMap<String, String>,
    pub target_stream: String,
}

/// Errors that can occur during cron operations.
#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("invalid cron expression for job '{job_name}': {reason}")]
    InvalidSchedule { job_name: String, reason: String },

    #[error("cron job '{job_name}' targets nonexistent stream '{stream_name}'")]
    StreamNotFound {
        job_name: String,
        stream_name: String,
    },

    #[error("failed to parse .wh file: {0}")]
    ParseError(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("YAML serialization error: {0}")]
    YamlError(#[from] serde_yaml::Error),
}

/// Partial .wh file structure for extracting cron and streams sections.
#[derive(Debug, Deserialize)]
struct WhFilePartial {
    #[serde(default)]
    streams: Vec<WhStreamPartial>,
    #[serde(default)]
    cron: Vec<CronJobConfig>,
}

#[derive(Debug, Deserialize)]
struct WhStreamPartial {
    name: String,
}

/// Parse the cron section from a `.wh` file content string.
///
/// Validates:
/// - Cron expression syntax (via `cron` crate)
/// - Target stream exists in the topology
///
/// Returns the list of validated cron job configs.
pub fn parse_wh_cron_section(content: &str) -> Result<Vec<CronJobConfig>, CronError> {
    let wh_file: WhFilePartial =
        serde_yaml::from_str(content).map_err(|e| CronError::ParseError(e.to_string()))?;

    let stream_names: Vec<&str> = wh_file.streams.iter().map(|s| s.name.as_str()).collect();

    for job in &wh_file.cron {
        // Validate cron expression
        validate_cron_expression(&job.name, &job.schedule)?;

        // Validate target stream exists
        if !stream_names.contains(&job.target.as_str()) {
            return Err(CronError::StreamNotFound {
                job_name: job.name.clone(),
                stream_name: job.target.clone(),
            });
        }
    }

    Ok(wh_file.cron)
}

/// Validate a cron expression using the `cron` crate.
///
/// The `cron` crate expects 7-field expressions (sec min hour dom month dow year).
/// Standard 5-field expressions are normalized by prepending "0 " and appending " *".
fn validate_cron_expression(job_name: &str, expression: &str) -> Result<(), CronError> {
    let normalized = normalize_cron_expression(expression);
    cron::Schedule::from_str(&normalized).map_err(|e| CronError::InvalidSchedule {
        job_name: job_name.to_string(),
        reason: e.to_string(),
    })?;
    Ok(())
}

/// Normalize a cron expression to 7-field format for the `cron` crate.
///
/// The `cron` crate uses 7 fields: sec min hour dom month dow year.
/// - 5 fields (standard crontab): prepend "0 " (seconds=0) and append " *" (year=any)
/// - 6 fields (sec min hour dom month dow): append " *" (year=any)
/// - 7 fields: pass through as-is
fn normalize_cron_expression(expression: &str) -> String {
    let field_count = expression.split_whitespace().count();
    match field_count {
        5 => format!("0 {} *", expression),
        6 => format!("{} *", expression),
        7 => expression.to_string(),
        _ => expression.to_string(), // Let the parser produce the error
    }
}

/// Save cron job configurations to `cron/jobs.yaml` under the given root directory.
pub fn save_cron_config(jobs: &[CronJobConfig], root: &Path) -> Result<(), CronError> {
    let cron_dir = root.join("cron");
    std::fs::create_dir_all(&cron_dir)?;
    let jobs_path = cron_dir.join("jobs.yaml");
    let yaml = serde_yaml::to_string(jobs)?;
    std::fs::write(jobs_path, yaml)?;
    Ok(())
}

/// Load cron job configurations from `cron/jobs.yaml` under the given root directory.
pub fn load_cron_config(root: &Path) -> Result<Vec<CronJobConfig>, CronError> {
    let jobs_path = root.join("cron").join("jobs.yaml");
    let content = std::fs::read_to_string(jobs_path)?;
    let jobs: Vec<CronJobConfig> = serde_yaml::from_str(&content)?;
    Ok(jobs)
}

/// Embedded cron scheduler that fires jobs on schedule and publishes CronEvents.
///
/// Architecture: ADR-012 — embedded in broker, `cron` crate + `tokio::time::interval`.
/// Shutdown safety: CRF-01 / SC-06 — checks CancellationToken before each fire.
/// Missed fires: silently skipped in MVP (ADR-012).
pub struct CronScheduler {
    jobs: Vec<ScheduledJob>,
    cancellation: CancellationToken,
    event_tx: mpsc::Sender<CronEventMessage>,
}

struct ScheduledJob {
    config: CronJobConfig,
    schedule: cron::Schedule,
    last_fire: Option<chrono::DateTime<chrono::Utc>>,
}

impl CronScheduler {
    /// Create a new cron scheduler.
    ///
    /// # Arguments
    /// * `jobs` - Validated cron job configurations
    /// * `cancellation` - Token for clean shutdown (CRF-01)
    /// * `event_tx` - Channel for publishing CronEvent messages
    pub fn new(
        jobs: Vec<CronJobConfig>,
        cancellation: CancellationToken,
        event_tx: mpsc::Sender<CronEventMessage>,
    ) -> Self {
        let scheduled_jobs = jobs
            .into_iter()
            .filter_map(|config| {
                let normalized = normalize_cron_expression(&config.schedule);
                match cron::Schedule::from_str(&normalized) {
                    Ok(schedule) => Some(ScheduledJob {
                        config,
                        schedule,
                        last_fire: None,
                    }),
                    Err(e) => {
                        warn!(
                            job_name = %config.name,
                            error = %e,
                            "skipping cron job with invalid schedule"
                        );
                        None
                    }
                }
            })
            .collect();

        Self {
            jobs: scheduled_jobs,
            cancellation,
            event_tx,
        }
    }

    /// Run the scheduler loop until cancellation.
    ///
    /// Checks all jobs each second. Fires jobs whose next scheduled time
    /// has passed since the last check. Missed fires are silently skipped (ADR-012).
    pub async fn run(&mut self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

        loop {
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => {
                    info!("cron scheduler shutting down (cancellation token)");
                    return;
                }
                _ = interval.tick() => {
                    self.check_and_fire().await;
                }
            }
        }
    }

    async fn check_and_fire(&mut self) {
        let now = chrono::Utc::now();

        for job in &mut self.jobs {
            // Get the next scheduled time after the last fire (or epoch)
            let reference = job.last_fire.unwrap_or_else(|| {
                // On first check, use now minus 1 second so we don't immediately fire
                now - chrono::Duration::seconds(2)
            });

            // Check if there's a scheduled time between last_fire and now
            let should_fire = job
                .schedule
                .after(&reference)
                .take(1)
                .any(|next_time| next_time <= now);

            if should_fire {
                job.last_fire = Some(now);

                let seconds = now.timestamp();
                let nanos = now.timestamp_subsec_nanos() as i32;

                let event = CronEventMessage {
                    job_name: job.config.name.clone(),
                    action: job.config.action.clone(),
                    schedule: job.config.schedule.clone(),
                    triggered_at: prost_types::Timestamp { seconds, nanos },
                    payload: job.config.payload.clone().unwrap_or_default(),
                    target_stream: job.config.target.clone(),
                };

                // Publish via channel — failure is logged, not fatal (AC #3)
                if let Err(e) = self.event_tx.send(event).await {
                    error!(
                        job_name = %job.config.name,
                        target = %job.config.target,
                        error = %e,
                        "CRON_PUBLISH_FAILED: failed to publish CronEvent"
                    );
                    // Continue — do not crash or stop other jobs
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_cron_section() {
        let content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

cron:
  - name: daily-compaction
    schedule: "0 3 * * *"
    target: main
    action: compact
"#;

        let configs = parse_wh_cron_section(content).expect("should parse");
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "daily-compaction");
        assert_eq!(configs[0].schedule, "0 3 * * *");
        assert_eq!(configs[0].target, "main");
        assert_eq!(configs[0].action, "compact");
        assert!(configs[0].payload.is_none());
    }

    #[test]
    fn parse_cron_with_payload() {
        let content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

cron:
  - name: morning-briefing
    schedule: "0 8 * * 1-5"
    target: main
    action: event
    payload:
      type: briefing
"#;

        let configs = parse_wh_cron_section(content).expect("should parse");
        assert_eq!(configs.len(), 1);
        let payload = configs[0].payload.as_ref().expect("should have payload");
        assert_eq!(payload.get("type").unwrap(), "briefing");
    }

    #[test]
    fn parse_multiple_cron_jobs() {
        let content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

cron:
  - name: job-a
    schedule: "0 3 * * *"
    target: main
    action: compact
  - name: job-b
    schedule: "0 8 * * 1-5"
    target: main
    action: event
"#;

        let configs = parse_wh_cron_section(content).expect("should parse");
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name, "job-a");
        assert_eq!(configs[1].name, "job-b");
    }

    #[test]
    fn parse_no_cron_section() {
        let content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local
"#;

        let configs = parse_wh_cron_section(content).expect("should parse");
        assert!(configs.is_empty());
    }

    #[test]
    fn reject_invalid_cron_expression() {
        let content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

cron:
  - name: bad-job
    schedule: "not-a-cron-expression"
    target: main
    action: event
"#;

        let result = parse_wh_cron_section(content);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            CronError::InvalidSchedule { job_name, .. } => {
                assert_eq!(job_name, "bad-job");
            }
            _ => panic!("expected InvalidSchedule error, got: {err}"),
        }
    }

    #[test]
    fn reject_cron_targeting_nonexistent_stream() {
        let content = r#"
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

cron:
  - name: orphan-job
    schedule: "0 3 * * *"
    target: nonexistent-stream
    action: event
"#;

        let result = parse_wh_cron_section(content);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            CronError::StreamNotFound {
                job_name,
                stream_name,
            } => {
                assert_eq!(job_name, "orphan-job");
                assert_eq!(stream_name, "nonexistent-stream");
            }
            _ => panic!("expected StreamNotFound error, got: {err}"),
        }
    }

    #[test]
    fn normalize_5_field_cron() {
        // 5-field standard crontab -> prepend 0 (sec), append * (year)
        let result = normalize_cron_expression("0 3 * * *");
        assert_eq!(result, "0 0 3 * * * *");
    }

    #[test]
    fn normalize_6_field_cron() {
        // 6-field with seconds -> append * (year)
        let result = normalize_cron_expression("* * * * * *");
        assert_eq!(result, "* * * * * * *");
    }

    #[test]
    fn normalize_7_field_cron_passthrough() {
        let result = normalize_cron_expression("0 0 3 * * * *");
        assert_eq!(result, "0 0 3 * * * *");
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let jobs = vec![CronJobConfig {
            name: "test-job".to_string(),
            schedule: "0 3 * * *".to_string(),
            target: "main".to_string(),
            action: "compact".to_string(),
            payload: None,
        }];

        save_cron_config(&jobs, tmp.path()).expect("save");

        let jobs_path = tmp.path().join("cron").join("jobs.yaml");
        assert!(jobs_path.exists(), "cron/jobs.yaml should exist");

        let loaded = load_cron_config(tmp.path()).expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "test-job");
        assert_eq!(loaded[0].schedule, "0 3 * * *");
        assert_eq!(loaded[0].target, "main");
        assert_eq!(loaded[0].action, "compact");
    }

    #[test]
    fn save_creates_cron_directory() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let cron_dir = tmp.path().join("cron");
        assert!(!cron_dir.exists());

        save_cron_config(&[], tmp.path()).expect("save");
        assert!(cron_dir.exists(), "cron/ directory should be created");
    }

    #[tokio::test]
    async fn scheduler_shuts_down_on_cancellation() {
        let (tx, _rx) = mpsc::channel(10);
        let cancel = CancellationToken::new();

        let mut scheduler = CronScheduler::new(
            vec![CronJobConfig {
                name: "test".to_string(),
                schedule: "0 3 * * *".to_string(),
                target: "main".to_string(),
                action: "event".to_string(),
                payload: None,
            }],
            cancel.clone(),
            tx,
        );

        cancel.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), scheduler.run()).await;
        assert!(result.is_ok(), "scheduler should shut down within 2s");
    }

    #[tokio::test]
    async fn scheduler_survives_closed_channel() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // Close receiver

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        // Use a per-second schedule to trigger quickly
        let mut scheduler = CronScheduler::new(
            vec![CronJobConfig {
                name: "failing-job".to_string(),
                schedule: "* * * * *".to_string(),
                target: "main".to_string(),
                action: "event".to_string(),
                payload: None,
            }],
            cancel_clone,
            tx,
        );

        let handle = tokio::spawn(async move {
            scheduler.run().await;
        });

        // Let it run for 2 seconds — should not crash
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert!(!handle.is_finished(), "scheduler should still be running");

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
    }
}
