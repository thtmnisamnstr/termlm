use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid enum value for {field}: {value}")]
    InvalidEnum { field: &'static str, value: String },
    #[error("invalid value for {field}: {reason}")]
    InvalidValue { field: &'static str, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub inference: InferenceConfig,
    pub performance: PerformanceConfig,
    pub model: ModelConfig,
    pub ollama: OllamaConfig,
    pub web: WebConfig,
    pub approval: ApprovalConfig,
    pub behavior: BehaviorConfig,
    pub daemon: DaemonConfig,
    pub logging: LoggingConfig,
    pub indexer: IndexerConfig,
    pub capture: CaptureConfig,
    pub terminal_context: TerminalContextConfig,
    pub local_tools: LocalToolsConfig,
    pub git_context: GitContextConfig,
    pub project_metadata: ProjectMetadataConfig,
    pub tool_routing: ToolRoutingConfig,
    pub context_budget: ContextBudgetConfig,
    pub cache: CacheConfig,
    pub source_ledger: SourceLedgerConfig,
    pub debug: DebugConfig,
    pub prompt: PromptConfig,
    pub session: SessionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InferenceConfig {
    pub provider: String,
    pub tool_calling_required: bool,
    pub stream: bool,
    pub token_idle_timeout_secs: u64,
    pub startup_failure_behavior: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceConfig {
    pub profile: String,
    pub warm_core_on_start: bool,
    pub keep_embedding_warm: bool,
    pub prewarm_common_docs: bool,
    pub indexer_priority_mode: String,
    pub max_background_cpu_pct: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub variant: String,
    pub auto_download: bool,
    pub download_only_selected_variant: bool,
    pub models_dir: String,
    pub e4b_filename: String,
    pub e2b_filename: String,
    pub context_tokens: u32,
    pub gpu_layers: i32,
    pub threads: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OllamaConfig {
    pub endpoint: String,
    pub model: String,
    pub options: BTreeMap<String, toml::Value>,
    pub keep_alive: String,
    pub request_timeout_secs: u64,
    pub connect_timeout_secs: u64,
    pub allow_remote: bool,
    pub allow_plain_http_remote: bool,
    pub healthcheck_on_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    pub enabled: bool,
    pub expose_tools: bool,
    pub provider: String,
    pub search_endpoint: String,
    pub search_api_key_env: String,
    pub user_agent: String,
    pub request_timeout_secs: u64,
    pub connect_timeout_secs: u64,
    pub max_results: u32,
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
    pub extract: WebExtractConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebExtractConfig {
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
#[serde(default)]
pub struct ApprovalConfig {
    pub mode: String,
    pub critical_patterns: Vec<String>,
    pub approve_all_resets_per_task: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    pub thinking: bool,
    pub allow_clarifications: bool,
    pub max_tool_rounds: u32,
    pub max_planning_rounds: u32,
    pub context_classifier_enabled: bool,
    pub clarification_timeout_secs: u64,
    pub command_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub socket_path: String,
    pub pid_file: String,
    pub log_file: String,
    pub log_level: String,
    pub shutdown_grace_secs: u64,
    pub boot_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub redact_critical: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexerConfig {
    pub enabled: bool,
    pub concurrency: usize,
    pub max_loadavg: f32,
    pub max_doc_bytes: usize,
    pub max_binaries: usize,
    pub max_chunks: usize,
    pub chunk_max_tokens: usize,
    pub cheatsheet_static_count: usize,
    pub rag_top_k: usize,
    pub rag_min_similarity: f32,
    pub rag_max_tokens: usize,
    pub lookup_max_bytes: usize,
    pub hybrid_retrieval_enabled: bool,
    pub lexical_index_enabled: bool,
    pub lexical_top_k: usize,
    pub exact_command_boost: f32,
    pub exact_flag_boost: f32,
    pub section_boost_options: f32,
    pub command_aware_retrieval: bool,
    pub command_aware_top_k: usize,
    pub validate_command_flags: bool,
    pub embedding_provider: String,
    pub query_embedding_timeout_secs: u64,
    pub embed_filename: String,
    pub embed_dim: usize,
    pub embed_query_prefix: String,
    pub embed_doc_prefix: String,
    pub ollama_embed_model: String,
    pub extra_paths: Vec<String>,
    pub ignore_paths: Vec<String>,
    pub fsevents_debounce_ms: u64,
    pub disk_write_coalesce_secs: u64,
    pub vector_storage: String,
    pub lexical_index_impl: String,
    pub priority_indexing: bool,
    pub priority_recent_commands: bool,
    pub priority_prompt_commands: bool,
    pub cache_retrieval_results: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub enabled: bool,
    pub max_bytes: usize,
    pub redact_env: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalContextConfig {
    pub enabled: bool,
    pub capture_all_interactive_commands: bool,
    pub capture_command_output: bool,
    pub max_entries: usize,
    pub max_output_bytes_per_command: usize,
    pub recent_context_max_tokens: usize,
    pub older_context_max_tokens: usize,
    pub redact_secrets: bool,
    pub exclude_tui_commands: bool,
    pub exclude_command_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalToolsConfig {
    pub enabled: bool,
    pub redact_secrets: bool,
    pub default_max_bytes: usize,
    pub max_file_bytes: usize,
    pub max_search_results: usize,
    pub max_search_files: usize,
    pub max_workspace_entries: usize,
    pub respect_gitignore: bool,
    pub workspace_markers: Vec<String>,
    pub allow_home_as_workspace: bool,
    pub allow_system_dirs: bool,
    pub sensitive_path_allowlist: Vec<String>,
    pub include_hidden_by_default: bool,
    pub text_detection: LocalToolsTextDetectionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalToolsTextDetectionConfig {
    pub mode: String,
    pub sample_bytes: usize,
    pub reject_nul_bytes: bool,
    pub accepted_encodings: Vec<String>,
    pub deny_binary_magic: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GitContextConfig {
    pub enabled: bool,
    pub max_changed_files: usize,
    pub max_recent_commits: usize,
    pub include_diff_summary: bool,
    pub max_diff_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectMetadataConfig {
    pub enabled: bool,
    pub max_files_read: usize,
    pub max_bytes_per_file: usize,
    pub detect_scripts: bool,
    pub detect_package_managers: bool,
    pub detect_ci: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolRoutingConfig {
    pub dynamic_exposure_enabled: bool,
    pub always_expose_execute: bool,
    pub always_expose_lookup_docs: bool,
    pub expose_web_only_when_needed: bool,
    pub expose_terminal_context_only_when_needed: bool,
    pub expose_file_tools_for_local_questions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextBudgetConfig {
    pub enabled: bool,
    pub max_total_context_tokens: usize,
    pub reserve_response_tokens: usize,
    pub current_question_tokens: usize,
    pub recent_terminal_tokens: usize,
    pub older_session_tokens: usize,
    pub local_tool_result_tokens: usize,
    pub project_git_metadata_tokens: usize,
    pub docs_rag_tokens: usize,
    pub web_result_tokens: usize,
    pub cheat_sheet_tokens: usize,
    pub trim_strategy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    pub enabled: bool,
    pub retrieval_cache_ttl_secs: u64,
    pub command_validation_cache_ttl_secs: u64,
    pub project_metadata_cache_ttl_secs: u64,
    pub git_context_cache_ttl_secs: u64,
    pub file_read_cache_ttl_secs: u64,
    pub web_cache_ttl_secs: u64,
    pub max_total_cache_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceLedgerConfig {
    pub enabled: bool,
    pub expose_on_status: bool,
    pub include_in_debug_logs: bool,
    pub max_refs_on_status: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugConfig {
    pub retrieval_trace_enabled: bool,
    pub retrieval_trace_dir: String,
    pub retrieval_trace_max_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptConfig {
    pub indicator: String,
    pub session_indicator: String,
    pub use_color: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub context_window_tokens: u32,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            provider: "local".to_string(),
            tool_calling_required: true,
            stream: true,
            token_idle_timeout_secs: 30,
            startup_failure_behavior: "fail".to_string(),
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            profile: "performance".to_string(),
            warm_core_on_start: true,
            keep_embedding_warm: true,
            prewarm_common_docs: true,
            indexer_priority_mode: "usage".to_string(),
            max_background_cpu_pct: 200,
        }
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            variant: "E4B".to_string(),
            auto_download: true,
            download_only_selected_variant: true,
            models_dir: "~/.local/share/termlm/models".to_string(),
            e4b_filename: "gemma-4-E4B-it-Q4_K_M.gguf".to_string(),
            e2b_filename: "gemma-4-E2B-it-Q4_K_M.gguf".to_string(),
            context_tokens: 8192,
            gpu_layers: -1,
            threads: 0,
        }
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:11434".to_string(),
            model: "gemma4:e4b".to_string(),
            options: BTreeMap::new(),
            keep_alive: "5m".to_string(),
            request_timeout_secs: 300,
            connect_timeout_secs: 3,
            allow_remote: false,
            allow_plain_http_remote: false,
            healthcheck_on_start: true,
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            expose_tools: true,
            provider: "duckduckgo_html".to_string(),
            search_endpoint: String::new(),
            search_api_key_env: String::new(),
            user_agent: "termlm/0.1 (+https://github.com/thtmnisamnstr/termlm)".to_string(),
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
            extract: WebExtractConfig::default(),
        }
    }
}

impl Default for WebExtractConfig {
    fn default() -> Self {
        Self {
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
        }
    }
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            mode: "manual".to_string(),
            critical_patterns: vec![
                r"^\s*sudo\b".to_string(),
                r"\brm\s+-[a-zA-Z]*r".to_string(),
                r"\bgit\s+(push\s+--force|push\s+-f|reset\s+--hard|clean\s+-fdx)".to_string(),
                r"\b(curl|wget)\b.*\|\s*(sh|bash|zsh)".to_string(),
                r">\s*/dev/(disk|sd|nvme|rdisk)".to_string(),
                r"\bchmod\s+(-R\s+)?777\b".to_string(),
                r"\bchown\s+-R\b".to_string(),
                r"\bmv\s+.*\s+/dev/null\b".to_string(),
                r"\bdrop\s+(table|database)\b".to_string(),
                r"\bkillall?\b".to_string(),
                r"\bdocker\s+system\s+prune".to_string(),
                r"\bbrew\s+uninstall\s+--force".to_string(),
            ],
            approve_all_resets_per_task: true,
        }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            thinking: false,
            allow_clarifications: true,
            max_tool_rounds: 8,
            max_planning_rounds: 3,
            context_classifier_enabled: true,
            clarification_timeout_secs: 120,
            command_timeout_secs: 300,
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: "$XDG_RUNTIME_DIR/termlm.sock".to_string(),
            pid_file: "$XDG_RUNTIME_DIR/termlm.pid".to_string(),
            log_file: "~/.local/state/termlm/termlm.log".to_string(),
            log_level: "info".to_string(),
            shutdown_grace_secs: 5,
            boot_timeout_secs: 60,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            redact_critical: true,
        }
    }
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            concurrency: 8,
            max_loadavg: 4.0,
            max_doc_bytes: 262_144,
            max_binaries: 10_000,
            max_chunks: 100_000,
            chunk_max_tokens: 512,
            cheatsheet_static_count: 150,
            rag_top_k: 5,
            rag_min_similarity: 0.30,
            rag_max_tokens: 3_000,
            lookup_max_bytes: 8_192,
            hybrid_retrieval_enabled: true,
            lexical_index_enabled: true,
            lexical_top_k: 50,
            exact_command_boost: 2.0,
            exact_flag_boost: 1.0,
            section_boost_options: 0.25,
            command_aware_retrieval: true,
            command_aware_top_k: 8,
            validate_command_flags: true,
            embedding_provider: "local".to_string(),
            query_embedding_timeout_secs: 4,
            embed_filename: "bge-small-en-v1.5.Q4_K_M.gguf".to_string(),
            embed_dim: 384,
            embed_query_prefix: "Represent this sentence for searching relevant passages: "
                .to_string(),
            embed_doc_prefix: String::new(),
            ollama_embed_model: "nomic-embed-text".to_string(),
            extra_paths: Vec::new(),
            ignore_paths: Vec::new(),
            fsevents_debounce_ms: 500,
            disk_write_coalesce_secs: 30,
            vector_storage: "f16".to_string(),
            lexical_index_impl: "embedded".to_string(),
            priority_indexing: true,
            priority_recent_commands: true,
            priority_prompt_commands: true,
            cache_retrieval_results: true,
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_bytes: 16_384,
            redact_env: vec![
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "GITHUB_TOKEN".to_string(),
                "OPENAI_API_KEY".to_string(),
            ],
        }
    }
}

impl Default for TerminalContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            capture_all_interactive_commands: true,
            capture_command_output: false,
            max_entries: 50,
            max_output_bytes_per_command: 32_768,
            recent_context_max_tokens: 6_000,
            older_context_max_tokens: 4_000,
            redact_secrets: true,
            exclude_tui_commands: true,
            exclude_command_patterns: vec![
                r"^\s*(env|printenv)(\s|$)".to_string(),
                r"^\s*security\s+find-.*password".to_string(),
                r"^\s*(op|pass)\s+.*(show|get)".to_string(),
                r"^\s*gcloud\s+auth\s+print-access-token".to_string(),
                r"^\s*aws\s+configure\s+get".to_string(),
            ],
        }
    }
}

impl Default for LocalToolsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            redact_secrets: true,
            default_max_bytes: 65_536,
            max_file_bytes: 1_048_576,
            max_search_results: 100,
            max_search_files: 20_000,
            max_workspace_entries: 500,
            respect_gitignore: true,
            workspace_markers: Vec::new(),
            allow_home_as_workspace: false,
            allow_system_dirs: false,
            sensitive_path_allowlist: Vec::new(),
            include_hidden_by_default: false,
            text_detection: LocalToolsTextDetectionConfig::default(),
        }
    }
}

impl Default for LocalToolsTextDetectionConfig {
    fn default() -> Self {
        Self {
            mode: "content".to_string(),
            sample_bytes: 8_192,
            reject_nul_bytes: true,
            accepted_encodings: vec![
                "utf-8".to_string(),
                "utf-16le".to_string(),
                "utf-16be".to_string(),
            ],
            deny_binary_magic: true,
        }
    }
}

impl Default for GitContextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_changed_files: 200,
            max_recent_commits: 10,
            include_diff_summary: true,
            max_diff_bytes: 12_000,
        }
    }
}

impl Default for ProjectMetadataConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_files_read: 50,
            max_bytes_per_file: 65_536,
            detect_scripts: true,
            detect_package_managers: true,
            detect_ci: true,
        }
    }
}

impl Default for ToolRoutingConfig {
    fn default() -> Self {
        Self {
            dynamic_exposure_enabled: true,
            always_expose_execute: true,
            always_expose_lookup_docs: true,
            expose_web_only_when_needed: true,
            expose_terminal_context_only_when_needed: true,
            expose_file_tools_for_local_questions: true,
        }
    }
}

impl Default for ContextBudgetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_total_context_tokens: 8_192,
            reserve_response_tokens: 1_024,
            current_question_tokens: 1_024,
            recent_terminal_tokens: 5_000,
            older_session_tokens: 2_500,
            local_tool_result_tokens: 5_000,
            project_git_metadata_tokens: 2_500,
            docs_rag_tokens: 3_000,
            web_result_tokens: 3_000,
            cheat_sheet_tokens: 5_500,
            trim_strategy: "priority_newest_first".to_string(),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retrieval_cache_ttl_secs: 300,
            command_validation_cache_ttl_secs: 300,
            project_metadata_cache_ttl_secs: 60,
            git_context_cache_ttl_secs: 10,
            file_read_cache_ttl_secs: 30,
            web_cache_ttl_secs: 900,
            max_total_cache_bytes: 100 * 1024 * 1024,
        }
    }
}

impl Default for SourceLedgerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            expose_on_status: true,
            include_in_debug_logs: true,
            max_refs_on_status: 32,
        }
    }
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            retrieval_trace_enabled: false,
            retrieval_trace_dir: "~/.local/state/termlm/retrieval-traces".to_string(),
            retrieval_trace_max_files: 25,
        }
    }
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            indicator: "?> ".to_string(),
            session_indicator: "?? ".to_string(),
            use_color: true,
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            context_window_tokens: 32_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadClass {
    HotReload,
    RestartRequired,
}

pub struct LoadedConfig {
    pub config: AppConfig,
    pub warnings: Vec<String>,
}

pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".config/termlm/config.toml")
}

pub fn load_or_create(path: Option<&Path>) -> Result<LoadedConfig> {
    let path = path.map(PathBuf::from).unwrap_or_else(default_config_path);

    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(&AppConfig::default())?;
        fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: toml::Value =
        toml::from_str(&raw).with_context(|| format!("invalid TOML in {}", path.display()))?;

    let warnings = warn_unknown_keys(&value);
    let cfg: AppConfig = value
        .try_into()
        .with_context(|| format!("schema parse failed for {}", path.display()))?;
    validate(&cfg)?;

    Ok(LoadedConfig {
        config: cfg,
        warnings,
    })
}

pub fn reload_class_for_key(path: &str) -> ReloadClass {
    let restart_required_prefixes = [
        "model.",
        "inference.provider",
        "ollama.endpoint",
        "performance.profile",
        "indexer.embed_filename",
        "indexer.embed_dim",
        "indexer.vector_storage",
        "indexer.embedding_provider",
        "indexer.lexical_index_impl",
        "indexer.embed_query_prefix",
        "indexer.embed_doc_prefix",
        "web.provider",
    ];

    if restart_required_prefixes
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(prefix))
    {
        ReloadClass::RestartRequired
    } else {
        ReloadClass::HotReload
    }
}

pub fn validate(cfg: &AppConfig) -> Result<()> {
    if !matches!(cfg.inference.provider.as_str(), "local" | "ollama") {
        return Err(ConfigError::InvalidEnum {
            field: "inference.provider",
            value: cfg.inference.provider.clone(),
        }
        .into());
    }

    if !matches!(cfg.inference.startup_failure_behavior.as_str(), "fail") {
        return Err(ConfigError::InvalidEnum {
            field: "inference.startup_failure_behavior",
            value: cfg.inference.startup_failure_behavior.clone(),
        }
        .into());
    }

    if !matches!(
        cfg.performance.profile.as_str(),
        "eco" | "balanced" | "performance"
    ) {
        return Err(ConfigError::InvalidEnum {
            field: "performance.profile",
            value: cfg.performance.profile.clone(),
        }
        .into());
    }

    if !matches!(
        cfg.performance.indexer_priority_mode.as_str(),
        "usage" | "path_order"
    ) {
        return Err(ConfigError::InvalidEnum {
            field: "performance.indexer_priority_mode",
            value: cfg.performance.indexer_priority_mode.clone(),
        }
        .into());
    }

    if !matches!(
        cfg.approval.mode.as_str(),
        "manual" | "manual_critical" | "auto"
    ) {
        return Err(ConfigError::InvalidEnum {
            field: "approval.mode",
            value: cfg.approval.mode.clone(),
        }
        .into());
    }

    if !matches!(
        cfg.web.provider.as_str(),
        "duckduckgo_html" | "custom_json" | "brave" | "kagi" | "tavily" | "whoogle" | "none"
    ) {
        return Err(ConfigError::InvalidEnum {
            field: "web.provider",
            value: cfg.web.provider.clone(),
        }
        .into());
    }

    if cfg.web.provider == "custom_json" && cfg.web.search_endpoint.trim().is_empty() {
        return Err(ConfigError::InvalidValue {
            field: "web.search_endpoint",
            reason: format!("must be set when web.provider is {}", cfg.web.provider),
        }
        .into());
    }

    if matches!(cfg.web.provider.as_str(), "brave" | "kagi" | "tavily")
        && cfg.web.search_api_key_env.trim().is_empty()
    {
        return Err(ConfigError::InvalidValue {
            field: "web.search_api_key_env",
            reason: format!("must be set when web.provider is {}", cfg.web.provider),
        }
        .into());
    }

    if !cfg.web.search_api_key_env.trim().is_empty()
        && !cfg
            .web
            .search_api_key_env
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(ConfigError::InvalidValue {
            field: "web.search_api_key_env",
            reason: "must be a valid environment variable name".to_string(),
        }
        .into());
    }

    if !cfg.web.search_endpoint.trim().is_empty() {
        let parsed = url::Url::parse(cfg.web.search_endpoint.trim()).map_err(|_| {
            ConfigError::InvalidValue {
                field: "web.search_endpoint",
                reason: "must be a valid absolute URL".to_string(),
            }
        })?;
        match parsed.scheme() {
            "https" => {}
            "http" if cfg.web.allow_plain_http => {}
            "http" => {
                return Err(ConfigError::InvalidValue {
                    field: "web.search_endpoint",
                    reason: "http endpoint requires web.allow_plain_http=true".to_string(),
                }
                .into());
            }
            _ => {
                return Err(ConfigError::InvalidValue {
                    field: "web.search_endpoint",
                    reason: "unsupported URL scheme".to_string(),
                }
                .into());
            }
        }
    }

    if cfg
        .web
        .allowed_schemes
        .iter()
        .any(|s| !matches!(s.as_str(), "https" | "http"))
    {
        return Err(ConfigError::InvalidValue {
            field: "web.allowed_schemes",
            reason: "allowed values are http/https".to_string(),
        }
        .into());
    }

    if !matches!(cfg.web.extract.output_format.as_str(), "markdown") {
        return Err(ConfigError::InvalidEnum {
            field: "web.extract.output_format",
            value: cfg.web.extract.output_format.clone(),
        }
        .into());
    }
    if !matches!(
        cfg.web.extract.strategy.as_str(),
        "auto" | "semantic_selector" | "readability" | "clean_full_page"
    ) {
        return Err(ConfigError::InvalidEnum {
            field: "web.extract.strategy",
            value: cfg.web.extract.strategy.clone(),
        }
        .into());
    }
    if cfg.web.extract.include_images {
        return Err(ConfigError::InvalidValue {
            field: "web.extract.include_images",
            reason: "v1 supports markdown extraction only; images must remain disabled".to_string(),
        }
        .into());
    }
    if cfg.web.extract.max_table_rows == 0 || cfg.web.extract.max_table_cols == 0 {
        return Err(ConfigError::InvalidValue {
            field: "web.extract.max_table_rows/max_table_cols",
            reason: "table row/column limits must be >= 1".to_string(),
        }
        .into());
    }
    if cfg.web.extract.max_markdown_bytes == 0 || cfg.web.extract.max_html_bytes == 0 {
        return Err(ConfigError::InvalidValue {
            field: "web.extract.max_html_bytes/max_markdown_bytes",
            reason: "extraction byte caps must be >= 1".to_string(),
        }
        .into());
    }

    if !matches!(cfg.indexer.vector_storage.as_str(), "f16" | "f32") {
        return Err(ConfigError::InvalidEnum {
            field: "indexer.vector_storage",
            value: cfg.indexer.vector_storage.clone(),
        }
        .into());
    }

    if !matches!(cfg.indexer.embedding_provider.as_str(), "local" | "ollama") {
        return Err(ConfigError::InvalidEnum {
            field: "indexer.embedding_provider",
            value: cfg.indexer.embedding_provider.clone(),
        }
        .into());
    }

    if cfg.indexer.query_embedding_timeout_secs == 0 {
        return Err(ConfigError::InvalidValue {
            field: "indexer.query_embedding_timeout_secs",
            reason: "must be >= 1".to_string(),
        }
        .into());
    }

    if !matches!(cfg.indexer.lexical_index_impl.as_str(), "embedded") {
        return Err(ConfigError::InvalidEnum {
            field: "indexer.lexical_index_impl",
            value: cfg.indexer.lexical_index_impl.clone(),
        }
        .into());
    }

    if !matches!(
        cfg.context_budget.trim_strategy.as_str(),
        "priority_newest_first"
    ) {
        return Err(ConfigError::InvalidEnum {
            field: "context_budget.trim_strategy",
            value: cfg.context_budget.trim_strategy.clone(),
        }
        .into());
    }

    if !matches!(
        cfg.local_tools.text_detection.mode.as_str(),
        "content" | "binary_magic"
    ) {
        return Err(ConfigError::InvalidEnum {
            field: "local_tools.text_detection.mode",
            value: cfg.local_tools.text_detection.mode.clone(),
        }
        .into());
    }

    if cfg.model.context_tokens == 0 {
        return Err(ConfigError::InvalidValue {
            field: "model.context_tokens",
            reason: "must be > 0".to_string(),
        }
        .into());
    }

    if cfg.debug.retrieval_trace_enabled && cfg.debug.retrieval_trace_dir.trim().is_empty() {
        return Err(ConfigError::InvalidValue {
            field: "debug.retrieval_trace_dir",
            reason: "must be set when retrieval tracing is enabled".to_string(),
        }
        .into());
    }

    Ok(())
}

fn warn_unknown_keys(value: &toml::Value) -> Vec<String> {
    let schema = toml::Value::try_from(AppConfig::default())
        .unwrap_or(toml::Value::Table(toml::map::Map::new()));
    let mut out = Vec::new();
    collect_unknown_keys("", value, &schema, &mut out);
    out
}

fn collect_unknown_keys(
    path: &str,
    value: &toml::Value,
    schema: &toml::Value,
    out: &mut Vec<String>,
) {
    let Some(table) = value.as_table() else {
        return;
    };
    let schema_table = schema.as_table();
    for (key, nested) in table {
        let next_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        if is_dynamic_key_path(&next_path) {
            continue;
        }

        match schema_table.and_then(|t| t.get(key)) {
            Some(schema_nested) => collect_unknown_keys(&next_path, nested, schema_nested, out),
            None => out.push(format!("unknown config key: {next_path}")),
        }
    }
}

fn is_dynamic_key_path(path: &str) -> bool {
    path == "inference.options"
        || path.starts_with("inference.options.")
        || path == "ollama.options"
        || path.starts_with("ollama.options.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        validate(&AppConfig::default()).expect("defaults are valid");
    }

    #[test]
    fn defaults_enable_web_tools_without_user_config() {
        let cfg = AppConfig::default();
        assert!(cfg.web.enabled);
        assert!(cfg.web.expose_tools);
        assert_eq!(cfg.web.provider, "duckduckgo_html");
        validate(&cfg).expect("default web config is valid");
    }

    #[test]
    fn missing_web_section_still_defaults_to_enabled() {
        let raw = r#"
[approval]
mode = "manual"
"#;
        let cfg: AppConfig = toml::from_str(raw).expect("parse partial config");
        assert!(cfg.web.enabled);
        assert!(cfg.web.expose_tools);
        assert_eq!(cfg.web.provider, "duckduckgo_html");
        validate(&cfg).expect("partial config with default web is valid");
    }

    #[test]
    fn restart_required_key_classification() {
        assert!(matches!(
            reload_class_for_key("inference.provider"),
            ReloadClass::RestartRequired
        ));
        assert!(matches!(
            reload_class_for_key("approval.mode"),
            ReloadClass::HotReload
        ));
    }

    #[test]
    fn token_web_providers_require_api_key_env() {
        let mut cfg = AppConfig::default();
        cfg.web.provider = "brave".to_string();
        cfg.web.search_api_key_env = String::new();
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn warn_unknown_keys_reports_nested_paths() {
        let raw = r#"
[inference]
provider = "local"
tool_calling_required = true
stream = true
token_idle_timeout_secs = 30
startup_failure_behavior = "fail"
unknown_nested = true

[web]
enabled = true
provider = "duckduckgo_html"

[web.extract]
output_format = "markdown"
unknown_inner = 1
"#;
        let value: toml::Value = toml::from_str(raw).expect("parse");
        let warnings = warn_unknown_keys(&value);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("inference.unknown_nested"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("web.extract.unknown_inner"))
        );
    }

    #[test]
    fn canonical_config_keys_are_recognized() {
        let raw = r#"
[model]
download_only_selected_variant = true

[ollama]
keep_alive = "5m"

[indexer]
rag_max_tokens = 3000
hybrid_retrieval_enabled = true
lexical_index_enabled = true
lexical_top_k = 50
exact_command_boost = 2.0
exact_flag_boost = 1.0
section_boost_options = 0.25
command_aware_retrieval = true
command_aware_top_k = 8
validate_command_flags = true
query_embedding_timeout_secs = 4

[terminal_context]
recent_context_max_tokens = 6000
older_context_max_tokens = 4000
capture_command_output = false

[tool_routing]
expose_terminal_context_only_when_needed = true
expose_file_tools_for_local_questions = true

[context_budget]
trim_strategy = "priority_newest_first"
local_tool_result_tokens = 5000
project_git_metadata_tokens = 2500
docs_rag_tokens = 3000
web_result_tokens = 3000

[source_ledger]
include_in_debug_logs = true

[debug]
retrieval_trace_enabled = false
retrieval_trace_max_files = 25

[prompt]
use_color = true
"#;
        let value: toml::Value = toml::from_str(raw).expect("parse");
        let warnings = warn_unknown_keys(&value);
        assert!(
            warnings.is_empty(),
            "expected canonical keys to be recognized, got: {warnings:?}"
        );
    }

    #[test]
    fn ollama_options_table_allows_dynamic_provider_keys() {
        let raw = r#"
[ollama]
model = "gemma4:e4b"

[ollama.options]
temperature = 0.1
num_ctx = 8192
repeat_penalty = 1.05
"#;
        let value: toml::Value = toml::from_str(raw).expect("parse");
        let warnings = warn_unknown_keys(&value);
        assert!(
            warnings.is_empty(),
            "expected [ollama.options] to be treated as dynamic, got: {warnings:?}"
        );
    }

    #[test]
    fn custom_json_requires_endpoint() {
        let mut cfg = AppConfig::default();
        cfg.web.provider = "custom_json".to_string();
        cfg.web.search_endpoint = String::new();
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn startup_failure_behavior_v1_requires_fail() {
        let mut cfg = AppConfig::default();
        cfg.inference.startup_failure_behavior = "continue".to_string();
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn http_search_endpoint_requires_allow_plain_http() {
        let mut cfg = AppConfig::default();
        cfg.web.provider = "custom_json".to_string();
        cfg.web.search_endpoint = "http://example.com/search".to_string();
        cfg.web.allow_plain_http = false;
        assert!(validate(&cfg).is_err());
    }
}
