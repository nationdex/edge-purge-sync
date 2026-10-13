use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::hash::HashAlgo;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub cloudflare: CloudflareConfig,
    #[serde(default)]
    pub assets: AssetsConfig,
    #[serde(default)]
    pub edge: EdgeConfig,
    #[serde(default)]
    pub purge: PurgeConfig,
    #[serde(default)]
    pub state: StateConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CloudflareConfig {
    #[serde(default = "default_api_base")]
    pub base_url: String,
    #[serde(default)]
    pub zone_id: String,
    #[serde(default)]
    pub api_token: Option<String>,
}

impl Default for CloudflareConfig {
    fn default() -> Self {
        Self {
            base_url: default_api_base(),
            zone_id: String::new(),
            api_token: None,
        }
    }
}

fn default_api_base() -> String {
    "https://api.cloudflare.com/client/v4".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetsConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default)]
    pub manifest: Option<PathBuf>,
    /// Explicit `url = local-file` pairs for unhashed assets (index.html, attachments).
    #[serde(default)]
    pub files: BTreeMap<String, PathBuf>,
    #[serde(default = "default_url_prefix")]
    pub url_prefix: String,
    #[serde(default)]
    pub hash_algorithm: HashAlgo,
    /// Extension filter for scanned files, e.g. ["js", "css"]. Empty means all.
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Fall back to the content hash embedded in artifact file names.
    #[serde(default)]
    pub hash_from_filename: bool,
}

impl Default for AssetsConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            dir: None,
            manifest: None,
            files: BTreeMap::new(),
            url_prefix: default_url_prefix(),
            hash_algorithm: HashAlgo::default(),
            extensions: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            hash_from_filename: false,
        }
    }
}

fn default_url_prefix() -> String {
    "/".to_string()
}

impl AssetsConfig {
    pub fn has_sources(&self) -> bool {
        self.dir.is_some() || self.manifest.is_some() || !self.files.is_empty()
    }

    pub fn allows(&self, rel: &str) -> bool {
        if !self.extensions.is_empty() {
            let ext = rel.rsplit('.').next().unwrap_or("");
            if !self.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                return false;
            }
        }
        if !self.include.is_empty() && !self.include.iter().any(|p| wildcard_match(p, rel)) {
            return false;
        }
        if self.exclude.iter().any(|p| wildcard_match(p, rel)) {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyMode {
    /// Compare local hash against the edge ETag.
    Etag,
    /// Compare local hash against the edge `Digest` / `Content-Digest` header.
    Digest,
    /// Hash the edge response body and compare with the local build hash.
    #[default]
    Body,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EdgeConfig {
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub verify: VerifyMode,
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
}

impl Default for EdgeConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_timeout(),
            concurrency: default_concurrency(),
            verify: VerifyMode::default(),
            user_agent: default_user_agent(),
        }
    }
}

fn default_timeout() -> u64 {
    15
}

fn default_concurrency() -> usize {
    8
}

fn default_user_agent() -> String {
    format!("edge-purge-sync/{}", env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PurgeStrategy {
    #[default]
    Url,
    Tag,
    Everything,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PurgeConfig {
    #[serde(default)]
    pub strategy: PurgeStrategy,
    #[serde(default = "default_batch")]
    pub batch_size: usize,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    #[serde(default = "default_backoff")]
    pub retry_backoff_ms: u64,
    /// Cache tags used when `strategy = "tag"`.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Default for PurgeConfig {
    fn default() -> Self {
        Self {
            strategy: PurgeStrategy::default(),
            batch_size: default_batch(),
            max_retries: default_retries(),
            retry_backoff_ms: default_backoff(),
            tags: Vec::new(),
        }
    }
}

fn default_batch() -> usize {
    30
}

fn default_retries() -> u32 {
    3
}

fn default_backoff() -> u64 {
    500
}

#[derive(Debug, Clone, Deserialize)]
pub struct StateConfig {
    #[serde(default = "default_state_path")]
    pub path: PathBuf,
}

impl Default for StateConfig {
    fn default() -> Self {
        Self {
            path: default_state_path(),
        }
    }
}

fn default_state_path() -> PathBuf {
    PathBuf::from(".edge-purge-sync.state.json")
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_interval(),
        }
    }
}

fn default_interval() -> u64 {
    300
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("read {}: {e}", path.display())))?;
        let mut cfg: Config = toml::from_str(&raw)?;
        cfg.resolve_env();
        cfg.validate()?;
        Ok(cfg)
    }

    fn resolve_env(&mut self) {
        if self.cloudflare.api_token.is_none() {
            self.cloudflare.api_token =
                std::env::var("CF_API_TOKEN").ok().filter(|s| !s.is_empty());
        }
        if self.cloudflare.zone_id.is_empty() {
            self.cloudflare.zone_id = std::env::var("CF_ZONE_ID").unwrap_or_default();
        }
    }

    fn validate(&self) -> Result<()> {
        if self.purge.batch_size == 0 {
            return Err(Error::Config("purge.batch_size must be > 0".into()));
        }
        if self.edge.concurrency == 0 {
            return Err(Error::Config("edge.concurrency must be > 0".into()));
        }
        if self.edge.timeout_secs == 0 {
            return Err(Error::Config("edge.timeout_secs must be > 0".into()));
        }
        Ok(())
    }
}

/// Wildcard match supporting `*` and `?`.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = i;
            mark = j;
            i += 1;
        } else if star != usize::MAX {
            i = star + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_rules() {
        assert!(wildcard_match("*.js", "app/main.js"));
        assert!(wildcard_match("assets/*", "assets/a/b.css"));
        assert!(wildcard_match("app.?.js", "app.1.js"));
        assert!(!wildcard_match("*.css", "main.js"));
        assert!(!wildcard_match("a*b", "ab.txt"));
    }

    #[test]
    fn parses_config_with_defaults() {
        let cfg: Config = toml::from_str(
            r#"
            [assets]
            base_url = "https://cdn.example.com"
            dir = "dist"
            hash_algorithm = "sha256"
            "#,
        )
        .expect("config parses");
        assert_eq!(cfg.assets.base_url, "https://cdn.example.com");
        assert_eq!(cfg.purge.batch_size, 30);
        assert_eq!(cfg.edge.verify, VerifyMode::Body);
        assert_eq!(cfg.assets.hash_algorithm, HashAlgo::Sha256);
        assert!(cfg.assets.has_sources());
    }

    #[test]
    fn extension_filter() {
        let cfg = AssetsConfig {
            extensions: vec!["js".into(), "css".into()],
            ..Default::default()
        };
        assert!(cfg.allows("main.js"));
        assert!(!cfg.allows("main.map"));
    }
}
