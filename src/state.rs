use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub updated_at: String,
    /// url -> hash observed on the last successful sync.
    pub assets: BTreeMap<String, String>,
}

impl State {
    pub fn empty() -> Self {
        Self {
            version: 1,
            updated_at: now_iso(),
            assets: BTreeMap::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_state() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("state.json");
        let mut state = State::empty();
        state
            .assets
            .insert("https://cdn.example.com/a.js".into(), "abc".into());
        state.save(&path).expect("save");
        let loaded = State::load(&path).expect("load");
        assert_eq!(loaded.assets["https://cdn.example.com/a.js"], "abc");
    }

    #[test]
    fn missing_state_is_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = State::load(&tmp.path().join("nope.json")).expect("load");
        assert!(state.assets.is_empty());
    }
}
