use std::path::Path;

use base64::Engine as _;
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest as _, Sha256};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgo {
    #[default]
    Sha256,
    Sha1,
    Md5,
}

impl std::str::FromStr for HashAlgo {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "sha256" | "sha-256" => Ok(Self::Sha256),
            "sha1" | "sha-1" => Ok(Self::Sha1),
            "md5" => Ok(Self::Md5),
            other => Err(format!("unknown hash algorithm: {other}")),
        }
    }
}

impl HashAlgo {
    pub fn hex_len(self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Sha1 => 40,
            Self::Md5 => 32,
        }
    }

    pub fn digest_name(self) -> &'static str {
        match self {
            Self::Sha256 => "sha-256",
            Self::Sha1 => "sha-1",
            Self::Md5 => "md5",
        }
    }

    pub fn hash(self, data: &[u8]) -> String {
        match self {
            Self::Sha256 => hex::encode(Sha256::digest(data)),
            Self::Sha1 => hex::encode(Sha1::digest(data)),
            Self::Md5 => hex::encode(Md5::digest(data)),
        }
    }

    pub fn hash_file(self, path: &Path) -> Result<String> {
        Ok(self.hash(&std::fs::read(path)?))
    }
}

pub fn normalize_etag(value: &str) -> String {
    let value = value.trim();
    let value = value.strip_prefix("W/").unwrap_or(value);
    value.trim_matches('"').to_string()
}

/// Parse `Digest` / `Content-Digest` header payloads (hex or base64).
pub fn parse_digest_header(value: &str, algo: HashAlgo) -> Option<String> {
    for part in value.split(',') {
        let Some((name, payload)) = part.split_once('=') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase().replace('-', "");
        let expected = match name.as_str() {
            "sha256" => HashAlgo::Sha256,
            "sha1" => HashAlgo::Sha1,
            "md5" => HashAlgo::Md5,
            _ => continue,
        };
        if expected != algo {
            continue;
        }
        let payload = payload.trim().trim_matches(':');
        if payload.is_empty() {
            continue;
        }
        if payload.len() == expected.hex_len() && payload.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(payload.to_ascii_lowercase());
        }
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(payload) {
            return Some(hex::encode(bytes));
        }
    }
    None
}

/// Content hash embedded in a hashed build artifact name, e.g. `app.a1b2c3d4.js`.
pub fn embedded_hash(file_name: &str) -> Option<String> {
    let (stem, _ext) = file_name.rsplit_once('.')?;
    let (_prefix, candidate) = stem.rsplit_once('.')?;
    let candidate = candidate.to_ascii_lowercase();
    if candidate.len() >= 6 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(candidate)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_etags() {
        assert_eq!(normalize_etag(r#"W/"abc""#), "abc");
        assert_eq!(normalize_etag(r#""abc""#), "abc");
        assert_eq!(normalize_etag("abc"), "abc");
    }

    #[test]
    fn parses_digest_hex_and_base64() {
        let data = b"hello";
        let sha_hex = HashAlgo::Sha256.hash(data);
        let b64 = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(data));
        assert_eq!(
            parse_digest_header(&format!("sha-256={sha_hex}"), HashAlgo::Sha256),
            Some(sha_hex.clone())
        );
        assert_eq!(
            parse_digest_header(&format!("sha-256=:{b64}:"), HashAlgo::Sha256),
            Some(sha_hex.clone())
        );
        assert_eq!(
            parse_digest_header(
                &format!("sha-1={}", HashAlgo::Sha1.hash(data)),
                HashAlgo::Sha256
            ),
            None
        );
    }

    #[test]
    fn extracts_embedded_hash() {
        assert_eq!(
            embedded_hash("app.a1b2c3d4.js").as_deref(),
            Some("a1b2c3d4")
        );
        assert_eq!(
            embedded_hash("style.E5F6A7B8.css").as_deref(),
            Some("e5f6a7b8")
        );
        assert_eq!(embedded_hash("app.min.js"), None);
        assert_eq!(embedded_hash("app.js"), None);
        assert_eq!(embedded_hash("jquery-3.6.0.min.js"), None);
    }

    #[test]
    fn hashes_known_vectors() {
        assert_eq!(
            HashAlgo::Sha256.hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
