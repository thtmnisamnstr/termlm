use crate::extract::{ExtractOptions, extract_markdown_with_options};
use crate::security::validate_web_url;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use url::Url;

static ROBOTS_CACHE: Lazy<Mutex<BTreeMap<String, RobotsCacheEntry>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));
pub const DEFAULT_MAX_REDIRECTS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebReadRequest {
    pub url: String,
    pub max_bytes: usize,
    pub allow_plain_http: bool,
    pub allow_local_addresses: bool,
    pub user_agent: String,
    pub obey_robots_txt: bool,
    pub min_delay_between_requests_ms: u64,
    pub robots_cache_ttl_secs: u64,
    pub extract_strategy: String,
    pub include_images: bool,
    pub include_links: bool,
    pub include_tables: bool,
    pub max_table_rows: usize,
    pub max_table_cols: usize,
    pub preserve_code_blocks: bool,
    pub strip_tracking_params: bool,
    pub max_html_bytes: usize,
    pub max_markdown_bytes: usize,
    pub min_extracted_chars: usize,
    pub dedupe_boilerplate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebReadResponse {
    pub url: String,
    pub normalized_url: String,
    pub final_url: String,
    pub title: Option<String>,
    pub markdown: String,
    pub retrieved_at: DateTime<Utc>,
    pub content_type: String,
    pub status: u16,
    pub truncated: bool,
    pub fetched_bytes: usize,
    pub extracted_bytes: usize,
    pub extraction_method: String,
    pub extraction_status: String,
    pub content_hash_prefix: String,
    pub robots_allowed: bool,
}

pub fn web_read_redirect_policy(
    allow_plain_http: bool,
    allow_local_addresses: bool,
    max_redirects: usize,
) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let hop = attempt.previous().len();
        if hop > max_redirects {
            return attempt.error(format!(
                "redirect limit exceeded (max {max_redirects} hops)"
            ));
        }
        if let Err(err) = validate_web_url(
            attempt.url().as_str(),
            allow_plain_http,
            allow_local_addresses,
        ) {
            return attempt.error(format!(
                "redirect target failed security policy at hop {}: {err}",
                hop
            ));
        }
        attempt.follow()
    })
}

pub async fn web_read(client: &Client, req: &WebReadRequest) -> Result<WebReadResponse> {
    let max_bytes = req.max_bytes.max(1);
    let parsed = validate_web_url(&req.url, req.allow_plain_http, req.allow_local_addresses)?;
    let robots_allowed = robots_allowed(client, req, &parsed).await?;
    if !robots_allowed {
        anyhow::bail!("blocked by robots.txt policy");
    }

    let res = client
        .get(parsed.clone())
        .header("User-Agent", &req.user_agent)
        .send()
        .await
        .context("fetch failed")?;

    let status = res.status().as_u16();
    let final_url = res.url().to_string();
    let _final_parsed =
        validate_web_url(&final_url, req.allow_plain_http, req.allow_local_addresses)
            .context("redirect target failed security policy")?;
    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let mut bytes = Vec::<u8>::with_capacity(max_bytes.min(8192).saturating_add(1));
    let mut stream = res.bytes_stream();
    let mut truncated = false;
    while let Some(next) = stream.next().await {
        let chunk = next.context("stream read failed")?;
        let remaining = max_bytes.saturating_sub(bytes.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        if chunk.len() <= remaining {
            bytes.extend_from_slice(&chunk);
        } else {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
    }

    let parse_cap = req.max_html_bytes.max(1024).min(max_bytes);
    let html_bytes = if bytes.len() > parse_cap {
        truncated = true;
        &bytes[..parse_cap]
    } else {
        bytes.as_slice()
    };
    let html = String::from_utf8_lossy(html_bytes).to_string();
    let extracted = extract_markdown_with_options(
        &html,
        &ExtractOptions {
            strategy: req.extract_strategy.clone(),
            include_images: req.include_images,
            include_links: req.include_links,
            include_tables: req.include_tables,
            max_table_rows: req.max_table_rows,
            max_table_cols: req.max_table_cols,
            preserve_code_blocks: req.preserve_code_blocks,
            strip_tracking_params: req.strip_tracking_params,
            min_extracted_chars: req.min_extracted_chars,
            dedupe_boilerplate: req.dedupe_boilerplate,
        },
    );
    let title = extracted.title;
    let mut markdown = extracted.markdown;
    if markdown.len() > req.max_markdown_bytes {
        markdown = truncate_utf8_to_bytes(&markdown, req.max_markdown_bytes);
        truncated = true;
    }
    let extracted_bytes = markdown.len();
    let mut hasher = Sha256::new();
    hasher.update(markdown.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let hash_prefix = hash.chars().take(12).collect::<String>();

    Ok(WebReadResponse {
        url: req.url.clone(),
        normalized_url: normalize_url(&parsed),
        final_url,
        title,
        markdown,
        retrieved_at: Utc::now(),
        content_type,
        status,
        truncated,
        fetched_bytes: bytes.len(),
        extracted_bytes,
        extraction_method: extracted.method,
        extraction_status: extracted.status,
        content_hash_prefix: hash_prefix,
        robots_allowed,
    })
}

fn truncate_utf8_to_bytes(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    let mut end = max_bytes.min(input.len());
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    input[..end].to_string()
}

#[derive(Debug, Clone)]
struct RobotsCacheEntry {
    fetched_at: Instant,
    body: String,
}

async fn robots_allowed(client: &Client, req: &WebReadRequest, target: &Url) -> Result<bool> {
    if !req.obey_robots_txt {
        return Ok(true);
    }
    let host = target
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("missing host in URL"))?;
    let mut origin = format!("{}://{}", target.scheme(), host);
    if let Some(port) = target.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    let robots_url = format!("{origin}/robots.txt");
    let cache_key = origin.clone();
    let ttl = Duration::from_secs(req.robots_cache_ttl_secs.max(60));

    let robots_body = {
        let cache = ROBOTS_CACHE.lock().await;
        if let Some(entry) = cache.get(&cache_key)
            && entry.fetched_at.elapsed() <= ttl
        {
            Some(entry.body.clone())
        } else {
            None
        }
    };

    let robots_body = if let Some(cached) = robots_body {
        cached
    } else {
        let response = client
            .get(&robots_url)
            .header("User-Agent", &req.user_agent)
            .send()
            .await;

        let Some(response) = response.ok() else {
            return Ok(true);
        };
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(true);
        }
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(false);
        }
        if !status.is_success() {
            return Ok(true);
        }
        let body = response.text().await.unwrap_or_else(|_| String::new());
        let mut cache = ROBOTS_CACHE.lock().await;
        cache.insert(
            cache_key,
            RobotsCacheEntry {
                fetched_at: Instant::now(),
                body: body.clone(),
            },
        );
        body
    };

    let mut matcher = robotstxt::DefaultMatcher::default();
    let agent = req
        .user_agent
        .split_whitespace()
        .next()
        .unwrap_or("termlm")
        .trim();
    Ok(matcher.one_agent_allowed_by_robots(&robots_body, agent, target.as_str()))
}

fn normalize_url(url: &Url) -> String {
    let mut normalized = url.clone();
    normalized.set_fragment(None);
    normalized.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_request_path(stream: &mut TcpStream) -> String {
        let mut buf = vec![0u8; 4096];
        let mut used = 0usize;
        loop {
            if used == buf.len() {
                break;
            }
            let n = stream.read(&mut buf[used..]).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            used += n;
            if used >= 4 && buf[..used].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let req = String::from_utf8_lossy(&buf[..used]);
        req.lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string()
    }

    fn http_response(status_line: &str, headers: &[(&str, String)], body: &str) -> String {
        let mut response = format!("{status_line}\r\n");
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("Content-Length: ");
        response.push_str(&body.len().to_string());
        response.push_str("\r\nConnection: close\r\n\r\n");
        response.push_str(body);
        response
    }

    fn error_chain_text(err: &reqwest::Error) -> String {
        let mut out = err.to_string();
        let mut source = err.source();
        while let Some(next) = source {
            out.push_str(" | ");
            out.push_str(&next.to_string());
            source = next.source();
        }
        out
    }

    async fn spawn_redirect_chain_server(
        redirects: usize,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        let base = format!("http://{addr}");
        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let path = read_http_request_path(&mut stream).await;
                let response = if let Some(step_raw) = path.strip_prefix("/hop/") {
                    let step = step_raw.parse::<usize>().unwrap_or(0);
                    if step < redirects {
                        http_response(
                            "HTTP/1.1 302 Found",
                            &[("Location", format!("/hop/{}", step + 1))],
                            "",
                        )
                    } else {
                        http_response(
                            "HTTP/1.1 200 OK",
                            &[("Content-Type", "text/html".to_string())],
                            "<html><title>ok</title><body>done</body></html>",
                        )
                    }
                } else if path == "/final" {
                    http_response(
                        "HTTP/1.1 200 OK",
                        &[("Content-Type", "text/html".to_string())],
                        "<html><title>ok</title><body>done</body></html>",
                    )
                } else {
                    http_response("HTTP/1.1 404 Not Found", &[], "")
                };
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (base, handle)
    }

    async fn spawn_large_page_server(html_bytes: usize) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind large-page listener");
        let addr = listener.local_addr().expect("local addr");
        let base = format!("http://{addr}");
        let payload = format!(
            "<html><head><title>caps</title></head><body><main>{}</main></body></html>",
            "A".repeat(html_bytes.max(1))
        );
        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let _path = read_http_request_path(&mut stream).await;
                let response = http_response(
                    "HTTP/1.1 200 OK",
                    &[("Content-Type", "text/html; charset=utf-8".to_string())],
                    &payload,
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (base, handle)
    }

    #[tokio::test]
    async fn redirect_limit_is_enforced() {
        let (base, server) = spawn_redirect_chain_server(3).await;
        let client = Client::builder()
            .redirect(web_read_redirect_policy(true, true, 1))
            .build()
            .expect("build client");
        let err = client
            .get(format!("{base}/hop/0"))
            .send()
            .await
            .expect_err("expected redirect limit error");
        let msg = error_chain_text(&err);
        assert!(
            msg.contains("redirect limit exceeded"),
            "unexpected error: {msg}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn redirect_targets_are_revalidated() {
        let (base, server) = spawn_redirect_chain_server(1).await;
        let client = Client::builder()
            .redirect(web_read_redirect_policy(true, false, DEFAULT_MAX_REDIRECTS))
            .build()
            .expect("build client");
        let err = client
            .get(format!("{base}/hop/0"))
            .send()
            .await
            .expect_err("expected redirect policy error");
        let msg = error_chain_text(&err);
        assert!(
            msg.contains("redirect target failed security policy"),
            "unexpected error: {msg}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn web_read_respects_fetch_parse_and_markdown_caps() {
        let (base, server) = spawn_large_page_server(32_768).await;
        let client = Client::builder()
            .redirect(web_read_redirect_policy(true, true, DEFAULT_MAX_REDIRECTS))
            .build()
            .expect("build client");
        let req = WebReadRequest {
            url: format!("{base}/caps"),
            max_bytes: 4_096,
            allow_plain_http: true,
            allow_local_addresses: true,
            user_agent: "termlm-test".to_string(),
            obey_robots_txt: false,
            min_delay_between_requests_ms: 0,
            robots_cache_ttl_secs: 60,
            extract_strategy: "auto".to_string(),
            include_images: false,
            include_links: true,
            include_tables: true,
            max_table_rows: 20,
            max_table_cols: 6,
            preserve_code_blocks: true,
            strip_tracking_params: true,
            max_html_bytes: 1_024,
            max_markdown_bytes: 512,
            min_extracted_chars: 100,
            dedupe_boilerplate: true,
        };
        let out = web_read(&client, &req).await.expect("web_read");
        assert!(out.fetched_bytes <= req.max_bytes);
        assert!(out.extracted_bytes <= req.max_markdown_bytes);
        assert!(out.truncated);
        server.abort();
    }
}
