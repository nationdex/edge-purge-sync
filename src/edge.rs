use std::time::Duration;

use futures::{stream, StreamExt};
use tracing::debug;

use crate::config::{Config, EdgeConfig, VerifyMode};
use crate::error::Result;
use crate::hash::{normalize_etag, parse_digest_header, HashAlgo};

#[derive(Debug, Clone)]
pub struct EdgeProbe {
    pub url: String,
    pub status: u16,
    pub cache_status: Option<String>,
    pub etag: Option<String>,
    pub hash: Option<String>,
}

pub fn http_client(cfg: &Config) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.edge.timeout_secs))
        .user_agent(cfg.edge.user_agent.clone())
        .build()?)
}

pub async fn probe_one(
    client: &reqwest::Client,
    url: &str,
    edge: &EdgeConfig,
    algo: HashAlgo,
) -> Result<EdgeProbe> {
    let resp = if edge.verify == VerifyMode::Body {
        client.get(url).send().await?
    } else {
        match client.head(url).send().await {
            Ok(resp) if resp.status() != reqwest::StatusCode::METHOD_NOT_ALLOWED => resp,
            _ => client.get(url).send().await?,
        }
    };
    let status = resp.status().as_u16();
    let etag = header(&resp, "etag");
    let cache_status = header(&resp, "cf-cache-status");
    let hash = match edge.verify {
        VerifyMode::Etag => etag.as_deref().map(normalize_etag),
        VerifyMode::Digest => header(&resp, "digest")
            .as_deref()
            .and_then(|value| parse_digest_header(value, algo))
            .or_else(|| {
                header(&resp, "content-digest")
                    .as_deref()
                    .and_then(|value| parse_digest_header(value, algo))
            }),
        VerifyMode::Body => {
            let bytes = resp.bytes().await?;
            Some(algo.hash(&bytes))
        }
    };
    debug!(url, status, ?cache_status, "edge probe");
    Ok(EdgeProbe {
        url: url.to_string(),
        status,
        cache_status,
        etag,
        hash,
    })
}

pub async fn probe_all(
    client: &reqwest::Client,
    urls: Vec<String>,
    cfg: &Config,
) -> Vec<(String, Result<EdgeProbe>)> {
    let edge = cfg.edge.clone();
    let algo = cfg.assets.hash_algorithm;
    let concurrency = cfg.edge.concurrency.max(1);
    stream::iter(urls.into_iter().map(|url| {
        let client = client.clone();
        let edge = edge.clone();
        async move {
            let result = probe_one(&client, &url, &edge, algo).await;
            (url, result)
        }
    }))
    .buffer_unordered(concurrency)
    .collect()
    .await
}

fn header(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}
