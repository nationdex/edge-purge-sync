use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;

use edge_purge_sync::config::Config;
use edge_purge_sync::sync::{self, SyncOptions};

#[derive(Clone)]
struct Mock {
    origin_body: Arc<Mutex<String>>,
    purges: Arc<Mutex<Vec<Value>>>,
}

impl Mock {
    fn new(origin_body: &str) -> Self {
        Self {
            origin_body: Arc::new(Mutex::new(origin_body.to_string())),
            purges: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

async fn origin(State(mock): State<Mock>) -> ([(&'static str, &'static str); 2], String) {
    let body = mock.origin_body.lock().unwrap().clone();
    (
        [("etag", "\"stale-etag\""), ("cf-cache-status", "HIT")],
        body,
    )
}

async fn purge_cache(State(mock): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    mock.purges.lock().unwrap().push(body);
    Json(serde_json::json!({
        "success": true,
        "errors": [],
        "messages": [],
        "result": { "id": "purge-id" }
    }))
}

async fn spawn_mock(mock: Mock) -> SocketAddr {
    let app = Router::new()
        .route("/assets/app.deadbeef.js", get(origin))
        .route("/zones/test-zone/purge_cache", post(purge_cache))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    addr
}

fn write_config(addr: &SocketAddr, dist: &Path, state: &Path) -> Config {
    let raw = format!(
        r#"
        [cloudflare]
        base_url = "http://{addr}"
        zone_id = "test-zone"
        api_token = "test-token"

        [assets]
        base_url = "http://{addr}"
        dir = '{dist}'
        url_prefix = "/assets/"

        [state]
        path = '{state}'
        "#,
        addr = addr,
        dist = dist.display(),
        state = state.display()
    );
    let mut cfg: Config = toml::from_str(&raw).expect("config parses");
    cfg.edge.verify = edge_purge_sync::config::VerifyMode::Body;
    cfg.edge.concurrency = 4;
    cfg.edge.timeout_secs = 5;
    cfg.purge.batch_size = 1;
    cfg.purge.max_retries = 0;
    cfg
}

fn setup(tmp: &Path, previous: Option<Value>) -> std::path::PathBuf {
    let dist = tmp.join("dist");
    std::fs::create_dir_all(&dist).expect("mkdir");
    std::fs::write(dist.join("app.deadbeef.js"), "new content").expect("write asset");
    let state = tmp.join("state.json");
    if let Some(value) = previous {
        std::fs::write(&state, value.to_string()).expect("write state");
    }
    state
}

#[tokio::test]
async fn sync_purges_changed_and_orphaned_urls() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mock = Mock::new("stale content");
    let addr = spawn_mock(mock.clone()).await;
    let base = format!("http://{addr}");
    let orphan_url = format!("{base}/assets/old.gone9999.js");
    let asset_url = format!("{base}/assets/app.deadbeef.js");
    let state_path = setup(
        tmp.path(),
        Some(serde_json::json!({
            "version": 1,
            "updated_at": "2026-01-01T00:00:00Z",
            "assets": { orphan_url.clone(): "0000" }
        })),
    );
    let cfg = write_config(&addr, &tmp.path().join("dist"), &state_path);

    let report = sync::run(
        &cfg,
        &SyncOptions {
            dry_run: false,
            purge: true,
        },
    )
    .await
    .expect("sync");

    assert_eq!(report.assets_scanned, 1);
    assert_eq!(report.matched, 0);
    assert!(report.purged);
    assert_eq!(report.purge_batches, 2);
    assert!(report
        .drift
        .iter()
        .any(|d| d.url == asset_url && d.kind.to_string() == "changed"));
    assert!(report
        .drift
        .iter()
        .any(|d| d.url == orphan_url && d.kind.to_string() == "orphaned"));

    let purges = mock.purges.lock().unwrap().clone();
    assert_eq!(purges.len(), 2);
    let mut purged: Vec<String> = purges
        .iter()
        .flat_map(|p| p["files"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    purged.sort();
    assert_eq!(purged, vec![asset_url.clone(), orphan_url.clone()]);

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("read state"))
            .expect("state json");
    assert!(saved["assets"].get(asset_url.as_str()).is_some());
    assert!(saved["assets"].get(orphan_url.as_str()).is_none());
}

#[tokio::test]
async fn sync_in_sync_writes_state_without_purge() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mock = Mock::new("new content");
    let addr = spawn_mock(mock.clone()).await;
    let asset_url = format!("http://{addr}/assets/app.deadbeef.js");
    let state_path = setup(tmp.path(), None);
    let cfg = write_config(&addr, &tmp.path().join("dist"), &state_path);

    let report = sync::run(
        &cfg,
        &SyncOptions {
            dry_run: false,
            purge: true,
        },
    )
    .await
    .expect("sync");

    assert_eq!(report.matched, 1);
    assert!(report.drift.is_empty());
    assert!(!report.purged);
    assert!(mock.purges.lock().unwrap().is_empty());
    assert!(report.state_written);
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("read state"))
            .expect("state json");
    assert!(saved["assets"].get(asset_url.as_str()).is_some());
}

#[tokio::test]
async fn dry_run_reports_drift_without_side_effects() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mock = Mock::new("stale content");
    let addr = spawn_mock(mock.clone()).await;
    let orphan_url = format!("http://{addr}/assets/old.gone9999.js");
    let state_path = setup(
        tmp.path(),
        Some(serde_json::json!({
            "version": 1,
            "updated_at": "2026-01-01T00:00:00Z",
            "assets": { orphan_url.clone(): "0000" }
        })),
    );
    let cfg = write_config(&addr, &tmp.path().join("dist"), &state_path);

    let report = sync::run(
        &cfg,
        &SyncOptions {
            dry_run: true,
            purge: true,
        },
    )
    .await
    .expect("sync");

    assert!(report.has_purgeable_drift());
    assert!(!report.purged);
    assert!(!report.state_written);
    assert!(mock.purges.lock().unwrap().is_empty());
    assert!(!state_path.with_extension("tmp").exists());
}

#[tokio::test]
async fn scan_mode_never_purges() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mock = Mock::new("stale content");
    let addr = spawn_mock(mock.clone()).await;
    let state_path = setup(tmp.path(), None);
    let cfg = write_config(&addr, &tmp.path().join("dist"), &state_path);

    let report = sync::run(
        &cfg,
        &SyncOptions {
            dry_run: false,
            purge: false,
        },
    )
    .await
    .expect("sync");

    assert_eq!(report.mode, "scan");
    assert!(report.has_purgeable_drift());
    assert!(!report.purged);
    assert!(mock.purges.lock().unwrap().is_empty());
    assert!(!state_path.exists());
}
