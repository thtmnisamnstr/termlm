use crate::security::validate_web_url;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;
use url::Url;

const BRAVE_DEFAULT_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const KAGI_DEFAULT_ENDPOINT: &str = "https://kagi.com/api/v0/search";
const TAVILY_DEFAULT_ENDPOINT: &str = "https://api.tavily.com/search";
const WHOOGLE_DEFAULT_ENDPOINT: &str = "http://127.0.0.1:5000/search";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub freshness: Option<String>,
    pub max_results: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub url: String,
    pub normalized_url: String,
    pub title: String,
    pub snippet: String,
    pub content_hash_prefix: String,
    pub provider: String,
    pub rank: usize,
    pub retrieved_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultSet {
    pub query: String,
    pub provider: String,
    pub results: Vec<SearchResult>,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;
    async fn search(&self, req: &SearchRequest) -> Result<SearchResultSet>;
}

pub async fn web_search(
    provider: &dyn SearchProvider,
    req: &SearchRequest,
) -> Result<SearchResultSet> {
    provider.search(req).await
}

pub struct DuckDuckGoHtmlProvider {
    client: Client,
}

impl DuckDuckGoHtmlProvider {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

pub struct CustomJsonProvider {
    client: Client,
    endpoint: String,
    bearer_token: Option<String>,
    allow_plain_http_results: bool,
    allow_local_address_results: bool,
}

impl CustomJsonProvider {
    pub fn new(client: Client, endpoint: impl Into<String>, bearer_token: Option<String>) -> Self {
        Self::new_with_result_policy(client, endpoint, bearer_token, false, false)
    }

    pub fn new_with_result_policy(
        client: Client,
        endpoint: impl Into<String>,
        bearer_token: Option<String>,
        allow_plain_http_results: bool,
        allow_local_address_results: bool,
    ) -> Self {
        Self {
            client,
            endpoint: endpoint.into(),
            bearer_token,
            allow_plain_http_results,
            allow_local_address_results,
        }
    }
}

pub struct BraveProvider {
    client: Client,
    endpoint: String,
    api_key: String,
}

impl BraveProvider {
    pub fn new(client: Client, endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            client,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
        }
    }
}

pub struct KagiProvider {
    client: Client,
    endpoint: String,
    api_key: String,
}

impl KagiProvider {
    pub fn new(client: Client, endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            client,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
        }
    }
}

pub struct TavilyProvider {
    client: Client,
    endpoint: String,
    api_key: String,
}

impl TavilyProvider {
    pub fn new(client: Client, endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            client,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
        }
    }
}

pub struct WhoogleProvider {
    client: Client,
    endpoint: String,
}

impl WhoogleProvider {
    pub fn new(client: Client, endpoint: impl Into<String>) -> Self {
        Self {
            client,
            endpoint: endpoint.into(),
        }
    }
}

#[derive(Debug, Clone)]
struct SearchResponseMeta {
    status: Option<u16>,
    content_type: Option<String>,
    final_url: Option<String>,
    response_bytes: Option<usize>,
    extraction_method: String,
    allow_plain_http_results: bool,
    allow_local_address_results: bool,
}

#[derive(Debug, Clone)]
struct SearchPage {
    body: String,
    meta: SearchResponseMeta,
}

#[async_trait]
impl SearchProvider for DuckDuckGoHtmlProvider {
    fn provider_name(&self) -> &'static str {
        "duckduckgo_html"
    }

    async fn search(&self, req: &SearchRequest) -> Result<SearchResultSet> {
        let mut results = match self
            .fetch_search_page("https://duckduckgo.com/html/", req, "duckduckgo_html")
            .await
        {
            Ok(page) => parse_duckduckgo_results(&page.body, self.provider_name(), req, page.meta),
            Err(primary_error) => {
                let page = self
                    .fetch_search_page(
                        "https://lite.duckduckgo.com/lite/",
                        req,
                        "duckduckgo_lite",
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "duckduckgo_html request failed ({primary_error:#}); duckduckgo_lite fallback failed"
                        )
                    })?;
                parse_duckduckgo_results(&page.body, self.provider_name(), req, page.meta)
            }
        };

        if results.is_empty()
            && let Ok(page) = self
                .fetch_search_page("https://lite.duckduckgo.com/lite/", req, "duckduckgo_lite")
                .await
        {
            results = parse_duckduckgo_results(&page.body, self.provider_name(), req, page.meta);
        }

        Ok(SearchResultSet {
            query: req.query.clone(),
            provider: self.provider_name().to_string(),
            results,
        })
    }
}

impl DuckDuckGoHtmlProvider {
    async fn fetch_search_page(
        &self,
        endpoint: &str,
        req: &SearchRequest,
        extraction_method: &str,
    ) -> Result<SearchPage> {
        let response = self
            .client
            .get(endpoint)
            .query(&[("q", req.query.as_str())])
            .send()
            .await
            .with_context(|| format!("{extraction_method} request failed"))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let final_url = Some(response.url().to_string());
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("{extraction_method} body failed"))?;
        if !status.is_success() {
            anyhow::bail!("{extraction_method} request failed: status {status}");
        }
        Ok(SearchPage {
            body: String::from_utf8_lossy(&bytes).to_string(),
            meta: SearchResponseMeta {
                status: Some(status.as_u16()),
                content_type,
                final_url,
                response_bytes: Some(bytes.len()),
                extraction_method: extraction_method.to_string(),
                allow_plain_http_results: false,
                allow_local_address_results: false,
            },
        })
    }
}

fn parse_duckduckgo_results(
    body: &str,
    provider_name: &str,
    req: &SearchRequest,
    meta: SearchResponseMeta,
) -> Vec<SearchResult> {
    let mut results = parse_duckduckgo_result_links(body, provider_name, req, &meta);
    if results.is_empty() {
        let mut generic_meta = meta.clone();
        generic_meta.extraction_method = format!("{}_generic_links", meta.extraction_method);
        results = parse_generic_html_links(body, provider_name, req, &generic_meta);
    }
    results
}

fn parse_duckduckgo_result_links(
    body: &str,
    provider_name: &str,
    req: &SearchRequest,
    meta: &SearchResponseMeta,
) -> Vec<SearchResult> {
    let href_re = Regex::new(
        r#"(?is)<a\b[^>]*class\s*=\s*["'][^"']*\bresult__a\b[^"']*["'][^>]*href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#,
    )
    .expect("regex");
    let snippet_re = Regex::new(
        r#"(?is)<a\b[^>]*class\s*=\s*["'][^"']*\bresult__snippet\b[^"']*["'][^>]*>(.*?)</a>"#,
    )
    .expect("regex");

    let mut results = Vec::new();
    for cap in href_re.captures_iter(body) {
        if results.len() >= req.max_results {
            break;
        }
        let raw_url = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let raw_title = cap.get(2).map(|m| m.as_str()).unwrap_or_default();
        let snippet = if let Some(full) = cap.get(0) {
            let rest = &body[full.end()..body.len().min(full.end() + 1400)];
            snippet_re
                .captures(rest)
                .and_then(|scap| scap.get(1).map(|m| strip_html_text(m.as_str())))
                .unwrap_or_default()
        } else {
            String::new()
        };
        push_html_search_result(
            &mut results,
            provider_name,
            req,
            raw_url,
            &strip_html_text(raw_title),
            &snippet,
            meta,
        );
    }
    results
}

fn parse_generic_html_links(
    body: &str,
    provider_name: &str,
    req: &SearchRequest,
    meta: &SearchResponseMeta,
) -> Vec<SearchResult> {
    let anchor_re =
        Regex::new(r#"(?is)<a\b[^>]*href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#).expect("regex");
    let mut results = Vec::new();
    for cap in anchor_re.captures_iter(body) {
        if results.len() >= req.max_results {
            break;
        }
        let raw_url = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let title = strip_html_text(cap.get(2).map(|m| m.as_str()).unwrap_or_default());
        if is_generic_search_nav_title(&title) {
            continue;
        }
        push_html_search_result(&mut results, provider_name, req, raw_url, &title, "", meta);
    }
    results
}

fn push_html_search_result(
    results: &mut Vec<SearchResult>,
    provider_name: &str,
    req: &SearchRequest,
    raw_url: &str,
    title: &str,
    snippet: &str,
    meta: &SearchResponseMeta,
) {
    if results.len() >= req.max_results || title.trim().is_empty() {
        return;
    }
    let Some(clean_url) = normalize_ddg_href(raw_url) else {
        return;
    };
    let Ok(normalized) = normalize_result_url(
        &clean_url,
        meta.allow_plain_http_results,
        meta.allow_local_address_results,
    ) else {
        return;
    };
    if is_duckduckgo_internal_url(&normalized)
        || results.iter().any(|r| r.normalized_url == normalized)
    {
        return;
    }
    let title = collapse_html_whitespace(title);
    if title.is_empty() || is_generic_search_nav_title(&title) {
        return;
    }
    let snippet = collapse_html_whitespace(snippet);
    let content_hash_prefix = search_result_hash_prefix(&clean_url, &title, &snippet);
    results.push(SearchResult {
        url: clean_url,
        normalized_url: normalized,
        title,
        snippet,
        content_hash_prefix,
        provider: provider_name.to_string(),
        rank: results.len() + 1,
        retrieved_at: Utc::now(),
        status: meta.status,
        content_type: meta.content_type.clone(),
        final_url: meta.final_url.clone(),
        response_bytes: meta.response_bytes,
        extraction_method: Some(meta.extraction_method.clone()),
    });
}

fn strip_html_text(raw: &str) -> String {
    let strip_tags = Regex::new(r"(?is)<[^>]+>").expect("regex");
    collapse_html_whitespace(&html_unescape_basic(&strip_tags.replace_all(raw, " ")))
}

fn collapse_html_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_unescape_basic(raw: &str) -> String {
    raw.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn is_duckduckgo_internal_url(normalized_url: &str) -> bool {
    Url::parse(normalized_url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.contains("duckduckgo.com")))
        .unwrap_or(false)
}

fn is_generic_search_nav_title(title: &str) -> bool {
    let normalized = title.trim().to_ascii_lowercase();
    normalized.is_empty()
        || matches!(
            normalized.as_str(),
            "all"
                | "images"
                | "videos"
                | "news"
                | "maps"
                | "shopping"
                | "settings"
                | "feedback"
                | "privacy"
                | "next"
                | "previous"
                | "more"
                | "duckduckgo"
        )
}

#[async_trait]
impl SearchProvider for CustomJsonProvider {
    fn provider_name(&self) -> &'static str {
        "custom_json"
    }

    async fn search(&self, req: &SearchRequest) -> Result<SearchResultSet> {
        let max_results = req.max_results.to_string();
        let mut request = self.client.get(&self.endpoint).query(&[
            ("q", req.query.as_str()),
            ("query", req.query.as_str()),
            ("max_results", max_results.as_str()),
        ]);
        if let Some(freshness) = req.freshness.as_ref()
            && !freshness.trim().is_empty()
        {
            request = request.query(&[("freshness", freshness.as_str())]);
        }
        if let Some(token) = self.bearer_token.as_ref()
            && !token.trim().is_empty()
        {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.context("custom_json request failed")?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let final_url = Some(response.url().to_string());
        let value = response
            .bytes()
            .await
            .context("custom_json response decode failed")?;
        if !status.is_success() {
            anyhow::bail!("custom_json request failed: status {status}");
        }
        let response_bytes = value.len();
        let value: Value =
            serde_json::from_slice(&value).context("custom_json response decode failed")?;
        let rows = collect_rows(&value);
        Ok(results_from_rows(
            rows,
            self.provider_name(),
            req,
            SearchResponseMeta {
                status: Some(status.as_u16()),
                content_type,
                final_url,
                response_bytes: Some(response_bytes),
                extraction_method: "json_array".to_string(),
                allow_plain_http_results: self.allow_plain_http_results,
                allow_local_address_results: self.allow_local_address_results,
            },
        ))
    }
}

#[async_trait]
impl SearchProvider for BraveProvider {
    fn provider_name(&self) -> &'static str {
        "brave"
    }

    async fn search(&self, req: &SearchRequest) -> Result<SearchResultSet> {
        let count = req.max_results.to_string();
        let endpoint = if self.endpoint.trim().is_empty() {
            BRAVE_DEFAULT_ENDPOINT
        } else {
            self.endpoint.as_str()
        };
        let response = self
            .client
            .get(endpoint)
            .query(&[("q", req.query.as_str()), ("count", count.as_str())])
            .header("X-Subscription-Token", self.api_key.trim())
            .header("Accept", "application/json")
            .send()
            .await
            .context("brave request failed")?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let final_url = Some(response.url().to_string());
        let bytes = response.bytes().await.context("brave body read failed")?;
        let response_bytes = Some(bytes.len());
        let value: Value =
            serde_json::from_slice(&bytes).context("brave response decode failed")?;
        if !status.is_success() {
            anyhow::bail!("brave request failed: status {status}");
        }
        let rows = collect_rows(&value);
        Ok(results_from_rows(
            rows,
            self.provider_name(),
            req,
            SearchResponseMeta {
                status: Some(status.as_u16()),
                content_type,
                final_url,
                response_bytes,
                extraction_method: "json_array".to_string(),
                allow_plain_http_results: false,
                allow_local_address_results: false,
            },
        ))
    }
}

#[async_trait]
impl SearchProvider for KagiProvider {
    fn provider_name(&self) -> &'static str {
        "kagi"
    }

    async fn search(&self, req: &SearchRequest) -> Result<SearchResultSet> {
        let limit = req.max_results.to_string();
        let endpoint = if self.endpoint.trim().is_empty() {
            KAGI_DEFAULT_ENDPOINT
        } else {
            self.endpoint.as_str()
        };
        let response = self
            .client
            .get(endpoint)
            .query(&[("q", req.query.as_str()), ("limit", limit.as_str())])
            .header("Authorization", format!("Bot {}", self.api_key.trim()))
            .header("Accept", "application/json")
            .send()
            .await
            .context("kagi request failed")?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let final_url = Some(response.url().to_string());
        let bytes = response.bytes().await.context("kagi body read failed")?;
        let response_bytes = Some(bytes.len());
        let value: Value = serde_json::from_slice(&bytes).context("kagi response decode failed")?;
        if !status.is_success() {
            anyhow::bail!("kagi request failed: status {status}");
        }
        let rows = collect_rows(&value);
        Ok(results_from_rows(
            rows,
            self.provider_name(),
            req,
            SearchResponseMeta {
                status: Some(status.as_u16()),
                content_type,
                final_url,
                response_bytes,
                extraction_method: "json_array".to_string(),
                allow_plain_http_results: false,
                allow_local_address_results: false,
            },
        ))
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn provider_name(&self) -> &'static str {
        "tavily"
    }

    async fn search(&self, req: &SearchRequest) -> Result<SearchResultSet> {
        let endpoint = if self.endpoint.trim().is_empty() {
            TAVILY_DEFAULT_ENDPOINT
        } else {
            self.endpoint.as_str()
        };

        let mut payload = serde_json::json!({
            "query": req.query,
            "max_results": req.max_results,
            "search_depth": "basic",
            "include_answer": false,
            "include_images": false,
            "api_key": self.api_key.trim(),
        });
        if let Some(freshness) = req.freshness.as_ref()
            && !freshness.trim().is_empty()
        {
            payload["time_range"] = Value::String(freshness.clone());
        }
        let response = self
            .client
            .post(endpoint)
            .json(&payload)
            .send()
            .await
            .context("tavily request failed")?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let final_url = Some(response.url().to_string());
        let bytes = response.bytes().await.context("tavily body read failed")?;
        let response_bytes = Some(bytes.len());
        let value: Value =
            serde_json::from_slice(&bytes).context("tavily response decode failed")?;
        if !status.is_success() {
            anyhow::bail!("tavily request failed: status {status}");
        }
        let rows = collect_rows(&value);
        Ok(results_from_rows(
            rows,
            self.provider_name(),
            req,
            SearchResponseMeta {
                status: Some(status.as_u16()),
                content_type,
                final_url,
                response_bytes,
                extraction_method: "json_array".to_string(),
                allow_plain_http_results: false,
                allow_local_address_results: false,
            },
        ))
    }
}

#[async_trait]
impl SearchProvider for WhoogleProvider {
    fn provider_name(&self) -> &'static str {
        "whoogle"
    }

    async fn search(&self, req: &SearchRequest) -> Result<SearchResultSet> {
        let max = req.max_results.to_string();
        let endpoint = if self.endpoint.trim().is_empty() {
            WHOOGLE_DEFAULT_ENDPOINT
        } else {
            self.endpoint.as_str()
        };

        let response = self
            .client
            .get(endpoint)
            .query(&[
                ("q", req.query.as_str()),
                ("format", "json"),
                ("max", max.as_str()),
            ])
            .send()
            .await
            .context("whoogle request failed")?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let final_url = Some(response.url().to_string());
        let bytes = response.bytes().await.context("whoogle body read failed")?;
        let response_bytes = Some(bytes.len());
        let value: Value =
            serde_json::from_slice(&bytes).context("whoogle response decode failed")?;
        if !status.is_success() {
            anyhow::bail!("whoogle request failed: status {status}");
        }
        let rows = collect_rows(&value);
        Ok(results_from_rows(
            rows,
            self.provider_name(),
            req,
            SearchResponseMeta {
                status: Some(status.as_u16()),
                content_type,
                final_url,
                response_bytes,
                extraction_method: "json_array".to_string(),
                allow_plain_http_results: false,
                allow_local_address_results: false,
            },
        ))
    }
}

fn collect_rows(value: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let mut stack = vec![value.clone()];
    let candidate_keys = [
        "results",
        "data",
        "items",
        "web",
        "organic",
        "organic_results",
        "documents",
    ];

    while let Some(node) = stack.pop() {
        match node {
            Value::Array(arr) => {
                if arr
                    .iter()
                    .any(|v| first_string_field(v, &["url", "link", "href"]).is_some())
                {
                    out.extend(arr);
                } else {
                    for item in arr {
                        stack.push(item);
                    }
                }
            }
            Value::Object(map) => {
                for key in candidate_keys {
                    if let Some(v) = map.get(key) {
                        stack.push(v.clone());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn results_from_rows(
    rows: Vec<Value>,
    provider_name: &str,
    req: &SearchRequest,
    meta: SearchResponseMeta,
) -> SearchResultSet {
    let mut results = Vec::new();
    for row in rows {
        if results.len() >= req.max_results {
            break;
        }
        let Some(url) = first_string_field(&row, &["url", "link", "href"]) else {
            continue;
        };
        let Ok(normalized) = normalize_result_url(
            &url,
            meta.allow_plain_http_results,
            meta.allow_local_address_results,
        ) else {
            continue;
        };
        let title = first_string_field(&row, &["title", "name"]).unwrap_or_else(|| url.clone());
        let snippet = first_string_field(
            &row,
            &[
                "snippet",
                "description",
                "content",
                "summary",
                "body",
                "text",
            ],
        )
        .unwrap_or_default();
        let rank = results.len() + 1;
        let content_hash_prefix = search_result_hash_prefix(&url, &title, &snippet);
        results.push(SearchResult {
            url,
            normalized_url: normalized,
            title,
            snippet,
            content_hash_prefix,
            provider: provider_name.to_string(),
            rank,
            retrieved_at: Utc::now(),
            status: meta.status,
            content_type: meta.content_type.clone(),
            final_url: meta.final_url.clone(),
            response_bytes: meta.response_bytes,
            extraction_method: Some(meta.extraction_method.clone()),
        });
    }

    SearchResultSet {
        query: req.query.clone(),
        provider: provider_name.to_string(),
        results,
    }
}

fn first_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = value.get(*key).and_then(|v| v.as_str()) {
            let out = s.trim();
            if !out.is_empty() {
                return Some(out.to_string());
            }
        }
    }
    None
}

fn normalize_result_url(
    raw: &str,
    allow_plain_http: bool,
    allow_local_addresses: bool,
) -> Result<String> {
    let parsed = validate_web_url(raw, allow_plain_http, allow_local_addresses)?;
    let mut normalized = parsed;
    normalized.set_fragment(None);
    Ok(normalized.to_string())
}

fn normalize_ddg_href(raw_href: &str) -> Option<String> {
    let mut href = raw_href.trim().to_string();
    if href.starts_with("//") {
        href = format!("https:{href}");
    }

    let parsed = Url::parse(&href).ok()?;
    let host = parsed.host_str().unwrap_or_default();
    if host.contains("duckduckgo.com")
        && parsed.path().starts_with("/l/")
        && let Some((_, value)) = parsed.query_pairs().find(|(k, _)| k == "uddg")
    {
        return Some(value.to_string());
    }
    Some(href)
}

fn search_result_hash_prefix(url: &str, title: &str, snippet: &str) -> String {
    let payload = format!("{url}\n{title}\n{snippet}");
    let digest = sha2::Sha256::digest(payload.as_bytes());
    let full = format!("{digest:x}");
    full.chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_ddg_redirect_href() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdocs";
        let url = normalize_ddg_href(href).expect("decoded");
        assert_eq!(url, "https://example.com/docs");
    }

    #[test]
    fn parses_duckduckgo_generic_links_when_result_class_is_missing() {
        let html = r#"
            <html><body>
              <a href="/settings">Settings</a>
              <a href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fformulae.brew.sh%2Fformula%2Fjq">jq formula &amp; docs</a>
            </body></html>
        "#;
        let req = SearchRequest {
            query: "homebrew jq".to_string(),
            freshness: None,
            max_results: 3,
        };
        let results = parse_duckduckgo_results(
            html,
            "duckduckgo_html",
            &req,
            SearchResponseMeta {
                status: Some(200),
                content_type: Some("text/html".to_string()),
                final_url: Some("https://lite.duckduckgo.com/lite/?q=homebrew+jq".to_string()),
                response_bytes: Some(html.len()),
                extraction_method: "duckduckgo_lite".to_string(),
                allow_plain_http_results: false,
                allow_local_address_results: false,
            },
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://formulae.brew.sh/formula/jq");
        assert_eq!(results[0].title, "jq formula & docs");
        assert_eq!(
            results[0].extraction_method.as_deref(),
            Some("duckduckgo_lite_generic_links")
        );
    }

    #[test]
    fn parses_custom_json_rows() {
        let row = serde_json::json!({
            "url": "https://example.com",
            "title": "Example",
            "snippet": "hello"
        });
        assert_eq!(
            first_string_field(&row, &["url", "link"]),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            first_string_field(&row, &["title", "name"]),
            Some("Example".to_string())
        );
    }

    #[test]
    fn collects_nested_rows() {
        let payload = serde_json::json!({
            "web": { "results": [{ "url": "https://example.com/a" }] },
            "other": [{ "ignored": true }]
        });
        let rows = collect_rows(&payload);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["url"], "https://example.com/a");
    }

    #[test]
    fn normalizes_search_result_url() {
        let normalized = normalize_result_url("https://example.com/docs#a", false, false)
            .expect("normalize should pass");
        assert_eq!(normalized, "https://example.com/docs");
    }

    #[test]
    fn normalizes_local_custom_result_when_policy_allows_it() {
        let normalized = normalize_result_url("http://127.0.0.1:8080/docs#a", true, true)
            .expect("local result should pass when explicitly allowed");
        assert_eq!(normalized, "http://127.0.0.1:8080/docs");
        assert!(normalize_result_url("http://127.0.0.1:8080/docs", false, false).is_err());
    }
}
