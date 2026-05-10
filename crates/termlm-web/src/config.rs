use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebExtractRuntimeConfig {
    pub strategy: String,
    pub output_format: String,
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
pub struct WebRuntimeConfig {
    pub enabled: bool,
    pub provider: String,
    pub search_endpoint: String,
    pub search_api_key_env: String,
    pub request_timeout_secs: u64,
    pub connect_timeout_secs: u64,
    pub max_results: usize,
    pub max_fetch_bytes: usize,
    pub max_pages_per_task: usize,
    pub cache_ttl_secs: u64,
    pub cache_max_bytes: usize,
    pub allowed_schemes: Vec<String>,
    pub allow_plain_http: bool,
    pub allow_local_addresses: bool,
    pub obey_robots_txt: bool,
    pub citation_required: bool,
    pub freshness_required_terms: Vec<String>,
    pub min_delay_between_requests_ms: u64,
    pub search_cache_ttl_secs: u64,
    pub user_agent: String,
    pub extract: WebExtractRuntimeConfig,
}

impl Default for WebRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: "duckduckgo_html".to_string(),
            search_endpoint: String::new(),
            search_api_key_env: String::new(),
            request_timeout_secs: 10,
            connect_timeout_secs: 3,
            max_results: 8,
            max_fetch_bytes: 2 * 1024 * 1024,
            max_pages_per_task: 5,
            cache_ttl_secs: 900,
            cache_max_bytes: 50 * 1024 * 1024,
            allowed_schemes: vec!["https".to_string()],
            allow_plain_http: false,
            allow_local_addresses: false,
            obey_robots_txt: true,
            citation_required: true,
            freshness_required_terms: vec![
                "latest".to_string(),
                "current".to_string(),
                "today".to_string(),
                "now".to_string(),
                "recent".to_string(),
                "new".to_string(),
                "release".to_string(),
                "version".to_string(),
            ],
            min_delay_between_requests_ms: 1500,
            search_cache_ttl_secs: 900,
            user_agent: "termlm/0.1 (+https://example.invalid/termlm)".to_string(),
            extract: WebExtractRuntimeConfig {
                strategy: "auto".to_string(),
                output_format: "markdown".to_string(),
                include_images: false,
                include_links: true,
                include_tables: true,
                max_table_rows: 20,
                max_table_cols: 6,
                preserve_code_blocks: true,
                strip_tracking_params: true,
                max_html_bytes: 1_048_576,
                max_markdown_bytes: 65_536,
                min_extracted_chars: 400,
                dedupe_boilerplate: true,
            },
        }
    }
}
