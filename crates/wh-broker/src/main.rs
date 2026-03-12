//! Wheelhouse broker binary entry point.
//!
//! Minimal -- delegates to `wh_broker::run_broker()`.
//! This is the only file in the broker crate that may use `anyhow` (SCV-04).

use wh_broker::config::BrokerConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = BrokerConfig::from_env();
    wh_broker::run_broker(config).await?;
    Ok(())
}
