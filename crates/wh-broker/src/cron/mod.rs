//! Cron module — embedded cron scheduler and event dispatch (ADR-012).
//!
//! Bootstrapped from Stories 5-5 (CronEventMessage, CronScheduler) and
//! 5-6 (CronEventHandler, CronEventDispatcher).

pub mod handler;
pub mod dispatcher;
pub mod chain;
pub mod proto_bridge;
pub mod skill_handler;
pub mod orchestrator;

use std::collections::HashMap;

/// Internal cron event message passed via mpsc channel.
/// Decoupled from the proto CronEvent — this is the runtime representation.
#[derive(Debug, Clone)]
pub struct CronEventMessage {
    pub job_name: String,
    pub action: String,
    pub schedule: String,
    pub payload: HashMap<String, String>,
}
