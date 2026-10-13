use std::collections::BTreeMap;

use serde::Serialize;
use tracing::debug;

use crate::cloudflare::{CloudflareClient, PurgeTarget};
use crate::config::{Config, PurgeStrategy};
use crate::diff::{hash_matches, Drift, DriftKind};
use crate::edge::{self, EdgeProbe};
use crate::error::{Error, Result};
use crate::manifest;
use crate::state::State;

#[derive(Debug, Clone, Copy)]
pub struct SyncOptions {
    /// Report only, never call the purge API.
    pub dry_run: bool,
    /// Purge drifted URLs (false = scan mode).
    pub purge: bool,
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub mode: String,
    pub dry_run: bool,
    pub assets_scanned: usize,
    pub edge_probed: usize,
    pub matched: usize,
    pub skipped: Vec<String>,
    pub drift: Vec<Drift>,
    pub purge_urls: Vec<String>,
    pub purge_batches: usize,
    pub purged: bool,
    pub state_written: bool,
    pub errors: Vec<String>,
}

impl SyncReport {
    pub fn has_purgeable_drift(&self) -> bool {
        self.drift.iter().any(|d| d.is_purgeable())
    }
}

pub async fn run(cfg: &Config, opts: &SyncOptions) -> Result<SyncReport> {
    if cfg.assets.base_url.is_empty() {
        return Err(Error::Config("assets.base_url is empty".into()));
    }
    if !cfg.assets.has_sources() {
        return Err(Error::Config(
            "no asset sources: set assets.dir, assets.manifest or assets.files".into(),
        ));
    }
    let algo = cfg.assets.hash_algorithm;
    let (assets, skipped) = manifest::collect(&cfg.assets, algo)?;
    let previous = State::load(&cfg.state.path)?.assets;
    let client = edge::http_client(cfg)?;
    let urls: Vec<String> = assets.keys().cloned().collect();
    let probes: BTreeMap<String, Result<EdgeProbe>> = edge::probe_all(&client, urls, cfg)
        .await
        .into_iter()
        .collect();

    let mut drift = Vec::new();
    let mut matched = 0usize;
    for (url, local) in &assets {
        let Some(probe) = probes.get(url) else {
            continue;
        };
        match probe {
            Err(err) => drift.push(Drift {
                url: url.clone(),
                kind: DriftKind::ProbeError,
                local_hash: Some(local.hash.clone()),
                edge_hash: None,
                detail: err.to_string(),
            }),
            Ok(probe) if probe.status >= 500 => drift.push(Drift {
                url: url.clone(),
                kind: DriftKind::ProbeError,
                local_hash: Some(local.hash.clone()),
                edge_hash: None,
                detail: format!("edge status {}", probe.status),
            }),
            Ok(probe) if probe.status == 404 || probe.status == 410 => drift.push(Drift {
                url: url.clone(),
                kind: DriftKind::Changed,
                local_hash: Some(local.hash.clone()),
                edge_hash: None,
                detail: format!("edge status {} (stale negative cache)", probe.status),
            }),
            Ok(probe) => match probe.hash.as_deref() {
                None => drift.push(Drift {
                    url: url.clone(),
                    kind: DriftKind::Unverifiable,
                    local_hash: Some(local.hash.clone()),
                    edge_hash: probe.etag.clone(),
                    detail: "edge hash source missing".to_string(),
                }),
                Some(edge_hash) => match hash_matches(local, edge_hash, cfg.edge.verify) {
                    None => drift.push(Drift {
                        url: url.clone(),
                        kind: DriftKind::Unverifiable,
                        local_hash: Some(local.hash.clone()),
                        edge_hash: Some(edge_hash.to_string()),
                        detail: "embedded filename hash not comparable in this verify mode"
                            .to_string(),
                    }),
                    Some(true) => {
                        matched += 1;
                        debug!(url, cache = ?probe.cache_status, "edge in sync");
                    }
                    Some(false) => drift.push(Drift {
                        url: url.clone(),
                        kind: DriftKind::Changed,
                        local_hash: Some(local.hash.clone()),
                        edge_hash: Some(edge_hash.to_string()),
                        detail: "edge hash differs from local build hash".to_string(),
                    }),
                },
            },
        }
    }

    for (url, old_hash) in &previous {
        if !assets.contains_key(url) {
            drift.push(Drift {
                url: url.clone(),
                kind: DriftKind::Orphaned,
                local_hash: None,
                edge_hash: Some(old_hash.clone()),
                detail: "present in previous state, gone from build".to_string(),
            });
        }
    }

    let purge_urls: Vec<String> = drift
        .iter()
        .filter(|d| d.is_purgeable())
        .map(|d| d.url.clone())
        .collect();

    let mut report = SyncReport {
        mode: if opts.purge { "sync" } else { "scan" }.to_string(),
        dry_run: opts.dry_run,
        assets_scanned: assets.len(),
        edge_probed: probes.len(),
        matched,
        skipped,
        drift,
        purge_urls,
        purge_batches: 0,
        purged: false,
        state_written: false,
        errors: Vec::new(),
    };

    if opts.purge && !opts.dry_run && !report.purge_urls.is_empty() {
        let cf = CloudflareClient::from_config(cfg)?;
        let target = match cfg.purge.strategy {
            PurgeStrategy::Url => PurgeTarget::Urls(report.purge_urls.clone()),
            PurgeStrategy::Tag => PurgeTarget::Tags(cfg.purge.tags.clone()),
            PurgeStrategy::Everything => PurgeTarget::Everything,
        };
        match cf.purge(&target, &cfg.purge).await {
            Ok(batches) => {
                report.purge_batches = batches;
                report.purged = true;
            }
            Err(err) => report.errors.push(err.to_string()),
        }
    }

    if opts.purge && !opts.dry_run && report.errors.is_empty() {
        let mut snapshot = BTreeMap::new();
        for (url, local) in &assets {
            snapshot.insert(url.clone(), local.hash.clone());
        }
        State {
            version: 1,
            updated_at: crate::state::now_iso(),
            assets: snapshot,
        }
        .save(&cfg.state.path)?;
        report.state_written = true;
    }

    Ok(report)
}

pub fn render_human(report: &SyncReport) -> String {
    let mut out = format!(
        "mode={} dry_run={} scanned={} probed={} matched={} drift={} purged={} batches={} state_written={}\n",
        report.mode,
        report.dry_run,
        report.assets_scanned,
        report.edge_probed,
        report.matched,
        report.drift.len(),
        report.purged,
        report.purge_batches,
        report.state_written
    );
    for drift in &report.drift {
        out.push_str(&format!(
            "drift {} {} [{}] {}\n",
            drift.kind,
            drift.url,
            drift
                .local_hash
                .clone()
                .or(drift.edge_hash.clone())
                .unwrap_or_default(),
            drift.detail
        ));
    }
    for skipped in &report.skipped {
        out.push_str(&format!("skipped {skipped}\n"));
    }
    for err in &report.errors {
        out.push_str(&format!("error {err}\n"));
    }
    out
}
