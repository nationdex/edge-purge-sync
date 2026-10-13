use std::time::Duration;

use tracing::{error, info};

use crate::config::Config;
use crate::error::Result;
use crate::sync::{self, SyncOptions};

pub async fn run(cfg: &Config, interval_secs: Option<u64>, dry_run: bool) -> Result<()> {
    let interval = interval_secs.unwrap_or(cfg.daemon.interval_secs).max(1);
    let mut ticker = tokio::time::interval(Duration::from_secs(interval));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    info!(interval_secs = interval, "daemon start");
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("daemon shutdown");
                return Ok(());
            }
            _ = ticker.tick() => {
                match sync::run(cfg, &SyncOptions { dry_run, purge: true }).await {
                    Ok(report) => info!(
                        scanned = report.assets_scanned,
                        drift = report.drift.len(),
                        purged = report.purged,
                        batches = report.purge_batches,
                        "sync cycle done"
                    ),
                    Err(err) => error!(error = %err, "sync cycle failed"),
                }
            }
        }
    }
}
