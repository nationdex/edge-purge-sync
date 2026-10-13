use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::AssetsConfig;
use crate::error::{Error, Result};
use crate::hash::{embedded_hash, HashAlgo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashKind {
    /// Hash of file content with the configured algorithm.
    Content,
    /// Hash taken from the artifact file name (only comparable with ETag).
    Embedded,
}

#[derive(Debug, Clone)]
pub struct LocalAsset {
    pub url: String,
    pub hash: String,
    pub kind: HashKind,
    pub source: PathBuf,
}

/// Keyed by public URL.
pub type AssetMap = BTreeMap<String, LocalAsset>;

pub fn collect(cfg: &AssetsConfig, algo: HashAlgo) -> Result<(AssetMap, Vec<String>)> {
    let mut assets = AssetMap::new();
    let mut skipped = Vec::new();
    if let Some(dir) = &cfg.dir {
        if !dir.is_dir() {
            return Err(Error::NotFound(dir.clone()));
        }
        collect_dir(dir, dir, cfg, algo, &mut assets, &mut skipped);
    }
    if let Some(manifest) = &cfg.manifest {
        collect_manifest(manifest, cfg, algo, &mut assets, &mut skipped)?;
    }
    for (url, path) in &cfg.files {
        match hash_for(path, cfg, algo) {
            Some((hash, kind)) => {
                assets.insert(
                    url.clone(),
                    LocalAsset {
                        url: url.clone(),
                        hash,
                        kind,
                        source: path.clone(),
                    },
                );
            }
            None => skipped.push(format!("{}: no hash", path.display())),
        }
    }
    Ok((assets, skipped))
}

fn collect_dir(
    root: &Path,
    dir: &Path,
    cfg: &AssetsConfig,
    algo: HashAlgo,
    assets: &mut AssetMap,
    skipped: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_dir(root, &path, cfg, algo, assets, skipped);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if !cfg.allows(&rel) {
            continue;
        }
        match hash_for(&path, cfg, algo) {
            Some((hash, kind)) => {
                let url = join_url(&cfg.base_url, &cfg.url_prefix, &rel);
                assets.insert(
                    url.clone(),
                    LocalAsset {
                        url,
                        hash,
                        kind,
                        source: path.clone(),
                    },
                );
            }
            None => skipped.push(format!("{}: no hash", path.display())),
        }
    }
}

fn collect_manifest(
    path: &Path,
    cfg: &AssetsConfig,
    algo: HashAlgo,
    assets: &mut AssetMap,
    skipped: &mut Vec<String>,
) -> Result<()> {
    let raw = std::fs::read_to_string(path)?;
    let json: Value = serde_json::from_str(&raw)?;
    let mut files = Vec::new();
    walk_manifest(&json, &mut files);
    files.sort();
    files.dedup();
    let root = cfg.dir.clone().unwrap_or_else(|| PathBuf::from("."));
    for rel in files {
        let rel = rel.trim_start_matches('/').to_string();
        if !cfg.allows(&rel) {
            continue;
        }
        let file_path = root.join(&rel);
        match hash_for(&file_path, cfg, algo) {
            Some((hash, kind)) => {
                let url = join_url(&cfg.base_url, "", &rel);
                assets.insert(
                    url.clone(),
                    LocalAsset {
                        url,
                        hash,
                        kind,
                        source: file_path,
                    },
                );
            }
            None => skipped.push(format!("{}: no hash", rel)),
        }
    }
    Ok(())
}

fn walk_manifest(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(file)) = map.get("file") {
                out.push(file.clone());
            }
            if let Some(Value::Object(files)) = map.get("files") {
                for entry in files.values() {
                    if let Value::String(p) = entry {
                        out.push(p.clone());
                    }
                }
            }
            for key in ["css", "assets"] {
                if let Some(Value::Array(items)) = map.get(key) {
                    for entry in items {
                        if let Value::String(p) = entry {
                            out.push(p.clone());
                        }
                    }
                }
            }
            for entry in map.values() {
                walk_manifest(entry, out);
            }
        }
        Value::Array(items) => {
            for entry in items {
                walk_manifest(entry, out);
            }
        }
        _ => {}
    }
}

fn hash_for(path: &Path, cfg: &AssetsConfig, algo: HashAlgo) -> Option<(String, HashKind)> {
    if path.is_file() {
        if let Ok(hash) = algo.hash_file(path) {
            return Some((hash, HashKind::Content));
        }
    }
    if cfg.hash_from_filename {
        let name = path.file_name()?.to_string_lossy().to_string();
        if let Some(hash) = embedded_hash(&name) {
            return Some((hash, HashKind::Embedded));
        }
    }
    None
}

pub fn join_url(base: &str, prefix: &str, rel: &str) -> String {
    let mut path = String::from("/");
    let prefix = prefix.trim_matches('/');
    if !prefix.is_empty() {
        path.push_str(prefix);
        path.push('/');
    }
    path.push_str(rel.trim_start_matches('/'));
    format!("{}{}", base.trim_end_matches('/'), path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AssetsConfig;

    #[test]
    fn joins_urls() {
        assert_eq!(
            join_url("https://cdn.example.com/", "/assets/", "app.js"),
            "https://cdn.example.com/assets/app.js"
        );
        assert_eq!(
            join_url("https://cdn.example.com", "", "app.js"),
            "https://cdn.example.com/app.js"
        );
    }

    #[test]
    fn collects_vite_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dist = tmp.path().join("dist/assets");
        std::fs::create_dir_all(&dist).expect("mkdir");
        std::fs::write(dist.join("main.a1b2c3d4.js"), "console.log(1)").expect("write");
        let manifest = r#"{
            "main.js": {
                "file": "assets/main.a1b2c3d4.js",
                "src": "main.js",
                "isEntry": true,
                "css": ["assets/main.a1b2c3d4.css"]
            },
            "style.css": { "file": "assets/style.e5f6a7b8.css" }
        }"#;
        std::fs::write(tmp.path().join("manifest.json"), manifest).expect("write manifest");
        let cfg = AssetsConfig {
            base_url: "https://cdn.example.com".into(),
            dir: Some(tmp.path().join("dist")),
            manifest: Some(tmp.path().join("manifest.json")),
            ..Default::default()
        };
        let (assets, skipped) = collect(&cfg, HashAlgo::Sha256).expect("collect");
        assert!(skipped.iter().any(|s| s.contains("main.a1b2c3d4.css")));
        assert_eq!(assets.len(), 1);
        let asset = &assets["https://cdn.example.com/assets/main.a1b2c3d4.js"];
        assert_eq!(asset.kind, HashKind::Content);
        assert_eq!(asset.hash, HashAlgo::Sha256.hash(b"console.log(1)"));
    }

    #[test]
    fn collects_dir_scan_with_filters() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("dist/sub")).expect("mkdir");
        std::fs::write(tmp.path().join("dist/app.deadbeef.js"), "a").expect("write");
        std::fs::write(tmp.path().join("dist/sub/notes.txt"), "b").expect("write");
        let cfg = AssetsConfig {
            base_url: "https://cdn.example.com".into(),
            dir: Some(tmp.path().join("dist")),
            url_prefix: "/assets/".into(),
            extensions: vec!["js".into()],
            ..Default::default()
        };
        let (assets, _) = collect(&cfg, HashAlgo::Sha256).expect("collect");
        assert_eq!(assets.len(), 1);
        assert!(assets.contains_key("https://cdn.example.com/assets/app.deadbeef.js"));
    }

    #[test]
    fn embedded_fallback_when_file_missing() {
        let cfg = AssetsConfig {
            base_url: "https://cdn.example.com".into(),
            files: BTreeMap::from([(
                "https://cdn.example.com/app.a1b2c3d4.js".to_string(),
                PathBuf::from("missing/app.a1b2c3d4.js"),
            )]),
            hash_from_filename: true,
            ..Default::default()
        };
        let (assets, _) = collect(&cfg, HashAlgo::Sha256).expect("collect");
        let asset = &assets["https://cdn.example.com/app.a1b2c3d4.js"];
        assert_eq!(asset.kind, HashKind::Embedded);
        assert_eq!(asset.hash, "a1b2c3d4");
    }
}
