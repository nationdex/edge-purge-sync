# edge-purge-sync

Autonomous Cloudflare / CDN Cache Synchronizer

Diffs built asset hashes against Cloudflare edge headers, purges only what went stale. Kills manual purge after Docker deploys and asset hash changes, keeps Pingdom scores flat.

## How it works

1. Builds `url -> hash` map from `assets.dir` scan, Vite/webpack `manifest`, and explicit `assets.files` pairs.
2. Probes each URL at the edge and compares hashes (`verify = "body" | "etag" | "digest"`).
3. Purges drifted URLs via Cloudflare Cache API (`/zones/{zone}/purge_cache`), batched, retried on 429/5xx.
4. Purges orphans: URLs in last sync state that vanished from the build (stale hashed files, attachments).
5. Persists state (`.edge-purge-sync.state.json`) only after a clean purge run.

## Usage

```bash
export CF_API_TOKEN=...   # Zone.Cache Purge
export CF_ZONE_ID=...

cp edge-purge.toml.example edge-purge.toml

# pre-deploy / post-build hook: diff + granular purge
edge-purge-sync sync

# diff only, CI gate (exit 3 when purgeable drift remains)
edge-purge-sync scan --json --fail-on-drift

# dry run: report what would be purged
edge-purge-sync sync --dry-run

# micro-daemon: periodic sync cycles, Ctrl-C to stop
edge-purge-sync daemon --interval 300

# manual granular purge
edge-purge-sync purge --url https://example.com/app.js
edge-purge-sync purge --tag assets
edge-purge-sync purge --everything

edge-purge-sync status
```

Logs on stderr, report on stdout (`--json` for machine readable). Exit 0 ok, 1 error, 3 drift left with `--fail-on-drift`.

## Config

See `edge-purge.toml.example`. Env `CF_API_TOKEN` / `CF_ZONE_ID` override config values.

## Development

```bash
cargo test
cargo clippy --all-targets
cargo fmt
```
