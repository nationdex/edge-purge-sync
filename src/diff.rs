use serde::Serialize;

use crate::config::VerifyMode;
use crate::manifest::{HashKind, LocalAsset};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    /// Edge hash differs from the local build hash (or edge serves stale 404).
    Changed,
    /// Present in previous sync state, gone from the current build.
    Orphaned,
    /// Local and edge hashes are not comparable with the configured verify mode.
    Unverifiable,
    /// Edge probe failed.
    ProbeError,
}

impl std::fmt::Display for DriftKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Changed => "changed",
            Self::Orphaned => "orphaned",
            Self::Unverifiable => "unverifiable",
            Self::ProbeError => "probe-error",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Drift {
    pub url: String,
    pub kind: DriftKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_hash: Option<String>,
    pub detail: String,
}

impl Drift {
    pub fn is_purgeable(&self) -> bool {
        matches!(self.kind, DriftKind::Changed | DriftKind::Orphaned)
    }
}

/// `None` means the two hashes cannot be compared in this verify mode.
pub fn hash_matches(local: &LocalAsset, edge_hash: &str, verify: VerifyMode) -> Option<bool> {
    match local.kind {
        HashKind::Content => {
            if local.hash.eq_ignore_ascii_case(edge_hash) {
                return Some(true);
            }
            if verify == VerifyMode::Etag {
                let local = local.hash.to_ascii_lowercase();
                let edge = edge_hash.to_ascii_lowercase();
                return Some(edge.starts_with(&local) || local.starts_with(&edge));
            }
            Some(false)
        }
        HashKind::Embedded => {
            if verify == VerifyMode::Body {
                return None;
            }
            let local = local.hash.to_ascii_lowercase();
            let edge = edge_hash.to_ascii_lowercase();
            Some(edge == local || edge.contains(&local) || local.contains(&edge))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(hash: &str) -> LocalAsset {
        LocalAsset {
            url: "https://cdn.example.com/a.js".into(),
            hash: hash.into(),
            kind: HashKind::Content,
            source: "dist/a.js".into(),
        }
    }

    fn embedded(hash: &str) -> LocalAsset {
        LocalAsset {
            url: "https://cdn.example.com/a.js".into(),
            hash: hash.into(),
            kind: HashKind::Embedded,
            source: "dist/a.js".into(),
        }
    }

    #[test]
    fn content_hash_rules() {
        assert_eq!(
            hash_matches(&content("abc123"), "abc123", VerifyMode::Body),
            Some(true)
        );
        assert_eq!(
            hash_matches(&content("abc123"), "def456", VerifyMode::Body),
            Some(false)
        );
        assert_eq!(
            hash_matches(&content("abc123"), r#"W"abc123-x""#, VerifyMode::Etag),
            Some(false)
        );
        assert_eq!(
            hash_matches(&content("abc123"), "abc123-extra", VerifyMode::Etag),
            Some(true)
        );
    }

    #[test]
    fn embedded_hash_rules() {
        assert_eq!(
            hash_matches(&embedded("a1b2c3d4"), "a1b2c3d4", VerifyMode::Etag),
            Some(true)
        );
        assert_eq!(
            hash_matches(&embedded("a1b2c3d4"), "zz-a1b2c3d4-zz", VerifyMode::Digest),
            Some(true)
        );
        assert_eq!(
            hash_matches(&embedded("a1b2c3d4"), "ffffff", VerifyMode::Etag),
            Some(false)
        );
        assert_eq!(
            hash_matches(&embedded("a1b2c3d4"), "ffffff", VerifyMode::Body),
            None
        );
    }
}
