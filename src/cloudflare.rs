use std::time::Duration;

use serde::Serialize;
use tracing::{debug, warn};

use crate::config::{Config, PurgeConfig};
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub enum PurgeTarget {
    Urls(Vec<String>),
    Tags(Vec<String>),
    Hosts(Vec<String>),
    Everything,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum PurgeBody<'a> {
    Files { files: &'a [String] },
    Tags { tags: &'a [String] },
    Hosts { hosts: &'a [String] },
    Everything { purge_everything: bool },
}

#[derive(Debug, serde::Deserialize)]
struct CfEnvelope {
    success: bool,
    #[serde(default)]
    errors: Vec<CfError>,
}

#[derive(Debug, serde::Deserialize)]
struct CfError {
    code: i64,
    message: String,
}

pub struct CloudflareClient {
    http: reqwest::Client,
    base_url: String,
    zone_id: String,
    token: String,
}

impl CloudflareClient {
    pub fn from_config(cfg: &Config) -> Result<Self> {
        let token = cfg
            .cloudflare
            .api_token
            .clone()
            .ok_or(Error::MissingEnv("CF_API_TOKEN"))?;
        if cfg.cloudflare.zone_id.is_empty() {
            return Err(Error::Config("cloudflare.zone_id is empty".into()));
        }
        let base_url = cfg.cloudflare.base_url.trim_end_matches('/').to_string();
        let parsed = reqwest::Url::parse(&base_url)
            .map_err(|e| Error::Config(format!("cloudflare.base_url invalid: {e}")))?;
        if parsed.scheme() != "https" && !is_loopback_host(parsed.host_str()) {
            return Err(Error::Config(
                "cloudflare.base_url must use https (http allowed only for loopback)".into(),
            ));
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(cfg.edge.user_agent.clone())
                .build()?,
            base_url,
            zone_id: cfg.cloudflare.zone_id.clone(),
            token,
        })
    }

    /// Returns the number of purge API batches issued.
    pub async fn purge(&self, target: &PurgeTarget, cfg: &PurgeConfig) -> Result<usize> {
        let batch = cfg.batch_size.max(1);
        let bodies: Vec<PurgeBody<'_>> = match target {
            PurgeTarget::Urls(urls) => {
                if urls.is_empty() {
                    return Ok(0);
                }
                urls.chunks(batch)
                    .map(|c| PurgeBody::Files { files: c })
                    .collect()
            }
            PurgeTarget::Tags(tags) => {
                if tags.is_empty() {
                    return Err(Error::Config(
                        "purge strategy=tag but purge.tags is empty".into(),
                    ));
                }
                tags.chunks(batch)
                    .map(|c| PurgeBody::Tags { tags: c })
                    .collect()
            }
            PurgeTarget::Hosts(hosts) => {
                if hosts.is_empty() {
                    return Ok(0);
                }
                hosts
                    .chunks(batch)
                    .map(|c| PurgeBody::Hosts { hosts: c })
                    .collect()
            }
            PurgeTarget::Everything => vec![PurgeBody::Everything {
                purge_everything: true,
            }],
        };
        let total = bodies.len();
        for body in &bodies {
            self.send_purge(body, cfg).await?;
        }
        Ok(total)
    }

    async fn send_purge(&self, body: &PurgeBody<'_>, cfg: &PurgeConfig) -> Result<()> {
        let url = format!("{}/zones/{}/purge_cache", self.base_url, self.zone_id);
        let mut attempt = 0u32;
        loop {
            let response = self
                .http
                .post(&url)
                .bearer_auth(&self.token)
                .json(body)
                .send()
                .await;
            match response {
                Ok(resp) => {
                    let status = resp.status();
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                    {
                        if attempt >= cfg.max_retries {
                            let text = resp.text().await.unwrap_or_default();
                            return Err(Error::CloudflareHttp {
                                status: status.as_u16(),
                                body: text,
                            });
                        }
                        warn!(%status, attempt, "purge retry");
                    } else {
                        let text = resp.text().await.map_err(|e| {
                            Error::Config(format!(
                                "purge response read failed: {}",
                                describe_http_error(&e)
                            ))
                        })?;
                        if !status.is_success() {
                            return Err(Error::CloudflareHttp {
                                status: status.as_u16(),
                                body: text,
                            });
                        }
                        let envelope: CfEnvelope = serde_json::from_str(&text)?;
                        if !envelope.success {
                            let (code, message) = envelope
                                .errors
                                .first()
                                .map(|e| (e.code, e.message.clone()))
                                .unwrap_or((0, "unknown error".to_string()));
                            return Err(Error::Cloudflare { code, message });
                        }
                        debug!(url, "purge accepted");
                        return Ok(());
                    }
                }
                Err(err) => {
                    if attempt >= cfg.max_retries {
                        return Err(Error::Config(format!(
                            "purge request failed: {}",
                            describe_http_error(&err)
                        )));
                    }
                    warn!(error = %describe_http_error(&err), attempt, "purge retry");
                }
            }
            attempt += 1;
            let backoff = cfg.retry_backoff_ms.saturating_mul(1u64 << attempt.min(6));
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
    }
}

fn is_loopback_host(host: Option<&str>) -> bool {
    match host {
        Some(host) => {
            let host = host.trim_matches(|c| c == '[' || c == ']');
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .map(|ip| ip.is_loopback())
                    .unwrap_or(false)
        }
        None => false,
    }
}

fn describe_http_error(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "request timed out"
    } else if err.is_connect() {
        "connection failed"
    } else if err.is_request() {
        "request failed"
    } else if err.is_body() {
        "response body error"
    } else {
        "transport error"
    }
}
