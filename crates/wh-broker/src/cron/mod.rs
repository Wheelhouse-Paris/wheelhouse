//! Cron module — cron job scheduling and event publishing.
//!
//! Established in Story 5-5. This is the minimal prerequisite subset
//! needed for the monitor module to compile in this worktree.

use std::collections::HashMap;

/// Internal message representing a fired cron event.
/// Sent via `mpsc` channel from the scheduler to the broker's main loop.
#[derive(Debug, Clone)]
pub struct CronEventMessage {
    pub job_name: String,
    pub action: String,
    pub schedule: String,
    pub payload: HashMap<String, String>,
}
