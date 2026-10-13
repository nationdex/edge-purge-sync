use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(String),
    #[error("missing env var {0}")]
    MissingEnv(&'static str),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("cloudflare api: code {code}: {message}")]
    Cloudflare { code: i64, message: String },
    #[error("cloudflare api: http {status}: {body}")]
    CloudflareHttp { status: u16, body: String },
    #[error("path not found: {0}")]
    NotFound(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;
