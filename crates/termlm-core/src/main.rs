mod cache;
mod context;
mod ipc;
mod performance;
mod planning;
mod provider_bootstrap;
mod shell_registry;
mod source_ledger;
mod system_prompt;
mod tasks;

use anyhow::{Context, Result, anyhow, bail};
use arc_swap::ArcSwap;
use base64::Engine;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use performance::PerformanceProfile;
#[cfg(test)]
use provider_bootstrap::*;
use provider_bootstrap::{
    ensure_required_model_assets, is_loopback_endpoint, resolve_models_dir, validate_provider_boot,
};
use sha2::Digest;
use shell_registry::{ShellRegistry, ShellSession};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Component;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use termlm_config::{
    AppConfig, ReloadClass, default_config_path, load_or_create, reload_class_for_key,
};
use termlm_indexer::{
    Chunk, Chunker, HybridRetriever, IndexManifest, IndexStore, LayoutWriteArtifacts,
    RetrievalQuery, discover_binaries_with_stats, lookup_command_docs,
};
use termlm_inference::{
    ChatMessage, ChatRequest, InferenceProvider, LocalLlamaProvider, OllamaProvider,
    ProviderCapabilities, ProviderEvent, StructuredOutputMode, ToolSchema,
    tool_parser::{parse_json_tool_call, parse_tagged_tool_calls},
};
use termlm_protocol::{
    ClientMessage, ErrorKind, IndexProgress, ProposedCommand, RetrievedChunk, ServerMessage,
    ShellCapabilities, ShellKind, StatusSourceRef, TaskCompleteReason, UserDecision, WebStatus,
};
use termlm_safety::{CriticalMatcher, matches_safety_floor, parse_command};
use termlm_web::{
    DEFAULT_MAX_REDIRECTS, SearchRequest, WebReadRequest, WebReadResponse,
    cache::WebCache,
    config::{WebExtractRuntimeConfig, WebRuntimeConfig},
    fetch::{web_read, web_read_redirect_policy},
    search::{
        BraveProvider, CustomJsonProvider, DuckDuckGoHtmlProvider, KagiProvider, SearchProvider,
        SearchResultSet, TavilyProvider, WhoogleProvider,
    },
};
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

static LOG_GUARD: std::sync::Mutex<Option<tracing_appender::non_blocking::WorkerGuard>> =
    std::sync::Mutex::new(None);

#[derive(Debug, Clone)]
struct InFlightTask {
    task_id: Uuid,
    shell_id: Uuid,
    mode: String,
    original_prompt: String,
    proposed_command: String,
    classification: tasks::TaskClassification,
    classification_confidence: f32,
    approval_override: bool,
    awaiting_clarification: bool,
    provider_continuation: bool,
    tool_round: u32,
    created_at: std::time::Instant,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TaskTimeoutKind {
    Clarification,
    Command,
}

impl TaskTimeoutKind {
    fn summary(self) -> &'static str {
        match self {
            Self::Clarification => "Task ended: clarification timeout elapsed.",
            Self::Command => "Task ended: command timeout elapsed.",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TimedOutTask {
    at: std::time::Instant,
    kind: TaskTimeoutKind,
}

#[derive(Debug, Clone)]
struct CachedToolResult {
    rendered: String,
    refs: Vec<source_ledger::SourceRef>,
}

#[derive(Debug, Clone)]
struct CommandPlan {
    cmd: String,
    rationale: String,
    intent: String,
    expected_effect: String,
    commands_used: Vec<String>,
}

#[derive(Debug, Clone)]
struct IndexedBinary {
    name: String,
    path: String,
    mtime_secs: i64,
    size: u64,
    inode: u64,
}

#[derive(Debug, Clone)]
struct SyntheticDocSpec {
    extraction_method: String,
    doc_text: String,
}

#[derive(Debug, Clone, Default)]
struct IndexUpdateSummary {
    added: Vec<String>,
    updated: Vec<String>,
    removed: Vec<String>,
}

impl IndexUpdateSummary {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.removed.is_empty()
    }
}

#[derive(Debug, Clone)]
struct ObservedEntry {
    shell_id: Uuid,
    command_seq: u64,
    command: String,
    cwd: String,
    started_at: chrono::DateTime<chrono::Utc>,
    duration_ms: u64,
    exit_code: i32,
    output_capture_status: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
    redactions_applied: bool,
    detected_error_lines: Vec<String>,
    detected_paths: Vec<String>,
    detected_urls: Vec<String>,
    detected_commands: Vec<String>,
    stdout_head: String,
    stdout_tail: String,
    stderr_head: String,
    stderr_tail: String,
    stdout_full_ref: Option<String>,
    stderr_full_ref: Option<String>,
}

#[derive(Debug, Clone)]
struct SessionTurn {
    user: String,
    assistant: String,
}

#[derive(Debug, Clone, Default)]
struct SessionConversation {
    system_prompt: String,
    turns: VecDeque<SessionTurn>,
}

#[derive(Debug, Default)]
struct IndexRuntime {
    chunks: Vec<Chunk>,
    retriever: HybridRetriever,
    binaries: Vec<IndexedBinary>,
    uses_external_embeddings: bool,
    revision: u64,
}

#[derive(Debug, Default)]
struct EmbeddingRuntime {
    local_provider: Option<LocalLlamaProvider>,
    provider_signature: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ProviderUsageSnapshot {
    prompt_tokens: u64,
    completion_tokens: u64,
    reported: bool,
}

enum ProviderRuntime {
    Local(LocalLlamaProvider),
    Ollama(OllamaProvider),
}

impl std::fmt::Debug for ProviderRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(_) => write!(f, "ProviderRuntime::Local"),
            Self::Ollama(_) => write!(f, "ProviderRuntime::Ollama"),
        }
    }
}

impl ProviderRuntime {
    async fn load_or_connect(&mut self) -> Result<()> {
        match self {
            Self::Local(p) => p.load_or_connect().await,
            Self::Ollama(p) => p.load_or_connect().await,
        }
    }

    async fn capabilities(&self) -> Result<ProviderCapabilities> {
        match self {
            Self::Local(p) => p.capabilities().await,
            Self::Ollama(p) => p.capabilities().await,
        }
    }

    async fn health(&self) -> Result<termlm_inference::ProviderHealth> {
        match self {
            Self::Local(p) => p.health().await,
            Self::Ollama(p) => p.health().await,
        }
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<termlm_inference::ProviderStream> {
        match self {
            Self::Local(p) => p.chat_stream(request).await,
            Self::Ollama(p) => p.chat_stream(request).await,
        }
    }

    async fn cancel(&self, task_id: &str) -> Result<()> {
        match self {
            Self::Local(p) => p.cancel(task_id).await,
            Self::Ollama(p) => p.cancel(task_id).await,
        }
    }

    async fn shutdown(&self) -> Result<()> {
        match self {
            Self::Local(p) => p.shutdown().await,
            Self::Ollama(p) => p.shutdown().await,
        }
    }

    async fn apply_runtime_config(&mut self, cfg: &AppConfig) -> Result<()> {
        match self {
            Self::Local(_) => Ok(()),
            Self::Ollama(p) => {
                p.model = cfg.ollama.model.clone();
                p.keep_alive = if cfg.ollama.keep_alive.trim().is_empty() {
                    None
                } else {
                    Some(cfg.ollama.keep_alive.clone())
                };
                p.allow_remote = cfg.ollama.allow_remote;
                p.allow_plain_http_remote = cfg.ollama.allow_plain_http_remote;
                p.connect_timeout_secs = cfg.ollama.connect_timeout_secs;
                p.request_timeout_secs = cfg.ollama.request_timeout_secs;
                p.load_or_connect().await
            }
        }
    }
}

#[derive(Debug)]
struct DaemonState {
    config: ArcSwap<AppConfig>,
    config_path: PathBuf,
    sandbox_cwd: Option<PathBuf>,
    registry: Mutex<ShellRegistry>,
    detached_contexts: Mutex<BTreeMap<Uuid, termlm_protocol::ShellContext>>,
    tasks: Mutex<BTreeMap<Uuid, InFlightTask>>,
    timed_out_tasks: Mutex<BTreeMap<Uuid, TimedOutTask>>,
    task_conversations: Mutex<BTreeMap<Uuid, Vec<ChatMessage>>>,
    session_conversations: Mutex<BTreeMap<Uuid, SessionConversation>>,
    shell_approval_overrides: Mutex<BTreeMap<Uuid, bool>>,
    provider: Mutex<ProviderRuntime>,
    provider_caps: ProviderCapabilities,
    embedding_runtime: Mutex<EmbeddingRuntime>,
    index_runtime: Mutex<IndexRuntime>,
    index_store: IndexStore,
    default_zsh_builtins: BTreeSet<String>,
    index_write_lock: Mutex<()>,
    index_progress: Mutex<IndexProgress>,
    last_index_update: Mutex<IndexUpdateSummary>,
    index_update_tx: broadcast::Sender<IndexUpdateSummary>,
    observed: Mutex<VecDeque<ObservedEntry>>,
    last_source_ledger: Mutex<source_ledger::SourceLedger>,
    last_stage_timings_ms: Mutex<BTreeMap<String, u64>>,
    retrieval_cache: Mutex<cache::TimedCache<Vec<termlm_protocol::GroundingRef>>>,
    command_validation_cache: Mutex<cache::TimedCache<bool>>,
    file_read_cache: Mutex<cache::TimedCache<CachedToolResult>>,
    project_metadata_cache: Mutex<cache::TimedCache<CachedToolResult>>,
    git_context_cache: Mutex<cache::TimedCache<CachedToolResult>>,
    web_search_cache: Mutex<WebCache>,
    web_read_cache: Mutex<WebCache>,
    web_last_request_at: Mutex<Option<std::time::Instant>>,
    started_at: std::time::Instant,
    model_load_ms: u64,
    model_resident_mb: u64,
    last_provider_usage: Mutex<ProviderUsageSnapshot>,
}

impl DaemonState {
    fn config_snapshot(&self) -> Arc<AppConfig> {
        self.config.load_full()
    }
}

fn should_enforce_startup_health(cfg: &AppConfig) -> bool {
    cfg.inference.startup_failure_behavior == "fail"
        && (cfg.inference.provider != "ollama" || cfg.ollama.healthcheck_on_start)
}

#[derive(Debug)]
enum ControlMsg {
    MaybeShutdown,
    ShutdownNow,
}

#[derive(Debug, Parser)]
#[command(name = "termlm-core")]
#[command(about = "termlm daemon")]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    sandbox_cwd: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    detach: bool,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if std::env::var_os("OBJC_DISABLE_INITIALIZE_FORK_SAFETY").is_none() {
            // SAFETY: set once during daemon startup before any child-process
            // extraction helpers are spawned. This avoids macOS ObjC fork-safety
            // aborts when Metal-backed runtime threads are active.
            unsafe {
                std::env::set_var("OBJC_DISABLE_INITIALIZE_FORK_SAFETY", "YES");
            }
        }
    }

    let cli = Cli::parse();
    maybe_detach(cli.detach)?;

    let config_path = cli.config.unwrap_or_else(default_config_path);
    let sandbox_cwd = if let Some(path) = cli.sandbox_cwd.as_deref() {
        let resolved = resolve_sandbox_cwd(path)?;
        std::env::set_current_dir(&resolved)
            .with_context(|| format!("failed to chdir to sandbox {}", resolved.display()))?;
        Some(resolved)
    } else {
        None
    };
    let loaded = load_or_create(Some(&config_path))?;
    let cfg = loaded.config;
    init_logging(&cfg);
    for w in loaded.warnings {
        warn!("{w}");
    }
    let runtime_stub_provider = runtime_stub_provider_enabled();
    if !runtime_stub_provider {
        ensure_required_model_assets(&cfg).await?;
        validate_provider_boot(&cfg).await?;
    } else {
        warn!("runtime-stub feature enabled; skipping provider asset/bootstrap checks");
    }
    let mut provider = build_provider(&cfg)?;
    let model_rss_before_mb = current_process_rss_mb();
    let model_load_ms = if runtime_stub_provider {
        0
    } else {
        let load_started = std::time::Instant::now();
        provider.load_or_connect().await?;
        let enforce_startup_health = should_enforce_startup_health(&cfg);
        if enforce_startup_health {
            let health = provider.health().await?;
            if !health.healthy {
                bail!(
                    "inference provider failed startup health check: {}",
                    health.details
                );
            }
        } else if cfg.inference.provider == "ollama" && !cfg.ollama.healthcheck_on_start {
            warn!("ollama startup healthcheck is disabled; proceeding without endpoint probe");
        }
        load_started.elapsed().as_millis() as u64
    };
    let model_rss_after_mb = current_process_rss_mb();
    let model_resident_mb = if runtime_stub_provider || cfg.inference.provider == "ollama" {
        0
    } else {
        match (model_rss_before_mb, model_rss_after_mb) {
            (Some(before), Some(after)) => after.saturating_sub(before),
            _ => 0,
        }
    };
    let provider_caps = provider.capabilities().await?;
    if cfg.inference.tool_calling_required
        && !(provider_caps.supports_native_tool_calls || provider_caps.supports_json_mode)
    {
        bail!("provider does not support structured tool-calling and tool_calling_required=true");
    }
    info!(
        provider = %cfg.inference.provider,
        model = %active_model_name(&cfg),
        model_load_ms,
        model_resident_mb,
        "inference provider initialized"
    );

    let socket_path = resolve_socket_path(&cfg.daemon.socket_path);
    let pid_path = resolve_runtime_path(&cfg.daemon.pid_file);

    prepare_parent(&socket_path)?;
    prepare_parent(&pid_path)?;
    ensure_single_daemon(&pid_path, &socket_path)?;
    cleanup_stale_socket(&socket_path)?;

    // SAFETY: setting process umask for daemon startup.
    unsafe {
        libc::umask(0o077);
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 600 {}", socket_path.display()))?;
    write_pid_file(&pid_path)?;

    let index_root = resolve_index_root();
    let default_zsh_builtins = load_or_extract_zsh_builtins(&index_root).unwrap_or_else(|e| {
        warn!("failed to load zsh builtins cache: {e:#}");
        BTreeSet::new()
    });
    let web_total_cache_bytes = effective_web_cache_bytes(&cfg);
    let (search_cache_bytes, read_cache_bytes) = split_web_cache_bytes(web_total_cache_bytes);
    let (search_cache_ttl_secs, read_cache_ttl_secs) = effective_web_cache_ttls(&cfg);
    let retrieval_cache_ttl_secs = cfg.cache.retrieval_cache_ttl_secs.max(1);
    let command_validation_cache_ttl_secs = cfg.cache.command_validation_cache_ttl_secs.max(1);
    let file_read_cache_ttl_secs = cfg.cache.file_read_cache_ttl_secs.max(1);
    let project_metadata_cache_ttl_secs = cfg.cache.project_metadata_cache_ttl_secs.max(1);
    let git_context_cache_ttl_secs = cfg.cache.git_context_cache_ttl_secs.max(1);
    let (index_update_tx, _) = broadcast::channel(64);
    let state = Arc::new(DaemonState {
        config: ArcSwap::from_pointee(cfg),
        config_path,
        sandbox_cwd,
        registry: Mutex::new(ShellRegistry::default()),
        detached_contexts: Mutex::new(BTreeMap::new()),
        tasks: Mutex::new(BTreeMap::new()),
        timed_out_tasks: Mutex::new(BTreeMap::new()),
        task_conversations: Mutex::new(BTreeMap::new()),
        session_conversations: Mutex::new(BTreeMap::new()),
        shell_approval_overrides: Mutex::new(BTreeMap::new()),
        provider: Mutex::new(provider),
        provider_caps,
        embedding_runtime: Mutex::new(EmbeddingRuntime::default()),
        index_runtime: Mutex::new(IndexRuntime::default()),
        index_store: IndexStore::new(index_root),
        default_zsh_builtins,
        index_write_lock: Mutex::new(()),
        index_progress: Mutex::new(IndexProgress {
            scanned: 0,
            total: 0,
            percent: 0.0,
            phase: "idle".to_string(),
        }),
        last_index_update: Mutex::new(IndexUpdateSummary::default()),
        index_update_tx,
        observed: Mutex::new(VecDeque::new()),
        last_source_ledger: Mutex::new(source_ledger::SourceLedger::default()),
        last_stage_timings_ms: Mutex::new(BTreeMap::new()),
        retrieval_cache: Mutex::new(cache::TimedCache::new(std::time::Duration::from_secs(
            retrieval_cache_ttl_secs,
        ))),
        command_validation_cache: Mutex::new(cache::TimedCache::new(
            std::time::Duration::from_secs(command_validation_cache_ttl_secs),
        )),
        file_read_cache: Mutex::new(cache::TimedCache::new(std::time::Duration::from_secs(
            file_read_cache_ttl_secs,
        ))),
        project_metadata_cache: Mutex::new(cache::TimedCache::new(std::time::Duration::from_secs(
            project_metadata_cache_ttl_secs,
        ))),
        git_context_cache: Mutex::new(cache::TimedCache::new(std::time::Duration::from_secs(
            git_context_cache_ttl_secs,
        ))),
        web_search_cache: Mutex::new(WebCache::new(search_cache_bytes, search_cache_ttl_secs)),
        web_read_cache: Mutex::new(WebCache::new(read_cache_bytes, read_cache_ttl_secs)),
        web_last_request_at: Mutex::new(None),
        started_at: std::time::Instant::now(),
        model_load_ms,
        model_resident_mb,
        last_provider_usage: Mutex::new(ProviderUsageSnapshot::default()),
    });

    let state_for_reload = Arc::clone(&state);
    tokio::spawn(async move {
        if let Err(e) = watch_config_reload_signals(state_for_reload).await {
            warn!("config reload signal loop stopped: {e:#}");
        }
    });

    let state_for_timeouts = Arc::clone(&state);
    tokio::spawn(async move {
        run_task_timeout_sweeper(state_for_timeouts).await;
    });

    let state_for_index = Arc::clone(&state);
    let warm_core_on_start = state.config_snapshot().performance.warm_core_on_start;
    let should_prewarm_docs = state.config_snapshot().performance.prewarm_common_docs;
    tokio::spawn(async move {
        if warm_core_on_start {
            if let Err(e) = bootstrap_index_runtime(Arc::clone(&state_for_index)).await {
                warn!("index bootstrap failed: {e:#}");
                return;
            }
            if should_prewarm_docs && let Err(e) = prewarm_common_docs(&state_for_index).await {
                warn!("common docs prewarm failed: {e:#}");
            }
        } else {
            info!("warm_core_on_start=false: deferring startup index bootstrap");
        }
        if let Err(e) = run_index_watch_loop(state_for_index).await {
            warn!("index watch loop stopped: {e:#}");
        }
    });

    let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<ControlMsg>(64);

    info!("termlm-core listening at {}", socket_path.display());

    loop {
        tokio::select! {
            incoming = listener.accept() => {
                let (stream, _) = match incoming {
                    Ok(v) => v,
                    Err(e) => {
                        error!("accept failed: {e}");
                        continue;
                    }
                };
                let state = Arc::clone(&state);
                let ctrl_tx = ctrl_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(state, stream, ctrl_tx).await {
                        error!("connection error: {e:#}");
                    }
                });
            }
            Some(msg) = ctrl_rx.recv() => {
                match msg {
                    ControlMsg::ShutdownNow => {
                        info!("received forced shutdown");
                        break;
                    }
                    ControlMsg::MaybeShutdown => {
                        // Keep the daemon resident until an explicit `stop`/shutdown.
                        // Install/bootstrap flows and non-interactive commands need the
                        // runtime to stay up even when no shell is currently registered.
                    }
                }
            }
        }
    }

    if state.config_snapshot().indexer.enabled
        && let Err(e) = compact_index(&state).await
    {
        warn!("shutdown compaction failed: {e:#}");
    }

    {
        let provider = state.provider.lock().await;
        if let Err(e) = provider.shutdown().await {
            warn!("provider shutdown failed: {e:#}");
        }
    }
    shutdown_embedding_runtime(&state).await;
    let _ = std::fs::remove_file(&pid_path);
    let _ = std::fs::remove_file(&socket_path);
    info!("termlm-core shutdown complete");
    Ok(())
}

#[cfg(unix)]
async fn watch_config_reload_signals(state: Arc<DaemonState>) -> Result<()> {
    let mut hup = signal(SignalKind::hangup()).context("failed to subscribe to SIGHUP")?;
    while hup.recv().await.is_some() {
        if let Err(e) = reload_runtime_config(&state).await {
            warn!("SIGHUP config reload failed: {e:#}");
        }
    }
    Ok(())
}

#[cfg(not(unix))]
async fn watch_config_reload_signals(_state: Arc<DaemonState>) -> Result<()> {
    std::future::pending::<Result<()>>().await
}

async fn reload_runtime_config(state: &Arc<DaemonState>) -> Result<()> {
    let loaded = load_or_create(Some(&state.config_path))?;
    for w in loaded.warnings {
        warn!("{w}");
    }

    let old_cfg = state.config_snapshot();
    let mut next_cfg = loaded.config;
    let mut changed_keys = changed_config_keys(old_cfg.as_ref(), &next_cfg)?;
    changed_keys.sort();
    changed_keys.dedup();

    if changed_keys.is_empty() {
        info!("SIGHUP received: config unchanged");
        return Ok(());
    }

    let mut hot_keys = Vec::new();
    let mut restart_keys = Vec::new();
    for key in &changed_keys {
        match reload_class_for_key(key) {
            ReloadClass::HotReload => hot_keys.push(key.clone()),
            ReloadClass::RestartRequired => restart_keys.push(key.clone()),
        }
    }

    if !restart_keys.is_empty() {
        preserve_restart_required_fields(old_cfg.as_ref(), &mut next_cfg);
        warn!(
            "SIGHUP: keeping restart-required settings unchanged until restart: {}",
            restart_keys.join(", ")
        );
    }

    {
        let mut provider = state.provider.lock().await;
        provider
            .apply_runtime_config(&next_cfg)
            .await
            .context("failed applying provider runtime config")?;
    }

    {
        let web_total_cache_bytes = effective_web_cache_bytes(&next_cfg);
        let (search_cache_bytes, read_cache_bytes) = split_web_cache_bytes(web_total_cache_bytes);
        let (search_cache_ttl_secs, read_cache_ttl_secs) = effective_web_cache_ttls(&next_cfg);
        *state.web_search_cache.lock().await =
            WebCache::new(search_cache_bytes, search_cache_ttl_secs);
        *state.web_read_cache.lock().await = WebCache::new(read_cache_bytes, read_cache_ttl_secs);
        *state.retrieval_cache.lock().await = cache::TimedCache::new(
            std::time::Duration::from_secs(next_cfg.cache.retrieval_cache_ttl_secs.max(1)),
        );
        *state.command_validation_cache.lock().await = cache::TimedCache::new(
            std::time::Duration::from_secs(next_cfg.cache.command_validation_cache_ttl_secs.max(1)),
        );
        *state.file_read_cache.lock().await = cache::TimedCache::new(
            std::time::Duration::from_secs(next_cfg.cache.file_read_cache_ttl_secs.max(1)),
        );
        *state.project_metadata_cache.lock().await = cache::TimedCache::new(
            std::time::Duration::from_secs(next_cfg.cache.project_metadata_cache_ttl_secs.max(1)),
        );
        *state.git_context_cache.lock().await = cache::TimedCache::new(
            std::time::Duration::from_secs(next_cfg.cache.git_context_cache_ttl_secs.max(1)),
        );
    }

    state.config.store(Arc::new(next_cfg));
    info!(
        "SIGHUP config reload complete: {} hot keys applied{}",
        hot_keys.len(),
        if restart_keys.is_empty() {
            String::new()
        } else {
            format!(", {} restart-required keys deferred", restart_keys.len())
        }
    );
    Ok(())
}

fn preserve_restart_required_fields(old: &AppConfig, next: &mut AppConfig) {
    next.model = old.model.clone();
    next.inference.provider = old.inference.provider.clone();
    next.ollama.endpoint = old.ollama.endpoint.clone();
    next.performance.profile = old.performance.profile.clone();
    next.indexer.embed_filename = old.indexer.embed_filename.clone();
    next.indexer.embed_dim = old.indexer.embed_dim;
    next.indexer.vector_storage = old.indexer.vector_storage.clone();
    next.indexer.embedding_provider = old.indexer.embedding_provider.clone();
    next.indexer.lexical_index_impl = old.indexer.lexical_index_impl.clone();
    next.indexer.embed_query_prefix = old.indexer.embed_query_prefix.clone();
    next.indexer.embed_doc_prefix = old.indexer.embed_doc_prefix.clone();
    next.web.provider = old.web.provider.clone();
}

fn changed_config_keys(old: &AppConfig, new: &AppConfig) -> Result<Vec<String>> {
    let old_json = serde_json::to_value(old)?;
    let new_json = serde_json::to_value(new)?;
    let mut out = Vec::new();
    diff_json_paths("", &old_json, &new_json, &mut out);
    Ok(out)
}

fn diff_json_paths(
    prefix: &str,
    old: &serde_json::Value,
    new: &serde_json::Value,
    out: &mut Vec<String>,
) {
    use serde_json::Value;

    match (old, new) {
        (Value::Object(old_obj), Value::Object(new_obj)) => {
            let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            keys.extend(old_obj.keys().cloned());
            keys.extend(new_obj.keys().cloned());
            for key in keys {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match (old_obj.get(&key), new_obj.get(&key)) {
                    (Some(a), Some(b)) => diff_json_paths(&path, a, b, out),
                    _ => out.push(path),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a != b && !prefix.is_empty() {
                out.push(prefix.to_string());
            }
        }
        _ => {
            if old != new && !prefix.is_empty() {
                out.push(prefix.to_string());
            }
        }
    }
}

async fn handle_connection(
    state: Arc<DaemonState>,
    stream: UnixStream,
    ctrl_tx: mpsc::Sender<ControlMsg>,
) -> Result<()> {
    if !peer_uid_matches(&stream)? {
        warn!("rejecting client with mismatched uid");
        return Ok(());
    }

    let mut transport = ipc::make_transport(stream);
    let mut bound_shell_id: Option<Uuid> = None;
    let mut index_update_rx = state.index_update_tx.subscribe();

    loop {
        let next = tokio::select! {
            maybe = ipc::recv_message(&mut transport) => maybe,
            update = index_update_rx.recv() => {
                match update {
                    Ok(update) => {
                        if update.is_empty() {
                            continue;
                        }
                        if let Err(e) = transport.send(ServerMessage::IndexUpdate {
                            added: update.added,
                            updated: update.updated,
                            removed: update.removed,
                        }).await {
                            warn!("failed to send async index update to client: {e:#}");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("index update stream lagged; skipped {skipped} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => {}
                }
                continue;
            }
        };
        let Some(msg) = next else {
            break;
        };
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                if let Err(send_err) = ipc::send_protocol_error(&mut transport, e).await {
                    warn!("failed to send protocol error to client: {send_err:#}");
                }
                break;
            }
        };

        let handled: Result<()> = match msg {
            ClientMessage::RegisterShell { payload } => {
                let missing = missing_required_capabilities(&payload.capabilities);
                if !missing.is_empty() {
                    transport
                        .send(ServerMessage::Error {
                            task_id: None,
                            kind: ErrorKind::BadProtocol,
                            message: format!(
                                "adapter missing required capabilities: {}",
                                missing.join(", ")
                            ),
                            matched_pattern: None,
                        })
                        .await?;
                    continue;
                }
                let shell_id = Uuid::now_v7();
                {
                    let mut reg = state.registry.lock().await;
                    reg.insert(
                        shell_id,
                        ShellSession {
                            shell_pid: payload.shell_pid,
                            tty: payload.tty,
                            shell_kind: payload.shell_kind.clone(),
                            shell_version: payload.shell_version,
                            env_subset: payload.env_subset,
                            context: None,
                        },
                    );
                }
                let state_for_index = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(e) = run_delta_indexing(&state_for_index, false).await {
                        warn!("delta indexing refresh after shell register failed: {e:#}");
                    }
                });
                bound_shell_id = Some(shell_id);

                transport
                    .send(ServerMessage::ShellRegistered {
                        shell_id,
                        accepted_capabilities: required_capability_names()
                            .iter()
                            .map(|v| (*v).to_string())
                            .collect(),
                        provider: state.config_snapshot().inference.provider.clone(),
                        model: active_model_name(state.config_snapshot().as_ref()),
                        context_tokens: state.config_snapshot().model.context_tokens,
                    })
                    .await?;
                Ok(())
            }
            ClientMessage::ShellContext { payload } => {
                state
                    .registry
                    .lock()
                    .await
                    .set_context(payload.shell_id, payload.clone());
                state
                    .detached_contexts
                    .lock()
                    .await
                    .insert(payload.shell_id, payload);
                Ok(())
            }
            ClientMessage::StartTask { payload } => {
                let task_id = payload.task_id;
                match process_start_task(&state, &mut transport, payload).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        cancel_and_clear_task(&state, task_id).await;
                        Err(e)
                    }
                }
            }
            ClientMessage::UserResponse { payload } => {
                let task_id = payload.task_id;
                match process_user_response(&state, &mut transport, payload).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        cancel_and_clear_task(&state, task_id).await;
                        Err(e)
                    }
                }
            }
            ClientMessage::Ack { payload } => {
                let task_id = payload.task_id;
                match process_ack(&state, &mut transport, payload).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        cancel_and_clear_task(&state, task_id).await;
                        Err(e)
                    }
                }
            }
            ClientMessage::Status => send_status(&state, &mut transport).await,
            ClientMessage::ProviderHealth => send_provider_health(&state, &mut transport).await,
            ClientMessage::Shutdown => {
                let _ = ctrl_tx.send(ControlMsg::ShutdownNow).await;
                break;
            }
            ClientMessage::Ping => {
                transport.send(ServerMessage::Pong).await?;
                Ok(())
            }
            ClientMessage::UnregisterShell { shell_id } => {
                cancel_and_clear_shell_tasks(&state, shell_id).await;
                state.registry.lock().await.remove(&shell_id);
                if bound_shell_id == Some(shell_id) {
                    bound_shell_id = None;
                }
                let _ = ctrl_tx.send(ControlMsg::MaybeShutdown).await;
                Ok(())
            }
            ClientMessage::ObservedCommand { payload } => {
                process_observed_command(&state, payload).await
            }
            ClientMessage::Reindex { mode } => {
                process_reindex_request(&state, &mut transport, mode).await
            }
            ClientMessage::Retrieve { payload } => {
                process_retrieve_request(&state, &mut transport, payload).await
            }
        };

        if let Err(e) = handled {
            warn!("connection message handling failed: {e:#}");
            break;
        }
    }

    if let Some(shell_id) = bound_shell_id {
        cancel_and_clear_shell_tasks(&state, shell_id).await;
        state.registry.lock().await.remove(&shell_id);
        let _ = ctrl_tx.send(ControlMsg::MaybeShutdown).await;
    }
    let _ = ctrl_tx.send(ControlMsg::MaybeShutdown).await;

    Ok(())
}

async fn cancel_provider_task(state: &Arc<DaemonState>, task_id: Uuid) {
    let task_id_text = task_id.to_string();
    let result = {
        let provider = state.provider.lock().await;
        provider.cancel(&task_id_text).await
    };
    if let Err(e) = result {
        warn!(task_id = %task_id, "provider cancel failed: {e:#}");
    }
}

async fn clear_task_state(state: &Arc<DaemonState>, task_id: Uuid) {
    let removed = state.tasks.lock().await.remove(&task_id);
    if state.config_snapshot().approval.approve_all_resets_per_task
        && let Some(task) = removed
    {
        state
            .shell_approval_overrides
            .lock()
            .await
            .remove(&task.shell_id);
    }
    state.timed_out_tasks.lock().await.remove(&task_id);
    state.task_conversations.lock().await.remove(&task_id);
}

async fn cancel_and_clear_task(state: &Arc<DaemonState>, task_id: Uuid) {
    cancel_provider_task(state, task_id).await;
    clear_task_state(state, task_id).await;
}

async fn cancel_and_clear_shell_tasks(state: &Arc<DaemonState>, shell_id: Uuid) {
    let task_ids = {
        let tasks = state.tasks.lock().await;
        tasks
            .values()
            .filter(|t| t.shell_id == shell_id)
            .map(|t| t.task_id)
            .collect::<Vec<_>>()
    };
    if task_ids.is_empty() {
        return;
    }
    for task_id in &task_ids {
        cancel_provider_task(state, *task_id).await;
    }
    {
        let mut tasks = state.tasks.lock().await;
        for task_id in &task_ids {
            tasks.remove(task_id);
        }
    }
    {
        let mut conversations = state.task_conversations.lock().await;
        for task_id in &task_ids {
            conversations.remove(task_id);
        }
    }
    {
        let mut timed_out = state.timed_out_tasks.lock().await;
        for task_id in &task_ids {
            timed_out.remove(task_id);
        }
    }
    state
        .shell_approval_overrides
        .lock()
        .await
        .remove(&shell_id);
    warn!(
        shell_id = %shell_id,
        task_count = task_ids.len(),
        "cleared active task state on shell disconnect"
    );
}

async fn run_task_timeout_sweeper(state: Arc<DaemonState>) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        expire_timed_out_tasks(&state).await;
    }
}

async fn expire_timed_out_tasks(state: &Arc<DaemonState>) {
    let cfg = state.config_snapshot();
    let clarification_timeout =
        std::time::Duration::from_secs(cfg.behavior.clarification_timeout_secs.max(1));
    let command_timeout = std::time::Duration::from_secs(cfg.behavior.command_timeout_secs.max(1));
    let now = std::time::Instant::now();
    let mut expired = Vec::<(Uuid, TaskTimeoutKind)>::new();

    {
        let mut tasks = state.tasks.lock().await;
        for (task_id, task) in tasks.iter() {
            let elapsed = now.duration_since(task.created_at);
            let kind = if task.awaiting_clarification && elapsed >= clarification_timeout {
                Some(TaskTimeoutKind::Clarification)
            } else if !task.awaiting_clarification && elapsed >= command_timeout {
                Some(TaskTimeoutKind::Command)
            } else {
                None
            };
            if let Some(kind) = kind {
                expired.push((*task_id, kind));
            }
        }
        for (task_id, _) in &expired {
            tasks.remove(task_id);
        }
    }

    if !expired.is_empty() {
        {
            let mut conversations = state.task_conversations.lock().await;
            for (task_id, _) in &expired {
                conversations.remove(task_id);
            }
        }
        {
            let mut timed_out = state.timed_out_tasks.lock().await;
            for (task_id, kind) in &expired {
                timed_out.insert(
                    *task_id,
                    TimedOutTask {
                        at: now,
                        kind: *kind,
                    },
                );
            }
            let retention = std::time::Duration::from_secs(
                clarification_timeout
                    .as_secs()
                    .max(command_timeout.as_secs())
                    .max(30),
            );
            timed_out.retain(|_, t| now.duration_since(t.at) <= retention);
        }
        info!(
            expired_count = expired.len(),
            clarification_timeout_secs = clarification_timeout.as_secs(),
            command_timeout_secs = command_timeout.as_secs(),
            "expired task(s) due to timeout"
        );
    } else {
        let mut timed_out = state.timed_out_tasks.lock().await;
        let retention = std::time::Duration::from_secs(
            clarification_timeout
                .as_secs()
                .max(command_timeout.as_secs())
                .max(30),
        );
        timed_out.retain(|_, t| now.duration_since(t.at) <= retention);
    }
}

async fn consume_timed_out_task(
    state: &Arc<DaemonState>,
    task_id: Uuid,
) -> Option<TaskTimeoutKind> {
    state
        .timed_out_tasks
        .lock()
        .await
        .remove(&task_id)
        .map(|t| t.kind)
}

fn active_task_timeout_kind(
    state: &Arc<DaemonState>,
    task: &InFlightTask,
) -> Option<TaskTimeoutKind> {
    let cfg = state.config_snapshot();
    if task.awaiting_clarification {
        let timeout =
            std::time::Duration::from_secs(cfg.behavior.clarification_timeout_secs.max(1));
        if task.created_at.elapsed() >= timeout {
            return Some(TaskTimeoutKind::Clarification);
        }
    } else {
        let timeout = std::time::Duration::from_secs(cfg.behavior.command_timeout_secs.max(1));
        if task.created_at.elapsed() >= timeout {
            return Some(TaskTimeoutKind::Command);
        }
    }
    None
}

async fn process_start_task(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: termlm_protocol::StartTask,
) -> Result<()> {
    let mut payload = payload;
    apply_task_sandbox(state, &mut payload);
    let trimmed_prompt = payload.prompt.trim();
    if let Some(matched) = matches_safety_floor(trimmed_prompt) {
        transport
            .send(ServerMessage::Error {
                task_id: Some(payload.task_id),
                kind: ErrorKind::SafetyFloor,
                message: "Refused: prompt requested an immutable safety-floor command".to_string(),
                matched_pattern: Some(matched.pattern.to_string()),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::SafetyFloor,
                summary: "Prompt matched immutable safety floor.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            "Prompt blocked by immutable safety floor.".to_string(),
        )
        .await;
        return Ok(());
    }
    let task_started = std::time::Instant::now();
    let mut stage_timings = BTreeMap::<String, u64>::new();
    let source_ledger_started = std::time::Instant::now();
    *state.last_source_ledger.lock().await = source_ledger::SourceLedger::default();
    *state.last_provider_usage.lock().await = ProviderUsageSnapshot::default();
    append_source_refs(
        state,
        vec![source_ledger::SourceRef {
            source_type: "user_prompt".to_string(),
            source_id: payload.task_id.to_string(),
            hash: hash_prefix(&payload.prompt),
            redacted: false,
            truncated: false,
            observed_at: chrono::Utc::now(),
            detail: Some(truncate_string(&payload.prompt, 200)),
            section: None,
            offset_start: None,
            offset_end: None,
            extraction_method: None,
            extracted_at: None,
            index_version: None,
        }],
    )
    .await;
    let session = {
        let reg = state.registry.lock().await;
        reg.get(&payload.shell_id).cloned()
    };
    let session = if let Some(session) = session {
        session
    } else {
        let detached = state
            .detached_contexts
            .lock()
            .await
            .get(&payload.shell_id)
            .cloned();
        ShellSession {
            shell_pid: 0,
            tty: "unknown".to_string(),
            shell_kind: payload.shell_kind.clone(),
            shell_version: payload.shell_version.clone(),
            env_subset: payload.env_subset.clone(),
            context: detached,
        }
    };

    let classify_started = std::time::Instant::now();
    let classification =
        classify_prompt_for_task(state.config_snapshot().as_ref(), &payload.prompt);
    stage_timings.insert(
        "classify_ms".to_string(),
        classify_started.elapsed().as_millis() as u64,
    );

    let context_started = std::time::Instant::now();
    let context_assembly = assemble_task_prompt(state, &payload, &classification).await;
    stage_timings.insert(
        "assemble_context_ms".to_string(),
        context_started.elapsed().as_millis() as u64,
    );
    if context_assembly
        .included_blocks
        .iter()
        .any(|b| b == "recent_terminal")
    {
        let refs = {
            let observed = state.observed.lock().await;
            let mut entries = observed
                .iter()
                .filter(|e| e.shell_id == payload.shell_id)
                .cloned()
                .collect::<Vec<_>>();
            entries.sort_by_key(|e| std::cmp::Reverse(e.started_at));
            entries
                .into_iter()
                .take(25)
                .map(|entry| source_ledger::SourceRef {
                    source_type: "terminal_context".to_string(),
                    source_id: format!("{}:{}", payload.shell_id, entry.command_seq),
                    hash: hash_prefix(&format!(
                        "{}|{}|{}",
                        entry.command, entry.stderr_tail, entry.stdout_tail
                    )),
                    redacted: entry.redactions_applied,
                    truncated: entry.stdout_truncated || entry.stderr_truncated,
                    observed_at: entry.started_at,
                    detail: Some(format!(
                        "exit={} capture={} cwd={}",
                        entry.exit_code, entry.output_capture_status, entry.cwd
                    )),
                    section: entry.detected_error_lines.first().cloned(),
                    offset_start: None,
                    offset_end: None,
                    extraction_method: None,
                    extracted_at: None,
                    index_version: None,
                })
                .collect::<Vec<_>>()
        };
        append_source_refs(state, refs).await;
    }
    stage_timings.insert(
        "source_ledger_ms".to_string(),
        source_ledger_started.elapsed().as_millis() as u64,
    );
    let drafting_prompt = context_assembly.prompt.clone();

    let progress_started = std::time::Instant::now();
    let progress_guard = state.index_progress.lock().await;
    if let Some(progress_line) = indexing_progress_banner(&progress_guard) {
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: progress_line,
            })
            .await?;
    }
    drop(progress_guard);
    stage_timings.insert(
        "progress_banner_ms".to_string(),
        progress_started.elapsed().as_millis() as u64,
    );

    let allow_non_command_shortcuts = payload.mode != "?";
    let result = if allow_non_command_shortcuts
        && matches!(
            classification.classification,
            tasks::TaskClassification::DocumentationQuestion
        ) {
        let docs_started = std::time::Instant::now();
        let out = process_documentation_question(state, transport, &payload, &session).await;
        stage_timings.insert(
            "documentation_path_ms".to_string(),
            docs_started.elapsed().as_millis() as u64,
        );
        out
    } else if allow_non_command_shortcuts
        && matches!(
            classification.classification,
            tasks::TaskClassification::WebCurrentInfoQuestion
        )
        && state.config_snapshot().web.enabled
    {
        let web_started = std::time::Instant::now();
        let out = process_web_question(state, transport, &payload).await;
        stage_timings.insert(
            "web_path_ms".to_string(),
            web_started.elapsed().as_millis() as u64,
        );
        out
    } else {
        let local_started = std::time::Instant::now();
        let handled_local = if allow_non_command_shortcuts {
            try_handle_local_tool_request(state, transport, &payload).await?
        } else {
            false
        };
        stage_timings.insert(
            "local_shortcut_ms".to_string(),
            local_started.elapsed().as_millis() as u64,
        );
        if handled_local {
            Ok(())
        } else {
            let mut handled_by_runtime_stub = false;
            if runtime_stub_provider_enabled() {
                let stub_started = std::time::Instant::now();
                if maybe_run_runtime_stub_provider(
                    state,
                    transport,
                    &payload,
                    &session,
                    false,
                    &classification,
                    &payload.prompt,
                )
                .await?
                {
                    stage_timings.insert(
                        "runtime_stub_provider_ms".to_string(),
                        stub_started.elapsed().as_millis() as u64,
                    );
                    handled_by_runtime_stub = true;
                }
            }
            if handled_by_runtime_stub {
                Ok(())
            } else {
                let orchestration_started = std::time::Instant::now();
                let handled_provider = match try_provider_orchestration(
                    state,
                    transport,
                    &payload,
                    &session,
                    &classification,
                    &drafting_prompt,
                    true,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        cancel_and_clear_task(state, payload.task_id).await;
                        return Err(e);
                    }
                };
                stage_timings.insert(
                    "provider_orchestration_ms".to_string(),
                    orchestration_started.elapsed().as_millis() as u64,
                );
                if handled_provider {
                    Ok(())
                } else {
                    let fallback_started = std::time::Instant::now();
                    let handled_fallback = if maybe_run_runtime_stub_provider(
                        state,
                        transport,
                        &payload,
                        &session,
                        false,
                        &classification,
                        &payload.prompt,
                    )
                    .await?
                    {
                        true
                    } else {
                        maybe_run_heuristic_command_fallback(
                            state,
                            transport,
                            &payload,
                            &session,
                            &classification,
                        )
                        .await?
                    };
                    if handled_fallback {
                        stage_timings.insert(
                            "provider_no_tool_call_ms".to_string(),
                            fallback_started.elapsed().as_millis() as u64,
                        );
                        Ok(())
                    } else {
                        let shell_override =
                            approval_override_for_shell(state, payload.shell_id).await;
                        state.tasks.lock().await.insert(
                            payload.task_id,
                            InFlightTask {
                                task_id: payload.task_id,
                                shell_id: payload.shell_id,
                                mode: payload.mode.clone(),
                                original_prompt: payload.prompt.clone(),
                                proposed_command: String::new(),
                                classification: classification.classification.clone(),
                                classification_confidence: classification.confidence,
                                approval_override: shell_override,
                                awaiting_clarification: true,
                                provider_continuation: true,
                                tool_round: 0,
                                created_at: std::time::Instant::now(),
                            },
                        );
                        transport
                            .send(ServerMessage::NeedsClarification {
                                task_id: payload.task_id,
                                question:
                                    "I couldn't produce a structured command tool call yet. What exact command behavior should run?"
                                        .to_string(),
                            })
                            .await?;
                        stage_timings.insert(
                            "provider_no_tool_call_ms".to_string(),
                            fallback_started.elapsed().as_millis() as u64,
                        );
                        Ok(())
                    }
                }
            }
        }
    };

    stage_timings.insert(
        "task_total_ms".to_string(),
        task_started.elapsed().as_millis() as u64,
    );
    *state.last_stage_timings_ms.lock().await = stage_timings;
    result
}

async fn process_user_response(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: termlm_protocol::UserResponse,
) -> Result<()> {
    if let Some(kind) = consume_timed_out_task(state, payload.task_id).await {
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::Timeout,
                summary: kind.summary().to_string(),
            })
            .await?;
        return Ok(());
    }

    let task = {
        let tasks = state.tasks.lock().await;
        tasks.get(&payload.task_id).cloned()
    };
    let Some(task) = task else {
        transport
            .send(ServerMessage::Error {
                task_id: Some(payload.task_id),
                kind: ErrorKind::BadProtocol,
                message: "task_id not active".to_string(),
                matched_pattern: None,
            })
            .await?;
        return Ok(());
    };

    if let Some(kind) = active_task_timeout_kind(state, &task) {
        clear_task_state(state, payload.task_id).await;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::Timeout,
                summary: kind.summary().to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &task.mode,
            task.shell_id,
            task.original_prompt.clone(),
            kind.summary().to_string(),
        )
        .await;
        return Ok(());
    }

    match payload.decision {
        UserDecision::Approved => {
            if let Some(task) = state.tasks.lock().await.get_mut(&payload.task_id) {
                task.awaiting_clarification = false;
                task.created_at = std::time::Instant::now();
            }
            transport
                .send(ServerMessage::ModelText {
                    task_id: payload.task_id,
                    chunk: "Command approved. Execute in shell and send Ack.".to_string(),
                })
                .await?;
        }
        UserDecision::ApproveAllInTask => {
            if let Some(task) = state.tasks.lock().await.get_mut(&payload.task_id) {
                task.approval_override = true;
                task.awaiting_clarification = false;
                task.created_at = std::time::Instant::now();
                if !state.config_snapshot().approval.approve_all_resets_per_task {
                    state
                        .shell_approval_overrides
                        .lock()
                        .await
                        .insert(task.shell_id, true);
                }
            }
            transport
                .send(ServerMessage::ModelText {
                    task_id: payload.task_id,
                    chunk: "Approve-all enabled for this task. Execute in shell and send Ack."
                        .to_string(),
                })
                .await?;
        }
        UserDecision::Edited => {
            let edited = payload
                .edited_command
                .unwrap_or_default()
                .trim()
                .to_string();
            if edited.is_empty() {
                transport
                    .send(ServerMessage::Error {
                        task_id: Some(payload.task_id),
                        kind: ErrorKind::BadProtocol,
                        message: "edited command is empty".to_string(),
                        matched_pattern: None,
                    })
                    .await?;
            } else if let Some(matched) = matches_safety_floor(&edited) {
                transport
                    .send(ServerMessage::Error {
                        task_id: Some(payload.task_id),
                        kind: ErrorKind::SafetyFloor,
                        message: "edited command blocked by immutable safety floor".to_string(),
                        matched_pattern: Some(matched.pattern.to_string()),
                    })
                    .await?;
                transport
                    .send(ServerMessage::TaskComplete {
                        task_id: payload.task_id,
                        reason: TaskCompleteReason::SafetyFloor,
                        summary: "Edited command blocked by safety floor.".to_string(),
                    })
                    .await?;
                append_session_turn_if_session_mode(
                    state,
                    &task.mode,
                    task.shell_id,
                    task.original_prompt.clone(),
                    "Edited command blocked by immutable safety floor.".to_string(),
                )
                .await;
                clear_task_state(state, payload.task_id).await;
            } else {
                if let Some(task) = state.tasks.lock().await.get_mut(&payload.task_id) {
                    task.proposed_command = edited.clone();
                    task.awaiting_clarification = false;
                    task.created_at = std::time::Instant::now();
                }
                transport
                    .send(ServerMessage::ModelText {
                        task_id: payload.task_id,
                        chunk: "Edited command accepted. Execute in shell and send Ack."
                            .to_string(),
                    })
                    .await?;
            }
        }
        UserDecision::Rejected => {
            if task.provider_continuation {
                let rejection_reason = payload.text.unwrap_or_default();
                let mut rejection_tool_response = "User declined to run this command.".to_string();
                if !rejection_reason.trim().is_empty() {
                    rejection_tool_response
                        .push_str(&format!(" Reason: {}", rejection_reason.trim()));
                }
                let mut conversations = state.task_conversations.lock().await;
                let history = conversations
                    .entry(payload.task_id)
                    .or_insert_with(Vec::new);
                history.push(ChatMessage::tool(
                    "execute_shell_command",
                    rejection_tool_response,
                ));

                let session = lookup_task_session(state, &task).await;
                let mut retry_prompt = task.original_prompt.clone();
                retry_prompt.push_str(
                    "\n\nThe user declined the prior command. Propose a safer or alternate command that still satisfies the request.",
                );

                let synth = termlm_protocol::StartTask {
                    task_id: payload.task_id,
                    shell_id: task.shell_id,
                    shell_kind: session.shell_kind.clone(),
                    shell_version: session.shell_version.clone(),
                    mode: task.mode.clone(),
                    prompt: retry_prompt.clone(),
                    cwd: task_cwd_for_sandbox(
                        state,
                        session
                            .env_subset
                            .get("PWD")
                            .map(String::as_str)
                            .unwrap_or("."),
                    ),
                    env_subset: session.env_subset.clone(),
                };
                let classification = classify_prompt_for_task(
                    state.config_snapshot().as_ref(),
                    &task.original_prompt,
                );
                if match try_provider_orchestration(
                    state,
                    transport,
                    &synth,
                    &session,
                    &classification,
                    &retry_prompt,
                    true,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        cancel_and_clear_task(state, payload.task_id).await;
                        return Err(e);
                    }
                } {
                    if let Some(next_task) = state.tasks.lock().await.get_mut(&payload.task_id) {
                        next_task.provider_continuation = true;
                        next_task.original_prompt = task.original_prompt.clone();
                    }
                    return Ok(());
                }
            }

            transport
                .send(ServerMessage::ModelText {
                    task_id: payload.task_id,
                    chunk: "User declined to run this command.".to_string(),
                })
                .await?;
            transport
                .send(ServerMessage::TaskComplete {
                    task_id: payload.task_id,
                    reason: TaskCompleteReason::ModelDone,
                    summary: "Command rejected by user.".to_string(),
                })
                .await?;
            clear_task_state(state, payload.task_id).await;
            if task.mode == "/p" {
                append_session_turn(
                    state,
                    task.shell_id,
                    task.original_prompt.clone(),
                    "Task rejected by user.".to_string(),
                )
                .await;
            }
        }
        UserDecision::Abort => {
            transport
                .send(ServerMessage::TaskComplete {
                    task_id: payload.task_id,
                    reason: TaskCompleteReason::Aborted,
                    summary: "Task aborted by user.".to_string(),
                })
                .await?;
            cancel_and_clear_task(state, payload.task_id).await;
            if task.mode == "/p" {
                append_session_turn(
                    state,
                    task.shell_id,
                    task.original_prompt.clone(),
                    "Task aborted by user.".to_string(),
                )
                .await;
            }
        }
        UserDecision::Clarification => {
            if !task.awaiting_clarification {
                transport
                    .send(ServerMessage::Error {
                        task_id: Some(payload.task_id),
                        kind: ErrorKind::BadProtocol,
                        message: "task is not waiting for clarification".to_string(),
                        matched_pattern: None,
                    })
                    .await?;
                return Ok(());
            }
            let detail = payload.text.unwrap_or_default();
            let refined_prompt = format!("{}\nClarification: {}", task.original_prompt, detail);

            let session = lookup_task_session(state, &task).await;
            let synth = termlm_protocol::StartTask {
                task_id: payload.task_id,
                shell_id: task.shell_id,
                shell_kind: session.shell_kind.clone(),
                shell_version: session.shell_version.clone(),
                mode: task.mode.clone(),
                prompt: refined_prompt.clone(),
                cwd: task_cwd_for_sandbox(
                    state,
                    session
                        .env_subset
                        .get("PWD")
                        .map(String::as_str)
                        .unwrap_or("."),
                ),
                env_subset: session.env_subset.clone(),
            };

            let classification = tasks::ClassificationResult {
                classification: task.classification.clone(),
                confidence: task.classification_confidence,
            };

            // Session-mode clarification should continue orchestration (answer/question
            // workflow), not force a command fallback.
            if task.mode != "?" {
                let handled = match try_provider_orchestration(
                    state,
                    transport,
                    &synth,
                    &session,
                    &classification,
                    &refined_prompt,
                    true,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        cancel_and_clear_task(state, payload.task_id).await;
                        return Err(e);
                    }
                };

                if handled {
                    if let Some(active) = state.tasks.lock().await.get_mut(&payload.task_id) {
                        active.awaiting_clarification = false;
                        active.provider_continuation = true;
                        active.original_prompt = refined_prompt;
                        active.created_at = std::time::Instant::now();
                    }
                    return Ok(());
                }

                if let Some(active) = state.tasks.lock().await.get_mut(&payload.task_id) {
                    active.awaiting_clarification = true;
                    active.provider_continuation = true;
                    active.original_prompt = refined_prompt;
                    active.created_at = std::time::Instant::now();
                }

                transport
                    .send(ServerMessage::NeedsClarification {
                        task_id: payload.task_id,
                        question: "I still need a more specific request. Include the exact outcome, target path/repo, and any constraints.".to_string(),
                    })
                    .await?;
                return Ok(());
            }

            // Prompt mode keeps deterministic command fallback behavior.
            let fallback = tasks::validation_incomplete_fallback(&refined_prompt);
            let fallback_plan = CommandPlan {
                cmd: fallback.cmd.trim().to_string(),
                rationale: fallback.rationale,
                intent: fallback.intent,
                expected_effect: fallback.expected_effect,
                commands_used: fallback.commands_used,
            };
            propose_command_for_execution(
                state,
                transport,
                &synth,
                &session,
                true,
                &classification,
                fallback_plan,
            )
            .await?;
            if let Some(active) = state.tasks.lock().await.get_mut(&payload.task_id) {
                active.awaiting_clarification = false;
                active.provider_continuation = true;
                active.original_prompt = refined_prompt;
                active.created_at = std::time::Instant::now();
            }
            return Ok(());
        }
    }

    Ok(())
}

async fn process_ack(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: termlm_protocol::Ack,
) -> Result<()> {
    let cfg = state.config_snapshot();
    if let Some(kind) = consume_timed_out_task(state, payload.task_id).await {
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::Timeout,
                summary: kind.summary().to_string(),
            })
            .await?;
        return Ok(());
    }

    let task = state.tasks.lock().await.remove(&payload.task_id);
    let Some(task) = task else {
        transport
            .send(ServerMessage::Error {
                task_id: Some(payload.task_id),
                kind: ErrorKind::BadProtocol,
                message: "Ack received for unknown task_id".to_string(),
                matched_pattern: None,
            })
            .await?;
        return Ok(());
    };

    if let Some(kind) = active_task_timeout_kind(state, &task) {
        state
            .task_conversations
            .lock()
            .await
            .remove(&payload.task_id);
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::Timeout,
                summary: kind.summary().to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &task.mode,
            task.shell_id,
            task.original_prompt.clone(),
            kind.summary().to_string(),
        )
        .await;
        return Ok(());
    }

    if cfg.terminal_context.enabled && cfg.terminal_context.capture_all_interactive_commands {
        let observed = observed_from_ack(task.shell_id, &payload, cfg.as_ref());
        process_observed_command(state, observed).await?;
    }

    let stdout_text = if let Some(stdout_b64) = &payload.stdout_b64 {
        match base64::engine::general_purpose::STANDARD.decode(stdout_b64) {
            Ok(raw) => String::from_utf8_lossy(&raw).to_string(),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };
    let stderr_text = if let Some(stderr_b64) = &payload.stderr_b64 {
        match base64::engine::general_purpose::STANDARD.decode(stderr_b64) {
            Ok(raw) => String::from_utf8_lossy(&raw).to_string(),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };
    let mut stdout_text = stdout_text;
    let mut stderr_text = stderr_text;
    let mut applied_redactions = payload.redactions_applied.clone();
    if cfg.terminal_context.redact_secrets {
        let redacted_stdout = termlm_local_tools::redaction::redact_secrets(&stdout_text);
        let redacted_stderr = termlm_local_tools::redaction::redact_secrets(&stderr_text);
        if redacted_stdout != stdout_text || redacted_stderr != stderr_text {
            applied_redactions.push("secret".to_string());
        }
        stdout_text = redacted_stdout;
        stderr_text = redacted_stderr;
    }
    let (stdout_text, stdout_env_redactions) =
        redact_capture_env_values(&stdout_text, &cfg.capture.redact_env);
    let (stderr_text, stderr_env_redactions) =
        redact_capture_env_values(&stderr_text, &cfg.capture.redact_env);
    applied_redactions.extend(stdout_env_redactions);
    applied_redactions.extend(stderr_env_redactions);
    applied_redactions.sort();
    applied_redactions.dedup();

    if task.provider_continuation {
        let tool_result = serde_json::json!({
            "command": payload.executed_command,
            "cwd_before": payload.cwd_before,
            "cwd_after": payload.cwd_after,
            "exit_status": payload.exit_status,
            "stdout": truncate_string(&stdout_text, 3000),
            "stdout_truncated": payload.stdout_truncated,
            "stderr": truncate_string(&stderr_text, 2000),
            "stderr_truncated": payload.stderr_truncated,
            "redactions_applied": applied_redactions,
            "elapsed_ms": payload.elapsed_ms,
        });
        let serialized = serde_json::to_string(&tool_result).unwrap_or_else(|_| "{}".to_string());
        let mut conversations = state.task_conversations.lock().await;
        let history = conversations
            .entry(payload.task_id)
            .or_insert_with(Vec::new);
        history.push(ChatMessage::tool("execute_shell_command", serialized));
    }

    if task.provider_continuation {
        let next_round = task.tool_round.saturating_add(1);
        if next_round >= state.config_snapshot().behavior.max_tool_rounds {
            transport
                .send(ServerMessage::TaskComplete {
                    task_id: payload.task_id,
                    reason: TaskCompleteReason::ToolRoundLimit,
                    summary: format!(
                        "Stopped after {} tool rounds.",
                        state.config_snapshot().behavior.max_tool_rounds
                    ),
                })
                .await?;
            state
                .task_conversations
                .lock()
                .await
                .remove(&payload.task_id);
            append_session_turn_if_session_mode(
                state,
                &task.mode,
                task.shell_id,
                task.original_prompt.clone(),
                format!(
                    "Stopped after {} tool rounds.",
                    state.config_snapshot().behavior.max_tool_rounds
                ),
            )
            .await;
            return Ok(());
        }

        let session = lookup_task_session(state, &task).await;
        let mut followup_prompt = String::new();
        followup_prompt.push_str(&task.original_prompt);
        followup_prompt.push_str("\n\nPrevious command result:\n");
        followup_prompt.push_str(&format!(
            "- command: {}\n- exit_status: {}\n",
            payload.executed_command, payload.exit_status
        ));
        if !stdout_text.trim().is_empty() {
            followup_prompt.push_str("- stdout:\n");
            followup_prompt.push_str(&truncate_string(&stdout_text, 1600));
            followup_prompt.push('\n');
        }
        if !stderr_text.trim().is_empty() {
            followup_prompt.push_str("- stderr:\n");
            followup_prompt.push_str(&truncate_string(&stderr_text, 1200));
            followup_prompt.push('\n');
        }
        followup_prompt.push_str(
            "Continue the same task. Propose the next shell command only if needed; otherwise finish.",
        );

        let synth = termlm_protocol::StartTask {
            task_id: payload.task_id,
            shell_id: task.shell_id,
            shell_kind: session.shell_kind.clone(),
            shell_version: session.shell_version.clone(),
            mode: task.mode.clone(),
            prompt: followup_prompt.clone(),
            cwd: task_cwd_for_sandbox(state, &payload.cwd_after),
            env_subset: session.env_subset.clone(),
        };
        let classification = tasks::ClassificationResult {
            classification: tasks::TaskClassification::DiagnosticDebugging,
            confidence: 0.8,
        };
        if match try_provider_orchestration(
            state,
            transport,
            &synth,
            &session,
            &classification,
            &followup_prompt,
            true,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                cancel_and_clear_task(state, payload.task_id).await;
                return Err(e);
            }
        } {
            if let Some(t) = state.tasks.lock().await.get_mut(&payload.task_id) {
                t.tool_round = next_round;
                t.provider_continuation = true;
                t.original_prompt = task.original_prompt.clone();
            }
            return Ok(());
        }
    }

    let mut summary = format!("Command exited with status {}", payload.exit_status);
    if !stdout_text.trim().is_empty() {
        summary.push_str("; stdout captured");
        if payload.stdout_truncated {
            summary.push_str(" (truncated)");
        }
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: truncate_string(&stdout_text, 400),
            })
            .await?;
    }
    if !stderr_text.trim().is_empty() {
        if payload.stderr_truncated {
            summary.push_str("; stderr truncated");
        }
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: truncate_string(&stderr_text, 240),
            })
            .await?;
    }

    transport
        .send(ServerMessage::TaskComplete {
            task_id: payload.task_id,
            reason: TaskCompleteReason::ModelDone,
            summary,
        })
        .await?;
    append_session_turn_if_session_mode(
        state,
        &task.mode,
        task.shell_id,
        task.original_prompt.clone(),
        if payload.exit_status == 0 {
            "Command completed successfully.".to_string()
        } else {
            format!("Command failed with exit status {}.", payload.exit_status)
        },
    )
    .await;
    state
        .task_conversations
        .lock()
        .await
        .remove(&payload.task_id);

    Ok(())
}

fn observed_from_ack(
    shell_id: Uuid,
    payload: &termlm_protocol::Ack,
    cfg: &AppConfig,
) -> termlm_protocol::ObservedCommand {
    termlm_protocol::ObservedCommand {
        shell_id,
        command_seq: payload.command_seq,
        raw_command: payload.executed_command.clone(),
        expanded_command: payload.executed_command.clone(),
        cwd_before: payload.cwd_before.clone(),
        cwd_after: payload.cwd_after.clone(),
        started_at: payload.started_at,
        exit_status: payload.exit_status,
        duration_ms: payload.elapsed_ms,
        stdout_b64: payload.stdout_b64.clone(),
        stdout_truncated: payload.stdout_truncated,
        stderr_b64: payload.stderr_b64.clone(),
        stderr_truncated: payload.stderr_truncated,
        output_capture_status: if cfg.capture.enabled {
            "captured".to_string()
        } else {
            "skipped_not_captured".to_string()
        },
    }
}

async fn send_status(state: &Arc<DaemonState>, transport: &mut ipc::ServerTransport) -> Result<()> {
    let cfg = state.config_snapshot();
    let (active_shells, _registered_shell_pids, _registered_shell_ttys) = {
        let registry = state.registry.lock().await;
        let active_shells = registry.len();
        let unique_pids = registry
            .iter()
            .map(|(_, session)| session.shell_pid)
            .collect::<BTreeSet<_>>()
            .len();
        let non_empty_ttys = registry
            .iter()
            .filter(|(_, session)| !session.tty.trim().is_empty())
            .count();
        (active_shells, unique_pids, non_empty_ttys)
    };
    let active_tasks = state.tasks.lock().await.len();
    let progress = state.index_progress.lock().await.clone();
    let stage_timings_ms = state.last_stage_timings_ms.lock().await.clone();
    let chunk_count = state.index_runtime.lock().await.chunks.len() as u64;
    let provider_health = state.provider.lock().await.health().await.ok();
    let provider_remote = if cfg.inference.provider == "ollama" {
        !is_loopback_endpoint(&cfg.ollama.endpoint)
    } else {
        false
    };
    let rss_mb = current_process_rss_mb().unwrap_or(chunk_count / 512);
    let indexer_resident_mb = estimate_indexer_resident_mb(chunk_count);
    let kv_cache_mb = estimate_kv_cache_mb(cfg.as_ref(), active_tasks);
    let model_resident_mb = if cfg.inference.provider == "ollama" {
        0
    } else {
        state.model_resident_mb
    };
    let orchestration_resident_mb = rss_mb
        .saturating_sub(model_resident_mb)
        .saturating_sub(indexer_resident_mb)
        .saturating_sub(kv_cache_mb);
    let structured_mode = match state.provider_caps.structured_mode {
        StructuredOutputMode::NativeToolCalling => "native_tool_calling",
        StructuredOutputMode::StrictJsonFallback => "strict_json_fallback",
    };
    let provider_usage = *state.last_provider_usage.lock().await;
    let (last_task_source_refs, last_task_source_ledger) =
        if cfg.source_ledger.enabled && cfg.source_ledger.expose_on_status {
            let ledger = state.last_source_ledger.lock().await;
            let max_refs = cfg.source_ledger.max_refs_on_status.max(1);
            let refs = ledger
                .refs
                .iter()
                .rev()
                .take(max_refs)
                .cloned()
                .collect::<Vec<_>>();
            let mut status_refs = refs
                .into_iter()
                .map(|r| StatusSourceRef {
                    source_type: r.source_type,
                    source_id: r.source_id,
                    hash: r.hash,
                    redacted: r.redacted,
                    truncated: r.truncated,
                    observed_at: r.observed_at,
                    detail: r.detail,
                    section: r.section,
                    offset_start: r.offset_start,
                    offset_end: r.offset_end,
                    extraction_method: r.extraction_method,
                    extracted_at: r.extracted_at,
                    index_version: r.index_version,
                })
                .collect::<Vec<_>>();
            status_refs.reverse();
            (ledger.refs.len(), status_refs)
        } else {
            (0, Vec::new())
        };
    transport
        .send(ServerMessage::StatusReport {
            pid: std::process::id(),
            uptime_secs: state.started_at.elapsed().as_secs(),
            socket_path: resolve_socket_path(&cfg.daemon.socket_path)
                .display()
                .to_string(),
            provider: cfg.inference.provider.clone(),
            model: active_model_name(cfg.as_ref()),
            endpoint: if cfg.inference.provider == "ollama" {
                Some(cfg.ollama.endpoint.clone())
            } else {
                None
            },
            provider_healthy: provider_health.as_ref().map(|h| h.healthy).unwrap_or(false),
            provider_health_latency_ms: provider_health.as_ref().map(|h| h.latency_ms),
            provider_context_window: state.provider_caps.context_window,
            provider_structured_mode: structured_mode.to_string(),
            provider_supports_native_tool_calls: state.provider_caps.supports_native_tool_calls,
            provider_supports_json_mode: state.provider_caps.supports_json_mode,
            provider_remote,
            rss_mb,
            model_resident_mb,
            indexer_resident_mb,
            orchestration_resident_mb,
            kv_cache_mb,
            active_shells,
            active_tasks,
            model_load_ms: state.model_load_ms,
            last_task_prompt_tokens: if provider_usage.reported {
                Some(provider_usage.prompt_tokens)
            } else {
                None
            },
            last_task_completion_tokens: if provider_usage.reported {
                Some(provider_usage.completion_tokens)
            } else {
                None
            },
            last_task_usage_reported: provider_usage.reported,
            last_task_source_refs,
            last_task_source_ledger,
            stage_timings_ms,
            index_progress: progress,
            web: WebStatus {
                enabled: cfg.web.enabled,
                provider: cfg.web.provider.clone(),
            },
        })
        .await?;
    Ok(())
}

fn current_process_rss_mb() -> Option<u64> {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let kb = raw.trim().parse::<u64>().ok()?;
    Some(kb.div_ceil(1024))
}

fn estimate_kv_cache_mb(cfg: &AppConfig, active_tasks: usize) -> u64 {
    if cfg.inference.provider != "local" || active_tasks == 0 {
        return 0;
    }

    // Normalize against the v1 default expectation (~200 MB per active task at 8192 ctx).
    let per_task_mb = ((cfg.model.context_tokens.max(1) as f64 / 8192.0) * 200.0).ceil() as u64;
    per_task_mb
        .max(1)
        .saturating_mul(active_tasks.min(u64::MAX as usize) as u64)
}

fn estimate_indexer_resident_mb(fallback_chunk_count: u64) -> u64 {
    let root = resolve_index_root();
    let files = [
        "entries.bin",
        "docs.bin",
        "chunks.bin",
        "vectors.f16",
        "vectors.f32",
        "lexicon.bin",
        "postings.bin",
    ];
    let bytes = files
        .iter()
        .map(|name| root.join(name))
        .filter_map(|path| std::fs::metadata(path).ok().map(|m| m.len()))
        .sum::<u64>();
    if bytes == 0 {
        return fallback_chunk_count / 512;
    }
    bytes.div_ceil(1024 * 1024)
}

async fn send_provider_health(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
) -> Result<()> {
    let health = state.provider.lock().await.health().await.ok();
    let healthy = health.as_ref().map(|h| h.healthy).unwrap_or(false);
    let remote = if state.config_snapshot().inference.provider == "ollama" {
        !is_loopback_endpoint(&state.config_snapshot().ollama.endpoint)
    } else {
        false
    };

    transport
        .send(ServerMessage::ProviderStatus {
            provider: state.config_snapshot().inference.provider.clone(),
            model: active_model_name(state.config_snapshot().as_ref()),
            endpoint: if state.config_snapshot().inference.provider == "ollama" {
                Some(state.config_snapshot().ollama.endpoint.clone())
            } else {
                None
            },
            healthy,
            remote,
        })
        .await?;

    Ok(())
}

fn indexing_progress_banner(progress: &IndexProgress) -> Option<String> {
    if progress.phase == "complete" || progress.phase == "idle" {
        return None;
    }
    Some(format!(
        "Note: documentation indexing is in progress ({:.1}% complete; {} commands available so far). If a command you need is missing from the cheat sheet, try lookup_command_docs(name) — the index may still have it.",
        progress.percent, progress.scanned
    ))
}

fn split_web_cache_bytes(total_bytes: usize) -> (usize, usize) {
    let total = total_bytes.max(32 * 1024);
    let search = (total / 4).max(16 * 1024);
    let read = total.saturating_sub(search).max(16 * 1024);
    (search, read)
}

fn effective_web_cache_bytes(cfg: &AppConfig) -> usize {
    if !cfg.cache.enabled {
        return 32 * 1024;
    }
    cfg.web
        .cache_max_bytes
        .min(cfg.cache.max_total_cache_bytes.max(32 * 1024))
}

fn effective_web_cache_ttls(cfg: &AppConfig) -> (u64, u64) {
    if !cfg.cache.enabled {
        return (0, 0);
    }
    let read_ttl = cfg
        .web
        .cache_ttl_secs
        .min(cfg.cache.web_cache_ttl_secs)
        .max(1);
    let search_ttl = cfg
        .web
        .search_cache_ttl_secs
        .min(cfg.cache.web_cache_ttl_secs)
        .max(1);
    (search_ttl, read_ttl)
}

fn classify_prompt_for_task(cfg: &AppConfig, prompt: &str) -> tasks::ClassificationResult {
    if cfg.behavior.context_classifier_enabled {
        tasks::classify_prompt_with_freshness_terms(prompt, &cfg.web.freshness_required_terms)
    } else {
        tasks::ClassificationResult {
            classification: tasks::TaskClassification::FreshCommandRequest,
            confidence: 1.0,
        }
    }
}

fn infer_search_freshness(prompt: &str, freshness_terms: &[String]) -> Option<String> {
    let p = prompt.to_ascii_lowercase();
    let mut terms = freshness_terms
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        terms = vec![
            "latest".to_string(),
            "current".to_string(),
            "today".to_string(),
            "recent".to_string(),
            "release".to_string(),
            "version".to_string(),
            "new".to_string(),
            "now".to_string(),
        ];
    }

    for term in terms {
        if !p.contains(&term) {
            continue;
        }
        let mapped = match term.as_str() {
            "today" | "now" => "day".to_string(),
            "recent" => "week".to_string(),
            "latest" | "current" | "release" | "version" | "new" => "month".to_string(),
            other => other.to_string(),
        };
        return Some(mapped);
    }
    None
}

async fn approval_override_for_shell(state: &Arc<DaemonState>, shell_id: Uuid) -> bool {
    if state.config_snapshot().approval.approve_all_resets_per_task {
        return false;
    }
    state
        .shell_approval_overrides
        .lock()
        .await
        .get(&shell_id)
        .copied()
        .unwrap_or(false)
}

fn effective_indexer_concurrency(cfg: &AppConfig) -> usize {
    let base = cfg.indexer.concurrency.max(1);
    let profile_cap = match PerformanceProfile::from_str(&cfg.performance.profile) {
        PerformanceProfile::Eco => 2usize,
        PerformanceProfile::Balanced => 4usize,
        PerformanceProfile::Performance => base,
    };
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let cpu_budget =
        ((cpu.saturating_mul(cfg.performance.max_background_cpu_pct.max(1) as usize)) / 100).max(1);
    base.min(profile_cap).min(cpu_budget).max(1)
}

fn resolve_sandbox_cwd(path: &Path) -> Result<PathBuf> {
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to read current working directory")?
            .join(path)
    };
    let normalized = normalize_lexical_path(&base);
    std::fs::create_dir_all(&normalized).with_context(|| {
        format!(
            "failed to create sandbox directory {}",
            normalized.display()
        )
    })?;
    let meta = std::fs::metadata(&normalized)
        .with_context(|| format!("failed to stat sandbox {}", normalized.display()))?;
    if !meta.is_dir() {
        bail!("sandbox path is not a directory: {}", normalized.display());
    }
    normalized
        .canonicalize()
        .with_context(|| format!("failed to canonicalize sandbox {}", normalized.display()))
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        if path.is_absolute() {
            PathBuf::from("/")
        } else {
            PathBuf::from(".")
        }
    } else {
        out
    }
}

fn canonical_or_normalized(path: &Path) -> PathBuf {
    let normalized = normalize_lexical_path(path);
    normalized.canonicalize().unwrap_or(normalized)
}

fn resolve_path_within_root(root: &Path, requested: &Path) -> Option<PathBuf> {
    let root_path = canonical_or_normalized(root);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root_path.join(requested)
    };
    let candidate_path = canonical_or_normalized(&candidate);
    if candidate_path.starts_with(&root_path) {
        Some(candidate_path)
    } else {
        None
    }
}

fn expand_home_path_buf(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(raw)
}

fn path_allowed_by_local_sensitive_allowlist(path: &Path, cfg: &AppConfig) -> bool {
    let candidate = canonical_or_normalized(path);
    cfg.local_tools
        .sensitive_path_allowlist
        .iter()
        .filter_map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return None;
            }
            let mut allowed = expand_home_path_buf(trimmed);
            if !allowed.is_absolute()
                && let Ok(cwd) = std::env::current_dir()
            {
                allowed = cwd.join(allowed);
            }
            Some(canonical_or_normalized(&allowed))
        })
        .any(|allowed| candidate.starts_with(allowed))
}

fn is_sensitive_local_path(path: &Path, cfg: &AppConfig) -> bool {
    if path_allowed_by_local_sensitive_allowlist(path, cfg) {
        return false;
    }

    let normalized = canonical_or_normalized(path);
    let components = normalized
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let has_component = |needle: &str| components.iter().any(|c| c == needle);
    let has_component_pair =
        |a: &str, b: &str| components.windows(2).any(|w| w[0] == a && w[1] == b);

    if has_component(".ssh")
        || has_component(".gnupg")
        || has_component(".aws")
        || has_component(".azure")
        || has_component(".kube")
        || has_component("keychains")
        || has_component(".password-store")
        || has_component(".bitwarden")
        || has_component(".1password")
        || has_component_pair(".config", "gcloud")
        || has_component_pair("library", "keychains")
    {
        return true;
    }

    let name = normalized
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with(".env")
        && !name.contains("example")
        && !name.contains("sample")
        && !name.contains("template")
    {
        return true;
    }

    let exact_sensitive_names = [
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        "id_xmss",
        ".netrc",
        ".git-credentials",
        "credentials",
        "credentials.db",
        "tokens.json",
        "auth.json",
        "access_tokens.db",
    ];
    if exact_sensitive_names.iter().any(|entry| name == *entry) {
        return true;
    }

    name.ends_with(".pem")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
        || name.ends_with(".key")
}

fn task_cwd_for_sandbox(state: &DaemonState, requested: &str) -> String {
    let Some(root) = state.sandbox_cwd.as_ref() else {
        return requested.to_string();
    };
    let req = PathBuf::from(requested);
    let joined = if req.is_absolute() {
        req
    } else {
        root.join(req)
    };
    let mut candidate = normalize_lexical_path(&joined);
    if let Ok(canon) = candidate.canonicalize() {
        candidate = canon;
    }
    if !candidate.starts_with(root) {
        candidate = root.clone();
    }
    candidate.display().to_string()
}

fn apply_task_sandbox(state: &DaemonState, payload: &mut termlm_protocol::StartTask) {
    if state.sandbox_cwd.is_none() {
        return;
    }
    let requested = payload.cwd.clone();
    let confined = task_cwd_for_sandbox(state, &requested);
    if requested != confined {
        warn!(
            task_id = %payload.task_id,
            requested_cwd = %requested,
            confined_cwd = %confined,
            "confined task cwd to sandbox"
        );
    }
    payload.cwd = confined.clone();
    payload.env_subset.insert("PWD".to_string(), confined);
}

fn cache_key(kind: &str, parts: &[String]) -> String {
    let joined = parts.join("\u{1f}");
    let digest = sha2::Sha256::digest(joined.as_bytes());
    format!("{kind}:{digest:x}")
}

fn hash_prefix(input: &str) -> String {
    let digest = sha2::Sha256::digest(input.as_bytes());
    let full = format!("{digest:x}");
    full.chars().take(16).collect()
}

fn hash_prefix_bytes(input: &[u8]) -> String {
    let digest = sha2::Sha256::digest(input);
    let full = format!("{digest:x}");
    full.chars().take(16).collect()
}

fn filesystem_fingerprint(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    Some(format!(
        "dev={} ino={} mode={:o} size={} mtime={}.{}",
        meta.dev(),
        meta.ino(),
        meta.mode(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
    ))
}

fn file_content_hash_prefix(path: &Path, max_bytes: usize) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max_bytes.max(1)];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(hash_prefix_bytes(&buf))
}

fn file_read_cache_key(
    path: &Path,
    start_line: usize,
    max_lines: usize,
    max_bytes: usize,
    detection: &termlm_local_tools::TextDetectionOptions,
) -> Option<String> {
    let file_fp = filesystem_fingerprint(path)?;
    let detection_hash =
        hash_prefix(&serde_json::to_string(detection).unwrap_or_else(|_| "detection".to_string()));
    Some(cache_key(
        "local_read_file",
        &[
            path.display().to_string(),
            file_fp,
            start_line.to_string(),
            max_lines.to_string(),
            max_bytes.to_string(),
            detection_hash,
        ],
    ))
}

fn project_metadata_state_fingerprint(root: &Path) -> String {
    let mut parts = vec![root.display().to_string()];
    let candidates = [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lockb",
        "pyproject.toml",
        "poetry.lock",
        "Pipfile.lock",
        "go.mod",
        "go.sum",
        "Makefile",
        "justfile",
        "Justfile",
        "Taskfile.yml",
        ".gitlab-ci.yml",
        ".circleci/config.yml",
        "azure-pipelines.yml",
        "buildkite.yml",
    ];
    for rel in candidates {
        let path = root.join(rel);
        if let Some(fp) = filesystem_fingerprint(&path) {
            let content_hash =
                file_content_hash_prefix(&path, 4096).unwrap_or_else(|| "nohash".to_string());
            parts.push(format!("{rel}:{fp}:{content_hash}"));
        }
    }

    let workflows = root.join(".github/workflows");
    if let Ok(read_dir) = std::fs::read_dir(&workflows) {
        let mut workflow_entries = read_dir
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        workflow_entries.sort();
        for path in workflow_entries.into_iter().take(64) {
            if let Some(fp) = filesystem_fingerprint(&path) {
                let rel = path
                    .strip_prefix(root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                let content_hash =
                    file_content_hash_prefix(&path, 2048).unwrap_or_else(|| "nohash".to_string());
                parts.push(format!("{rel}:{fp}:{content_hash}"));
            }
        }
    }

    hash_prefix(&parts.join("\n"))
}

fn project_metadata_cache_key(
    root: &Path,
    include_scripts: bool,
    include_ci: bool,
    max_files_read: usize,
    max_bytes_per_file: usize,
    detect_package_managers: bool,
) -> String {
    cache_key(
        "project_metadata",
        &[
            root.display().to_string(),
            project_metadata_state_fingerprint(root),
            include_scripts.to_string(),
            include_ci.to_string(),
            detect_package_managers.to_string(),
            max_files_read.to_string(),
            max_bytes_per_file.to_string(),
        ],
    )
}

fn run_git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn git_context_cache_key(
    root: &Path,
    include_diff_summary: bool,
    max_files: usize,
    max_recent_commits: usize,
    max_diff_bytes: usize,
) -> String {
    let head = run_git_stdout(root, &["rev-parse", "--verify", "HEAD"])
        .unwrap_or_else(|| "no_head".to_string());
    let git_index = run_git_stdout(root, &["rev-parse", "--git-path", "index"])
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { root.join(p) });
    let index_fp = git_index
        .as_deref()
        .and_then(filesystem_fingerprint)
        .unwrap_or_else(|| "index_missing".to_string());
    let status_text = run_git_stdout(root, &["status", "--porcelain=v1", "--branch"])
        .unwrap_or_else(|| "status_unavailable".to_string());
    let status_hash = hash_prefix(&status_text);

    cache_key(
        "git_context",
        &[
            root.display().to_string(),
            include_diff_summary.to_string(),
            max_files.to_string(),
            max_recent_commits.to_string(),
            max_diff_bytes.to_string(),
            head,
            index_fp,
            status_hash,
        ],
    )
}

fn command_for_log(cfg: &AppConfig, cmd: &str, critical: bool) -> String {
    if critical && cfg.logging.redact_critical {
        return format!("<critical:{}>", hash_prefix(cmd));
    }
    cmd.to_string()
}

#[derive(Debug, Clone, Copy)]
struct DocsRetrievalCacheSemantics {
    rag_top_k: usize,
    rag_min_similarity: f32,
    hybrid_retrieval_enabled: bool,
    lexical_index_enabled: bool,
    lexical_top_k: usize,
    command_aware_retrieval: bool,
    command_aware_top_k: usize,
    index_revision: u64,
}

fn docs_retrieval_cache_key(
    query_text: &str,
    command_name: &str,
    semantics: DocsRetrievalCacheSemantics,
) -> String {
    cache_key(
        "docs_retrieval",
        &[
            query_text.to_string(),
            command_name.to_string(),
            semantics.rag_top_k.to_string(),
            format!("{:.4}", semantics.rag_min_similarity),
            format!("hybrid:{}", semantics.hybrid_retrieval_enabled),
            format!("lexical:{}", semantics.lexical_index_enabled),
            format!("lexical_top_k:{}", semantics.lexical_top_k),
            format!("command_aware:{}", semantics.command_aware_retrieval),
            format!("command_aware_top_k:{}", semantics.command_aware_top_k),
            format!("idx:{}", semantics.index_revision),
        ],
    )
}

async fn append_source_refs(state: &Arc<DaemonState>, refs: Vec<source_ledger::SourceRef>) {
    if refs.is_empty() {
        return;
    }
    let cfg = state.config_snapshot();
    if !cfg.source_ledger.enabled {
        return;
    }
    let mut ledger = state.last_source_ledger.lock().await;
    let mut seen = ledger
        .refs
        .iter()
        .map(source_ref_key)
        .collect::<HashSet<_>>();
    let filtered = refs
        .into_iter()
        .filter(|r| seen.insert(source_ref_key(r)))
        .collect::<Vec<_>>();
    if cfg.source_ledger.include_in_debug_logs && !filtered.is_empty() {
        debug!(
            new_refs = filtered.len(),
            total_before = ledger.refs.len(),
            "source ledger appended refs"
        );
        for source_ref in filtered.iter().take(16) {
            debug!(
                source_type = %source_ref.source_type,
                source_id = %source_ref.source_id,
                hash = %source_ref.hash,
                redacted = source_ref.redacted,
                truncated = source_ref.truncated,
                section = ?source_ref.section,
                offset_start = ?source_ref.offset_start,
                offset_end = ?source_ref.offset_end,
                extraction_method = ?source_ref.extraction_method,
                extracted_at = ?source_ref.extracted_at,
                index_version = ?source_ref.index_version,
                observed_at = %source_ref.observed_at.to_rfc3339(),
                "source ledger ref"
            );
        }
        if filtered.len() > 16 {
            debug!(
                omitted_refs = filtered.len() - 16,
                "source ledger ref debug output truncated"
            );
        }
    }
    ledger.extend(filtered);
}

fn source_ref_key(r: &source_ledger::SourceRef) -> String {
    format!(
        "{}|{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        r.source_type,
        r.source_id,
        r.hash,
        r.section,
        r.offset_start,
        r.offset_end,
        r.detail,
        r.extraction_method,
        r.extracted_at,
        r.index_version
    )
}

fn citation_urls_from_ledger(ledger: &source_ledger::SourceLedger) -> Vec<String> {
    let mut seen = BTreeSet::<String>::new();
    for r in &ledger.refs {
        if matches!(
            r.source_type.as_str(),
            "web_search_result" | "web_read_page"
        ) && (r.source_id.starts_with("https://") || r.source_id.starts_with("http://"))
        {
            seen.insert(r.source_id.clone());
        }
    }
    seen.into_iter().collect()
}

fn has_citation_block(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("## citations") {
        return true;
    }
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("[1] http://") || trimmed.starts_with("[1] https://")
    })
}

fn build_citation_block(urls: &[String]) -> String {
    if urls.is_empty() {
        return String::new();
    }
    let mut block = String::from("\n\n## Citations\n");
    for (idx, url) in urls.iter().enumerate() {
        block.push_str(&format!("[{}] {}\n", idx + 1, url));
    }
    block
}

async fn embed_texts_with_provider(
    state: &Arc<DaemonState>,
    cfg: &AppConfig,
    texts: &[String],
) -> Result<Option<Vec<Vec<f32>>>> {
    if texts.is_empty() {
        return Ok(Some(Vec::new()));
    }
    match cfg.indexer.embedding_provider.as_str() {
        "local" => {
            let result = match embed_texts_with_local_runtime(state, cfg, texts).await {
                Ok(v) => Ok(Some(v)),
                Err(e) => {
                    warn!("local embedding runtime unavailable, disabling vector retrieval: {e:#}");
                    Ok(None)
                }
            };
            if !cfg.performance.keep_embedding_warm {
                shutdown_embedding_runtime(state).await;
            }
            result
        }
        "ollama" => match embed_texts_with_ollama(cfg, texts).await {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                warn!("ollama embeddings unavailable, disabling vector retrieval: {e:#}");
                Ok(None)
            }
        },
        other => {
            warn!("unsupported embedding provider '{other}', disabling vector retrieval");
            Ok(None)
        }
    }
}

async fn embed_texts_with_ollama(cfg: &AppConfig, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let endpoint = cfg.ollama.endpoint.trim_end_matches('/');
    let url = format!("{endpoint}/api/embed");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(
            cfg.ollama.connect_timeout_secs.max(1),
        ))
        .timeout(std::time::Duration::from_secs(
            cfg.ollama.request_timeout_secs.max(1),
        ))
        .build()
        .context("build ollama embedding client")?;
    let payload = serde_json::json!({
        "model": cfg.indexer.ollama_embed_model,
        "input": texts,
    });
    let resp = client
        .post(url)
        .header("content-type", "application/json")
        .json(&payload)
        .send()
        .await
        .context("ollama embedding request failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("ollama embedding request failed with status {status}: {body}");
    }
    let value = resp
        .json::<serde_json::Value>()
        .await
        .context("parse ollama embedding response")?;
    let rows = parse_embeddings_response(value, cfg.indexer.embed_dim)?;
    if rows.len() != texts.len() {
        bail!(
            "ollama embedding response count mismatch: requested {} got {}",
            texts.len(),
            rows.len()
        );
    }
    Ok(rows)
}

async fn embed_texts_with_local_runtime(
    state: &Arc<DaemonState>,
    cfg: &AppConfig,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    let embed_model_path =
        resolve_models_dir(&cfg.model.models_dir).join(&cfg.indexer.embed_filename);
    if !embed_model_path.exists() {
        bail!(
            "embedding model file does not exist: {}",
            embed_model_path.display()
        );
    }

    // Prefer fully in-process local embeddings via the active local inference provider.
    // If the active inference provider is Ollama, use a dedicated local embedding runtime.
    let local_provider = {
        let provider = state.provider.lock().await;
        match &*provider {
            ProviderRuntime::Local(local) => Some(local.clone()),
            ProviderRuntime::Ollama(_) => None,
        }
    };
    let embed_model = embed_model_path.display().to_string();
    let provider = if let Some(local) = local_provider {
        local
    } else {
        let signature = format!(
            "{}|{}|{}|{}",
            embed_model, cfg.model.context_tokens, cfg.model.gpu_layers, cfg.model.threads
        );
        let mut runtime = state.embedding_runtime.lock().await;
        let needs_new_provider = runtime.provider_signature.as_deref() != Some(signature.as_str())
            || runtime.local_provider.is_none();
        if needs_new_provider {
            runtime.local_provider = Some(LocalLlamaProvider::new(
                embed_model.clone(),
                cfg.model.context_tokens,
                cfg.model.gpu_layers,
                cfg.model.threads,
            ));
            runtime.provider_signature = Some(signature);
        }
        runtime
            .local_provider
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("embedding provider runtime unavailable"))?
    };
    provider
        .embed_texts(&embed_model, cfg.indexer.embed_dim, texts)
        .await
}

fn parse_embeddings_response(value: serde_json::Value, dim: usize) -> Result<Vec<Vec<f32>>> {
    let mut rows = Vec::<Vec<f32>>::new();
    if let Some(data) = value.get("data").and_then(|v| v.as_array()) {
        for row in data {
            if let Some(embedding) = row.get("embedding") {
                rows.push(parse_embedding_row(embedding)?);
            }
        }
    } else if let Some(items) = value.get("embeddings").and_then(|v| v.as_array()) {
        for item in items {
            rows.push(parse_embedding_row(item)?);
        }
    } else if let Some(single) = value.get("embedding") {
        rows.push(parse_embedding_row(single)?);
    }
    if rows.is_empty() {
        bail!("embedding response contained no vectors");
    }
    Ok(rows
        .into_iter()
        .map(|row| normalize_embedding_dim(&row, dim))
        .collect())
}

fn parse_embedding_row(value: &serde_json::Value) -> Result<Vec<f32>> {
    let arr = value
        .as_array()
        .ok_or_else(|| anyhow!("embedding row is not an array"))?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        if let Some(f) = v.as_f64() {
            out.push(f as f32);
        }
    }
    if out.is_empty() {
        bail!("embedding row was empty");
    }
    Ok(out)
}

fn normalize_embedding_dim(input: &[f32], dim: usize) -> Vec<f32> {
    if dim == 0 {
        return vec![0.0];
    }
    let mut out = vec![0.0f32; dim];
    let copy = input.len().min(dim);
    out[..copy].copy_from_slice(&input[..copy]);
    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    for v in &mut out {
        *v /= norm;
    }
    out
}

fn build_doc_embedding_inputs(chunks: &[Chunk], cfg: &AppConfig) -> Vec<String> {
    chunks
        .iter()
        .map(|c| {
            format!(
                "{}{} {} {}",
                cfg.indexer.embed_doc_prefix, c.command_name, c.section_name, c.text
            )
        })
        .collect()
}

async fn embed_query_vector(
    state: &Arc<DaemonState>,
    cfg: &AppConfig,
    prompt: &str,
) -> Option<Vec<f32>> {
    let text = format!("{}{}", cfg.indexer.embed_query_prefix, prompt);
    let input = vec![text];
    match embed_texts_with_provider(state, cfg, &input).await {
        Ok(Some(mut rows)) => rows.pop(),
        _ => None,
    }
}

async fn shutdown_embedding_runtime(state: &Arc<DaemonState>) {
    let provider = {
        let mut runtime = state.embedding_runtime.lock().await;
        runtime.provider_signature = None;
        runtime.local_provider.take()
    };
    if let Some(provider) = provider
        && let Err(e) = provider.shutdown().await
    {
        warn!("embedding runtime shutdown failed: {e:#}");
    }
}

async fn cached_web_search(
    state: &Arc<DaemonState>,
    provider_cache_id: &str,
    provider: &dyn SearchProvider,
    req: &SearchRequest,
    min_delay_between_requests_ms: u64,
) -> Result<(SearchResultSet, bool)> {
    if !state.config_snapshot().cache.enabled {
        enforce_web_politeness(state, min_delay_between_requests_ms).await;
        let result = termlm_web::web_search(provider, req).await?;
        return Ok((result, false));
    }
    let key = web_search_cache_key(provider_cache_id, req);

    if let Some(raw) = state.web_search_cache.lock().await.get(&key)
        && let Ok(parsed) = serde_json::from_str::<SearchResultSet>(&raw)
    {
        return Ok((parsed, true));
    }

    enforce_web_politeness(state, min_delay_between_requests_ms).await;
    let result = termlm_web::web_search(provider, req).await?;
    if let Ok(raw) = serde_json::to_string(&result) {
        state.web_search_cache.lock().await.insert(key, raw);
    }
    Ok((result, false))
}

async fn cached_web_read(
    state: &Arc<DaemonState>,
    client: &reqwest::Client,
    req: &WebReadRequest,
    min_delay_between_requests_ms: u64,
) -> Result<(WebReadResponse, bool)> {
    if !state.config_snapshot().cache.enabled {
        enforce_web_politeness(state, min_delay_between_requests_ms).await;
        let result = web_read(client, req).await?;
        return Ok((result, false));
    }
    let key = web_read_cache_key(req);
    if let Some(raw) = state.web_read_cache.lock().await.get(&key)
        && let Ok(parsed) = serde_json::from_str::<WebReadResponse>(&raw)
    {
        return Ok((parsed, true));
    }

    enforce_web_politeness(state, min_delay_between_requests_ms).await;
    let result = web_read(client, req).await?;
    if let Ok(raw) = serde_json::to_string(&result) {
        state.web_read_cache.lock().await.insert(key, raw);
    }
    Ok((result, false))
}

async fn enforce_web_politeness(state: &Arc<DaemonState>, min_delay_between_requests_ms: u64) {
    let delay = std::time::Duration::from_millis(min_delay_between_requests_ms);
    if delay.is_zero() {
        return;
    }

    loop {
        let wait_for = {
            let mut guard = state.web_last_request_at.lock().await;
            match *guard {
                Some(last) => {
                    let elapsed = last.elapsed();
                    if elapsed >= delay {
                        *guard = Some(std::time::Instant::now());
                        None
                    } else {
                        Some(delay - elapsed)
                    }
                }
                None => {
                    *guard = Some(std::time::Instant::now());
                    None
                }
            }
        };
        if let Some(wait) = wait_for {
            tokio::time::sleep(wait).await;
            continue;
        }
        break;
    }
}

fn web_search_provider_cache_id(cfg: &WebRuntimeConfig) -> String {
    format!(
        "{}:{}",
        cfg.provider,
        cfg.search_endpoint.trim().to_ascii_lowercase()
    )
}

fn web_search_cache_key(provider_cache_id: &str, req: &SearchRequest) -> String {
    cache_key(
        "web_search",
        &[
            provider_cache_id.to_string(),
            req.query.clone(),
            req.freshness.clone().unwrap_or_default(),
            req.max_results.to_string(),
        ],
    )
}

fn web_read_cache_key(req: &WebReadRequest) -> String {
    cache_key(
        "web_read",
        &[
            req.url.clone(),
            req.max_bytes.to_string(),
            req.allow_plain_http.to_string(),
            req.allow_local_addresses.to_string(),
            req.user_agent.clone(),
            req.obey_robots_txt.to_string(),
            req.min_delay_between_requests_ms.to_string(),
            req.robots_cache_ttl_secs.to_string(),
            req.extract_strategy.clone(),
            req.include_images.to_string(),
            req.include_links.to_string(),
            req.include_tables.to_string(),
            req.max_table_rows.to_string(),
            req.max_table_cols.to_string(),
            req.preserve_code_blocks.to_string(),
            req.strip_tracking_params.to_string(),
            req.max_html_bytes.to_string(),
            req.max_markdown_bytes.to_string(),
            req.min_extracted_chars.to_string(),
            req.dedupe_boilerplate.to_string(),
        ],
    )
}

fn resolve_search_bearer_token(cfg: &WebRuntimeConfig) -> Option<String> {
    if cfg.search_api_key_env.trim().is_empty() {
        return None;
    }
    std::env::var(&cfg.search_api_key_env)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn require_search_api_key(cfg: &WebRuntimeConfig, provider: &str) -> Result<String> {
    let env_name = cfg.search_api_key_env.trim();
    if env_name.is_empty() {
        bail!("web.provider={provider} requires [web].search_api_key_env");
    }
    let key = std::env::var(env_name)
        .with_context(|| format!("required API key env var {env_name} is not set"))?;
    let key = key.trim().to_string();
    if key.is_empty() {
        bail!("required API key env var {env_name} is empty");
    }
    Ok(key)
}

fn build_web_search_provider(
    cfg: &WebRuntimeConfig,
    client: reqwest::Client,
) -> Result<Box<dyn SearchProvider>> {
    match cfg.provider.as_str() {
        "duckduckgo_html" => Ok(Box::new(DuckDuckGoHtmlProvider::new(client))),
        "custom_json" => {
            if cfg.search_endpoint.trim().is_empty() {
                bail!("web.provider=custom_json requires [web].search_endpoint");
            }
            Ok(Box::new(CustomJsonProvider::new(
                client,
                cfg.search_endpoint.clone(),
                resolve_search_bearer_token(cfg),
            )))
        }
        "brave" => {
            let key = require_search_api_key(cfg, "brave")?;
            Ok(Box::new(BraveProvider::new(
                client,
                cfg.search_endpoint.clone(),
                key,
            )))
        }
        "kagi" => {
            let key = require_search_api_key(cfg, "kagi")?;
            Ok(Box::new(KagiProvider::new(
                client,
                cfg.search_endpoint.clone(),
                key,
            )))
        }
        "tavily" => {
            let key = require_search_api_key(cfg, "tavily")?;
            Ok(Box::new(TavilyProvider::new(
                client,
                cfg.search_endpoint.clone(),
                key,
            )))
        }
        "whoogle" => Ok(Box::new(WhoogleProvider::new(
            client,
            cfg.search_endpoint.clone(),
        ))),
        "none" => bail!("web provider is disabled"),
        other => bail!("web provider '{other}' is not implemented yet"),
    }
}

async fn process_documentation_question(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: &termlm_protocol::StartTask,
    _session: &ShellSession,
) -> Result<()> {
    let Some(cmd_name) = tasks::extract_command_name_from_doc_prompt(&payload.prompt) else {
        let guidance =
            "I can look up installed command docs. Ask like: `what does <command> do?`".to_string();
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: guidance.clone(),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: "Documentation request handled.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            guidance,
        )
        .await;
        return Ok(());
    };

    let lookup = {
        let runtime = state.index_runtime.lock().await;
        lookup_command_docs(
            &runtime.chunks,
            &cmd_name,
            None,
            state.config_snapshot().indexer.lookup_max_bytes,
        )
    };

    let details = match lookup {
        Ok(found) => {
            format!(
                "## {name}\n{body}",
                name = found.name,
                body = truncate_string(&found.text, 3000)
            )
        }
        Err(suggestions) => {
            let note = "No indexed local documentation was found for this command.";
            if suggestions.is_empty() {
                format!("## {cmd_name}\n{note}")
            } else {
                format!(
                    "## {cmd_name}\n{note}\n\nSimilar commands: {}",
                    suggestions.join(", ")
                )
            }
        }
    };
    let answer = format!("Tool: lookup_command_docs(name={cmd_name})\n\n{details}");

    transport
        .send(ServerMessage::ModelText {
            task_id: payload.task_id,
            chunk: answer.clone(),
        })
        .await?;
    transport
        .send(ServerMessage::TaskComplete {
            task_id: payload.task_id,
            reason: TaskCompleteReason::ModelDone,
            summary: "Documentation request handled.".to_string(),
        })
        .await?;
    append_session_turn_if_session_mode(
        state,
        &payload.mode,
        payload.shell_id,
        payload.prompt.clone(),
        truncate_string(&answer, 1200),
    )
    .await;
    Ok(())
}

async fn process_web_question(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: &termlm_protocol::StartTask,
) -> Result<()> {
    let cfg = state.config_snapshot();
    let (search_cache_ttl_secs, read_cache_ttl_secs) = effective_web_cache_ttls(cfg.as_ref());
    let web_cfg = WebRuntimeConfig {
        enabled: cfg.web.enabled,
        provider: cfg.web.provider.clone(),
        search_endpoint: cfg.web.search_endpoint.clone(),
        search_api_key_env: cfg.web.search_api_key_env.clone(),
        request_timeout_secs: cfg.web.request_timeout_secs,
        connect_timeout_secs: cfg.web.connect_timeout_secs,
        max_results: cfg.web.max_results as usize,
        max_fetch_bytes: cfg.web.max_fetch_bytes,
        max_pages_per_task: cfg.web.max_pages_per_task,
        cache_ttl_secs: read_cache_ttl_secs,
        cache_max_bytes: effective_web_cache_bytes(cfg.as_ref()),
        allowed_schemes: cfg.web.allowed_schemes.clone(),
        allow_plain_http: cfg.web.allow_plain_http,
        allow_local_addresses: cfg.web.allow_local_addresses,
        obey_robots_txt: cfg.web.obey_robots_txt,
        citation_required: cfg.web.citation_required,
        freshness_required_terms: cfg.web.freshness_required_terms.clone(),
        min_delay_between_requests_ms: cfg.web.min_delay_between_requests_ms,
        search_cache_ttl_secs,
        user_agent: cfg.web.user_agent.clone(),
        extract: WebExtractRuntimeConfig {
            strategy: cfg.web.extract.strategy.clone(),
            output_format: cfg.web.extract.output_format.clone(),
            include_images: cfg.web.extract.include_images,
            include_links: cfg.web.extract.include_links,
            include_tables: cfg.web.extract.include_tables,
            max_table_rows: cfg.web.extract.max_table_rows,
            max_table_cols: cfg.web.extract.max_table_cols,
            preserve_code_blocks: cfg.web.extract.preserve_code_blocks,
            strip_tracking_params: cfg.web.extract.strip_tracking_params,
            max_html_bytes: cfg.web.extract.max_html_bytes,
            max_markdown_bytes: cfg.web.extract.max_markdown_bytes,
            min_extracted_chars: cfg.web.extract.min_extracted_chars,
            dedupe_boilerplate: cfg.web.extract.dedupe_boilerplate,
        },
    };

    let search_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(web_cfg.connect_timeout_secs))
        .timeout(std::time::Duration::from_secs(web_cfg.request_timeout_secs))
        .build()?;
    let read_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(web_cfg.connect_timeout_secs))
        .timeout(std::time::Duration::from_secs(web_cfg.request_timeout_secs))
        .redirect(web_read_redirect_policy(
            web_cfg.allow_plain_http,
            web_cfg.allow_local_addresses,
            DEFAULT_MAX_REDIRECTS,
        ))
        .build()?;

    let provider_cache_id = web_search_provider_cache_id(&web_cfg);
    let provider = match build_web_search_provider(&web_cfg, search_client.clone()) {
        Ok(p) => p,
        Err(e) => {
            let summary = "Web request ended with provider limitation.".to_string();
            transport
                .send(ServerMessage::Error {
                    task_id: Some(payload.task_id),
                    kind: ErrorKind::Internal,
                    message: format!("web provider configuration error: {e}"),
                    matched_pattern: None,
                })
                .await?;
            transport
                .send(ServerMessage::TaskComplete {
                    task_id: payload.task_id,
                    reason: TaskCompleteReason::ModelDone,
                    summary: summary.clone(),
                })
                .await?;
            append_session_turn_if_session_mode(
                state,
                &payload.mode,
                payload.shell_id,
                payload.prompt.clone(),
                summary,
            )
            .await;
            return Ok(());
        }
    };

    let (results, _) = match cached_web_search(
        state,
        &provider_cache_id,
        provider.as_ref(),
        &SearchRequest {
            query: payload.prompt.clone(),
            freshness: infer_search_freshness(&payload.prompt, &cfg.web.freshness_required_terms),
            max_results: web_cfg.max_results,
        },
        web_cfg.min_delay_between_requests_ms,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            let summary = "Web request failed.".to_string();
            transport
                .send(ServerMessage::Error {
                    task_id: Some(payload.task_id),
                    kind: ErrorKind::Internal,
                    message: format!("web search failed: {e}"),
                    matched_pattern: None,
                })
                .await?;
            transport
                .send(ServerMessage::TaskComplete {
                    task_id: payload.task_id,
                    reason: TaskCompleteReason::ModelDone,
                    summary: summary.clone(),
                })
                .await?;
            append_session_turn_if_session_mode(
                state,
                &payload.mode,
                payload.shell_id,
                payload.prompt.clone(),
                summary,
            )
            .await;
            return Ok(());
        }
    };

    if results.results.is_empty() {
        let chunk = "No web results found for that query.".to_string();
        let summary = "Web request completed with no results.".to_string();
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: chunk.clone(),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: summary.clone(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            chunk,
        )
        .await;
        return Ok(());
    }

    let mut text = String::new();
    text.push_str("## Web Answer\n");
    let mut source_extracts = String::new();
    let mut web_refs = Vec::<source_ledger::SourceRef>::new();
    let mut citation_sources = Vec::<(String, String)>::new();
    let mut summary_points = Vec::<String>::new();

    let mut added = 0usize;
    for result in results
        .results
        .iter()
        .take(web_cfg.max_pages_per_task.max(1))
    {
        citation_sources.push((result.url.clone(), result.title.clone()));
        web_refs.push(source_ledger::SourceRef {
            source_type: "web_search_result".to_string(),
            source_id: result.normalized_url.clone(),
            hash: result.content_hash_prefix.clone(),
            redacted: false,
            truncated: false,
            observed_at: result.retrieved_at,
            detail: Some(format!(
                "provider={} rank={} status={} content_type={} final_url={} bytes={} extraction={}",
                result.provider,
                result.rank,
                result
                    .status
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                result
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                result
                    .final_url
                    .clone()
                    .unwrap_or_else(|| "none".to_string()),
                result
                    .response_bytes
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                result
                    .extraction_method
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
            )),
            section: None,
            offset_start: None,
            offset_end: None,
            extraction_method: result.extraction_method.clone(),
            extracted_at: None,
            index_version: None,
        });
        let page = match cached_web_read(
            state,
            &read_client,
            &WebReadRequest {
                url: result.url.clone(),
                max_bytes: web_cfg.max_fetch_bytes,
                allow_plain_http: web_cfg.allow_plain_http,
                allow_local_addresses: web_cfg.allow_local_addresses,
                user_agent: web_cfg.user_agent.clone(),
                obey_robots_txt: web_cfg.obey_robots_txt,
                min_delay_between_requests_ms: web_cfg.min_delay_between_requests_ms,
                robots_cache_ttl_secs: web_cfg.cache_ttl_secs,
                extract_strategy: web_cfg.extract.strategy.clone(),
                include_images: web_cfg.extract.include_images,
                include_links: web_cfg.extract.include_links,
                include_tables: web_cfg.extract.include_tables,
                max_table_rows: web_cfg.extract.max_table_rows,
                max_table_cols: web_cfg.extract.max_table_cols,
                preserve_code_blocks: web_cfg.extract.preserve_code_blocks,
                strip_tracking_params: web_cfg.extract.strip_tracking_params,
                max_html_bytes: web_cfg.extract.max_html_bytes,
                max_markdown_bytes: web_cfg.extract.max_markdown_bytes,
                min_extracted_chars: web_cfg.extract.min_extracted_chars,
                dedupe_boilerplate: web_cfg.extract.dedupe_boilerplate,
            },
            web_cfg.min_delay_between_requests_ms,
        )
        .await
        {
            Ok((p, _)) => p,
            Err(e) => {
                source_extracts.push_str(&format!("- {} (fetch failed: {e})\n", result.url));
                if !result.snippet.trim().is_empty() {
                    let label = if result.title.trim().is_empty() {
                        result.url.clone()
                    } else {
                        result.title.clone()
                    };
                    summary_points.push(format!(
                        "{label}: {}",
                        truncate_string(result.snippet.trim(), 220)
                    ));
                }
                continue;
            }
        };

        added += 1;
        source_extracts.push_str(&format!("\n### Result {added}\n"));
        source_extracts.push_str(&format!("Source: {}\n", page.final_url));
        let page_title = page.title.clone();
        if let Some(title) = page_title.as_ref() {
            source_extracts.push_str(&format!("Title: {title}\n"));
        }
        let label = page_title
            .clone()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| page.final_url.clone());
        let summary_seed = page
            .markdown
            .lines()
            .map(str::trim)
            .find(|line| {
                !line.is_empty()
                    && !line.starts_with('#')
                    && !line.starts_with("```")
                    && !line.starts_with('[')
            })
            .map(std::borrow::ToOwned::to_owned)
            .filter(|line| line.len() > 20)
            .unwrap_or_else(|| result.snippet.clone());
        if !summary_seed.trim().is_empty() {
            summary_points.push(format!(
                "{label}: {}",
                truncate_string(summary_seed.trim(), 220)
            ));
        }
        citation_sources.push((
            page.final_url.clone(),
            page_title.clone().unwrap_or_else(|| "web page".to_string()),
        ));
        source_extracts.push('\n');
        source_extracts.push_str(&truncate_string(&page.markdown, 900));
        source_extracts.push('\n');
        web_refs.push(source_ledger::SourceRef {
            source_type: "web_read_page".to_string(),
            source_id: page.normalized_url.clone(),
            hash: page.content_hash_prefix.clone(),
            redacted: false,
            truncated: page.truncated,
            observed_at: page.retrieved_at,
            detail: Some(format!(
                "status={} extraction_status={} fetched_bytes={} extracted_bytes={} final_url={}",
                page.status,
                page.extraction_status,
                page.fetched_bytes,
                page.extracted_bytes,
                page.final_url
            )),
            section: page_title,
            offset_start: None,
            offset_end: None,
            extraction_method: Some(page.extraction_method.clone()),
            extracted_at: None,
            index_version: None,
        });
    }

    if added == 0 && summary_points.is_empty() {
        text.push_str("I could not extract readable web pages for this query.");
    } else if summary_points.is_empty() {
        text.push_str("I retrieved sources but could not extract a concise summary.");
    } else {
        text.push_str("Based on fetched sources:\n");
        for (idx, point) in summary_points.iter().take(6).enumerate() {
            text.push_str(&format!("{}. {}\n", idx + 1, point));
        }
    }

    if web_cfg.citation_required {
        text.push_str("\n\n## Citations\n");
        let mut seen = BTreeSet::<String>::new();
        let mut citation_index = 0usize;
        for (url, label) in citation_sources {
            if !seen.insert(url.clone()) {
                continue;
            }
            citation_index += 1;
            if label.trim().is_empty() {
                text.push_str(&format!("[{citation_index}] {url}\n"));
            } else {
                text.push_str(&format!("[{citation_index}] {label} | {url}\n"));
            }
        }
        if citation_index == 0 {
            text.push_str("[1] no citation sources available\n");
        }
    }

    if !source_extracts.trim().is_empty() {
        text.push_str("\n\n## Source Extracts\n");
        text.push_str(&truncate_string(&source_extracts, 4000));
    }

    append_source_refs(state, web_refs).await;

    transport
        .send(ServerMessage::ModelText {
            task_id: payload.task_id,
            chunk: text.clone(),
        })
        .await?;
    transport
        .send(ServerMessage::TaskComplete {
            task_id: payload.task_id,
            reason: TaskCompleteReason::ModelDone,
            summary: format!(
                "Web request completed with {} source(s).",
                added.max(summary_points.len())
            ),
        })
        .await?;
    append_session_turn_if_session_mode(
        state,
        &payload.mode,
        payload.shell_id,
        payload.prompt.clone(),
        truncate_string(&text, 1200),
    )
    .await;
    Ok(())
}

async fn emit_local_tool_access_denied(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: &termlm_protocol::StartTask,
    reason: &str,
    tool_name: &str,
    cwd: &Path,
) -> Result<()> {
    let msg = format!(
        "local tool access denied ({tool_name}): reason={reason}; cwd={}",
        cwd.display()
    );
    transport
        .send(ServerMessage::ModelText {
            task_id: payload.task_id,
            chunk: msg.clone(),
        })
        .await?;
    transport
        .send(ServerMessage::TaskComplete {
            task_id: payload.task_id,
            reason: TaskCompleteReason::ModelDone,
            summary: "Local tool access denied.".to_string(),
        })
        .await?;
    append_session_turn_if_session_mode(
        state,
        &payload.mode,
        payload.shell_id,
        payload.prompt.clone(),
        truncate_string(&msg, 1200),
    )
    .await;
    Ok(())
}

fn local_text_detection_options(cfg: &AppConfig) -> termlm_local_tools::TextDetectionOptions {
    termlm_local_tools::TextDetectionOptions {
        sample_bytes: cfg.local_tools.text_detection.sample_bytes.max(1),
        reject_nul_bytes: cfg.local_tools.text_detection.reject_nul_bytes,
        accepted_encodings: cfg.local_tools.text_detection.accepted_encodings.clone(),
        deny_binary_magic: cfg.local_tools.text_detection.deny_binary_magic,
    }
}

async fn try_handle_local_tool_request(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: &termlm_protocol::StartTask,
) -> Result<bool> {
    let cfg = state.config_snapshot();
    let prompt = payload.prompt.trim();
    let lower = prompt.to_ascii_lowercase();
    let cwd = PathBuf::from(&payload.cwd);
    let workspace = termlm_local_tools::resolve_workspace_root_with_markers(
        &cwd,
        None,
        cfg.local_tools.allow_home_as_workspace,
        cfg.local_tools.allow_system_dirs,
        &cfg.local_tools.workspace_markers,
    );

    if lower.starts_with("read file ") {
        let Some(root) = workspace.root.as_ref() else {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                &workspace.reason,
                "read_file",
                &cwd,
            )
            .await?;
            return Ok(true);
        };
        if is_sensitive_local_path(root, cfg.as_ref()) {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                "access_denied_sensitive_path",
                "read_file",
                &cwd,
            )
            .await?;
            return Ok(true);
        }
        let target = prompt["read file ".len()..].trim();
        let Some(target_path) = resolve_path_within_root(root, Path::new(target)) else {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                "access_denied_sensitive_path",
                "read_file",
                &cwd,
            )
            .await?;
            return Ok(true);
        };
        if is_sensitive_local_path(&target_path, cfg.as_ref()) {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                "access_denied_sensitive_path",
                "read_file",
                &cwd,
            )
            .await?;
            return Ok(true);
        }
        let out = termlm_local_tools::read_file_with_detection(
            &target_path,
            cfg.local_tools.default_max_bytes,
            &local_text_detection_options(cfg.as_ref()),
        );
        let msg = match out {
            Ok(r) => format!(
                "## read_file\nPath: {}\nWorkspace Root: {}\nDetector: {} ({})\n\n{}",
                r.path,
                root.display(),
                r.detector,
                r.encoding,
                truncate_string(&r.content, 5000)
            ),
            Err(e) => format!("read_file failed: {e}"),
        };
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: msg.clone(),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: "Local tool read_file completed.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            truncate_string(&msg, 1200),
        )
        .await;
        return Ok(true);
    }

    if let Some(q) = lower.strip_prefix("search files for ") {
        let Some(root) = workspace.root.as_ref() else {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                &workspace.reason,
                "search_files",
                &cwd,
            )
            .await?;
            return Ok(true);
        };
        if is_sensitive_local_path(root, cfg.as_ref()) {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                "access_denied_sensitive_path",
                "search_files",
                &cwd,
            )
            .await?;
            return Ok(true);
        }
        let query = q.trim();
        let out = termlm_local_tools::search_files(
            root,
            query,
            termlm_local_tools::SearchFilesOptions {
                glob: None,
                regex_mode: false,
                max_results: cfg.local_tools.max_search_results,
                max_files_scanned: cfg.local_tools.max_search_files,
                max_bytes_per_file: cfg.local_tools.max_file_bytes,
                include_hidden: cfg.local_tools.include_hidden_by_default,
                respect_gitignore: cfg.local_tools.respect_gitignore,
                text_detection: local_text_detection_options(cfg.as_ref()),
            },
        );
        let msg = match out {
            Ok(r) => {
                let mut lines = vec![format!("## search_files\nRoot: {}", r.root)];
                for m in r.matches.iter().take(50) {
                    lines.push(format!("{}:{}: {}", m.path, m.line, m.text));
                }
                if r.truncated {
                    lines.push("… [truncated]".to_string());
                }
                lines.join("\n")
            }
            Err(e) => format!("search_files failed: {e}"),
        };
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: truncate_string(&msg, 6000),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: "Local tool search_files completed.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            truncate_string(&msg, 1200),
        )
        .await;
        return Ok(true);
    }

    if lower.contains("list workspace files") || lower.contains("show workspace files") {
        let Some(root) = workspace.root.as_ref() else {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                &workspace.reason,
                "list_workspace_files",
                &cwd,
            )
            .await?;
            return Ok(true);
        };
        if is_sensitive_local_path(root, cfg.as_ref()) {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                "access_denied_sensitive_path",
                "list_workspace_files",
                &cwd,
            )
            .await?;
            return Ok(true);
        }
        let out = termlm_local_tools::list_workspace_files(
            root,
            cfg.local_tools.max_workspace_entries,
            6,
            false,
        );
        let msg = match out {
            Ok(r) => {
                let mut lines = vec![format!("## list_workspace_files\nRoot: {}", r.root)];
                for e in r.entries.iter().take(120) {
                    lines.push(format!("{} {}", e.kind, e.path));
                }
                if r.truncated {
                    lines.push("… [truncated]".to_string());
                }
                lines.join("\n")
            }
            Err(e) => format!("list_workspace_files failed: {e}"),
        };
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: truncate_string(&msg, 6000),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: "Local tool list_workspace_files completed.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            truncate_string(&msg, 1200),
        )
        .await;
        return Ok(true);
    }

    if cfg.project_metadata.enabled && lower.contains("project metadata") {
        let Some(root) = workspace.root.as_ref() else {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                &workspace.reason,
                "project_metadata",
                &cwd,
            )
            .await?;
            return Ok(true);
        };
        if is_sensitive_local_path(root, cfg.as_ref()) {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                "access_denied_sensitive_path",
                "project_metadata",
                &cwd,
            )
            .await?;
            return Ok(true);
        }
        let out = termlm_local_tools::project_metadata(
            root,
            termlm_local_tools::ProjectMetadataOptions {
                max_files_read: cfg.project_metadata.max_files_read,
                max_bytes_per_file: cfg.project_metadata.max_bytes_per_file,
                detect_scripts: cfg.project_metadata.detect_scripts,
                detect_package_managers: cfg.project_metadata.detect_package_managers,
                detect_ci: cfg.project_metadata.detect_ci,
            },
        );
        let msg = match out {
            Ok(r) => serde_json::to_string_pretty(&r)
                .unwrap_or_else(|_| "project_metadata serialization failed".to_string()),
            Err(e) => format!("project_metadata failed: {e}"),
        };
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: format!("## project_metadata\n{msg}"),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: "Local tool project_metadata completed.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            truncate_string(&msg, 1200),
        )
        .await;
        return Ok(true);
    }

    if cfg.git_context.enabled
        && (lower.contains("git context") || lower.contains("git status context"))
    {
        let Some(root) = workspace.root.as_ref() else {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                &workspace.reason,
                "git_context",
                &cwd,
            )
            .await?;
            return Ok(true);
        };
        if is_sensitive_local_path(root, cfg.as_ref()) {
            emit_local_tool_access_denied(
                state,
                transport,
                payload,
                "access_denied_sensitive_path",
                "git_context",
                &cwd,
            )
            .await?;
            return Ok(true);
        }
        let out = termlm_local_tools::git_context(
            root,
            termlm_local_tools::GitContextOptions {
                max_changed_files: cfg.git_context.max_changed_files,
                max_recent_commits: cfg.git_context.max_recent_commits,
                include_diff_summary: cfg.git_context.include_diff_summary,
                max_diff_bytes: cfg.git_context.max_diff_bytes,
            },
        );
        let msg = match out {
            Ok(r) => serde_json::to_string_pretty(&r)
                .unwrap_or_else(|_| "git_context serialization failed".to_string()),
            Err(e) => format!("git_context failed: {e}"),
        };
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: format!("## git_context\n{msg}"),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: "Local tool git_context completed.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            truncate_string(&msg, 1200),
        )
        .await;
        return Ok(true);
    }

    if let Some(q) = lower.strip_prefix("search terminal context for ") {
        let query = q.trim();
        let entries = {
            let observed = state.observed.lock().await;
            observed
                .iter()
                .cloned()
                .map(|e| termlm_local_tools::ObservedTerminalEntry {
                    command_seq: e.command_seq,
                    command: e.command,
                    cwd: e.cwd,
                    started_at: e.started_at,
                    duration_ms: e.duration_ms,
                    exit_code: e.exit_code,
                    detected_urls: e.detected_urls,
                    stderr_head: e.stderr_head,
                    stderr_tail: e.stderr_tail,
                    stdout_head: e.stdout_head,
                    stdout_tail: e.stdout_tail,
                    stdout_full_ref: e.stdout_full_ref,
                    stderr_full_ref: e.stderr_full_ref,
                })
                .collect::<Vec<_>>()
        };
        let result = termlm_local_tools::search_terminal_context(&entries, query, 25);
        let msg = serde_json::to_string_pretty(&result)
            .unwrap_or_else(|_| "search_terminal_context serialization failed".to_string());
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: format!("## search_terminal_context\n{msg}"),
            })
            .await?;
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: "Local tool search_terminal_context completed.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            truncate_string(&msg, 1200),
        )
        .await;
        return Ok(true);
    }

    Ok(false)
}

async fn handle_readonly_tool_call(
    state: &Arc<DaemonState>,
    payload: &termlm_protocol::StartTask,
    _session: &ShellSession,
    call: &termlm_inference::ToolCall,
) -> Result<Option<(String, String)>> {
    let cfg = state.config_snapshot();
    let cwd = PathBuf::from(&payload.cwd);
    let workspace = termlm_local_tools::resolve_workspace_root_with_markers(
        &cwd,
        None,
        cfg.local_tools.allow_home_as_workspace,
        cfg.local_tools.allow_system_dirs,
        &cfg.local_tools.workspace_markers,
    );

    let render_json = |value: serde_json::Value| -> String {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
    };
    let denied_root_result = |reason: &str, requested_root: Option<String>| {
        let error_code = if reason == "no_workspace_detected_system_directory" {
            "no_workspace_detected_system_directory"
        } else {
            "access_denied_sensitive_path"
        };
        serde_json::json!({
            "error": error_code,
            "reason": reason,
            "requested_root": requested_root,
            "cwd": cwd.display().to_string(),
            "resolved_workspace_root": serde_json::Value::Null
        })
    };
    let resolve_tool_root =
        |requested_root: Option<&str>| -> std::result::Result<PathBuf, serde_json::Value> {
            let root = if let Some(r) = requested_root {
                let explicit = if Path::new(r).is_absolute() {
                    PathBuf::from(r)
                } else if let Some(base) = workspace.root.as_ref() {
                    base.join(r)
                } else {
                    cwd.join(r)
                };
                let resolved = termlm_local_tools::resolve_workspace_root_with_markers(
                    &cwd,
                    Some(&explicit),
                    cfg.local_tools.allow_home_as_workspace,
                    cfg.local_tools.allow_system_dirs,
                    &cfg.local_tools.workspace_markers,
                );
                if let Some(root) = resolved.root {
                    root
                } else {
                    return Err(denied_root_result(
                        &resolved.reason,
                        Some(explicit.display().to_string()),
                    ));
                }
            } else if let Some(root) = workspace.root.as_ref() {
                root.clone()
            } else {
                return Err(denied_root_result(&workspace.reason, None));
            };
            if is_sensitive_local_path(&root, cfg.as_ref()) {
                return Err(denied_root_result(
                    "access_denied_sensitive_path",
                    Some(root.display().to_string()),
                ));
            }
            Ok(root)
        };

    match call.name.as_str() {
        "read_file" if cfg.local_tools.enabled => {
            let Some(path) = call.arguments.get("path").and_then(|v| v.as_str()) else {
                return Ok(Some((
                    "read_file".to_string(),
                    render_json(serde_json::json!({"error":"missing_path"})),
                )));
            };
            let start_line = call
                .arguments
                .get("start_line")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1) as usize;
            let max_lines = call
                .arguments
                .get("max_lines")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let max_bytes = call
                .arguments
                .get("max_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(cfg.local_tools.default_max_bytes as u64)
                .min(cfg.local_tools.max_file_bytes as u64) as usize;
            let text_detection = local_text_detection_options(cfg.as_ref());
            let base_root = match resolve_tool_root(None) {
                Ok(root) => root,
                Err(denied) => {
                    return Ok(Some(("read_file".to_string(), render_json(denied))));
                }
            };
            let target = match resolve_path_within_root(&base_root, Path::new(path)) {
                Some(path) => path,
                None => {
                    return Ok(Some((
                        "read_file".to_string(),
                        render_json(denied_root_result(
                            "access_denied_sensitive_path",
                            Some(path.to_string()),
                        )),
                    )));
                }
            };
            if is_sensitive_local_path(&target, cfg.as_ref()) {
                return Ok(Some((
                    "read_file".to_string(),
                    render_json(denied_root_result(
                        "access_denied_sensitive_path",
                        Some(path.to_string()),
                    )),
                )));
            }
            let read_cache_key = if cfg.cache.enabled {
                file_read_cache_key(&target, start_line, max_lines, max_bytes, &text_detection)
            } else {
                None
            };
            if let Some(key) = read_cache_key.as_ref()
                && let Some(cached) = state.file_read_cache.lock().await.get(key)
            {
                append_source_refs(state, cached.refs.clone()).await;
                return Ok(Some(("read_file".to_string(), cached.rendered)));
            }
            let (result, refs) = match termlm_local_tools::read_file_with_detection(
                &target,
                max_bytes,
                &text_detection,
            ) {
                Ok(r) => {
                    let (content, offset_start, offset_end) = if max_lines > 0 || start_line > 1 {
                        let lines = r.content.lines().collect::<Vec<_>>();
                        let start_idx = start_line.saturating_sub(1).min(lines.len());
                        let end_idx = if max_lines == 0 {
                            lines.len()
                        } else {
                            start_idx.saturating_add(max_lines).min(lines.len())
                        };
                        let sliced = lines[start_idx..end_idx].join("\n");
                        let line_count = sliced.lines().count();
                        let start = if line_count == 0 {
                            None
                        } else {
                            Some((start_idx + 1) as u64)
                        };
                        let end = start.map(|s| s + line_count.saturating_sub(1) as u64);
                        (sliced, start, end)
                    } else {
                        let line_count = r.content.lines().count();
                        let start = if line_count == 0 { None } else { Some(1) };
                        let end = start.map(|s| s + line_count.saturating_sub(1) as u64);
                        (r.content.clone(), start, end)
                    };
                    (
                        serde_json::json!({
                            "path": r.path,
                            "cwd": cwd.display().to_string(),
                            "workspace_root": base_root.display().to_string(),
                            "content": content,
                            "start_line": start_line,
                            "max_lines": max_lines,
                            "redacted": r.redacted,
                            "truncated": r.truncated,
                            "bytes_read": r.bytes_read,
                            "detector": r.detector,
                            "encoding": r.encoding,
                            "detection_reason": r.detection_reason
                        }),
                        vec![source_ledger::SourceRef {
                            source_type: "local_file_read".to_string(),
                            source_id: target.display().to_string(),
                            hash: hash_prefix(&r.content),
                            redacted: true,
                            truncated: r.truncated,
                            observed_at: chrono::Utc::now(),
                            detail: Some(format!(
                                "bytes_read={} detector={} encoding={}",
                                r.bytes_read, r.detector, r.encoding
                            )),
                            section: None,
                            offset_start,
                            offset_end,
                            extraction_method: None,
                            extracted_at: None,
                            index_version: None,
                        }],
                    )
                }
                Err(e) => (
                    serde_json::json!({"error":"read_failed","message": e.to_string()}),
                    Vec::new(),
                ),
            };
            let rendered = render_json(result);
            if cfg.cache.enabled
                && let Some(key) = read_cache_key
            {
                state.file_read_cache.lock().await.insert(
                    key,
                    CachedToolResult {
                        rendered: rendered.clone(),
                        refs: refs.clone(),
                    },
                );
            }
            append_source_refs(state, refs).await;
            Ok(Some(("read_file".to_string(), rendered)))
        }
        "search_files" if cfg.local_tools.enabled => {
            let Some(query) = call.arguments.get("query").and_then(|v| v.as_str()) else {
                return Ok(Some((
                    "search_files".to_string(),
                    render_json(serde_json::json!({"error":"missing_query"})),
                )));
            };
            let requested_root = call.arguments.get("root").and_then(|v| v.as_str());
            let resolved_root = match resolve_tool_root(requested_root) {
                Ok(root) => root,
                Err(denied) => {
                    return Ok(Some(("search_files".to_string(), render_json(denied))));
                }
            };
            let glob = call.arguments.get("glob").and_then(|v| v.as_str());
            let regex_mode = call
                .arguments
                .get("regex")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let include_hidden = call
                .arguments
                .get("include_hidden")
                .and_then(|v| v.as_bool())
                .unwrap_or(cfg.local_tools.include_hidden_by_default);
            let max_bytes_per_file =
                call.arguments
                    .get("max_bytes_per_file")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(cfg.local_tools.max_file_bytes as u64)
                    .min(cfg.local_tools.max_file_bytes as u64) as usize;
            let max_results =
                call.arguments
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(cfg.local_tools.max_search_results as u64) as usize;
            let respect_gitignore = call
                .arguments
                .get("respect_gitignore")
                .and_then(|v| v.as_bool())
                .unwrap_or(cfg.local_tools.respect_gitignore);
            let (result, refs) = match termlm_local_tools::search_files(
                &resolved_root,
                query,
                termlm_local_tools::SearchFilesOptions {
                    glob,
                    regex_mode,
                    max_results: max_results.min(cfg.local_tools.max_search_results),
                    max_files_scanned: cfg.local_tools.max_search_files,
                    max_bytes_per_file,
                    include_hidden,
                    respect_gitignore,
                    text_detection: local_text_detection_options(cfg.as_ref()),
                },
            ) {
                Ok(r) => {
                    let refs = r
                        .matches
                        .iter()
                        .take(25)
                        .map(|m| source_ledger::SourceRef {
                            source_type: "local_search_match".to_string(),
                            source_id: format!("{}:{}", m.path, m.line),
                            hash: hash_prefix(&m.text),
                            redacted: true,
                            truncated: r.truncated,
                            observed_at: chrono::Utc::now(),
                            detail: Some(format!("query={query}")),
                            section: None,
                            offset_start: Some(m.line as u64),
                            offset_end: Some(m.line as u64),
                            extraction_method: None,
                            extracted_at: None,
                            index_version: None,
                        })
                        .collect::<Vec<_>>();
                    (
                        serde_json::to_value(r).unwrap_or_else(|_| serde_json::json!({})),
                        refs,
                    )
                }
                Err(e) => (
                    serde_json::json!({"error":"search_failed","message": e.to_string()}),
                    Vec::new(),
                ),
            };
            append_source_refs(state, refs).await;
            Ok(Some(("search_files".to_string(), render_json(result))))
        }
        "list_workspace_files" if cfg.local_tools.enabled => {
            let requested_root = call.arguments.get("root").and_then(|v| v.as_str());
            let resolved_root = match resolve_tool_root(requested_root) {
                Ok(root) => root,
                Err(denied) => {
                    return Ok(Some((
                        "list_workspace_files".to_string(),
                        render_json(denied),
                    )));
                }
            };
            let max_entries = call
                .arguments
                .get("max_entries")
                .and_then(|v| v.as_u64())
                .unwrap_or(cfg.local_tools.max_workspace_entries as u64)
                as usize;
            let max_depth = call
                .arguments
                .get("max_depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(6) as usize;
            let include_hidden = call
                .arguments
                .get("include_hidden")
                .and_then(|v| v.as_bool())
                .unwrap_or(cfg.local_tools.include_hidden_by_default);
            let (result, refs) = match termlm_local_tools::list_workspace_files(
                &resolved_root,
                max_entries,
                max_depth,
                include_hidden,
            ) {
                Ok(r) => (
                    serde_json::to_value(&r).unwrap_or_else(|_| serde_json::json!({})),
                    vec![source_ledger::SourceRef {
                        source_type: "workspace_listing".to_string(),
                        source_id: r.root.clone(),
                        hash: hash_prefix(&format!(
                            "{}:{}:{}",
                            r.root,
                            r.entries.len(),
                            r.truncated
                        )),
                        redacted: false,
                        truncated: r.truncated,
                        observed_at: chrono::Utc::now(),
                        detail: Some(format!("entries={}", r.entries.len())),
                        section: None,
                        offset_start: None,
                        offset_end: None,
                        extraction_method: None,
                        extracted_at: None,
                        index_version: None,
                    }],
                ),
                Err(e) => (
                    serde_json::json!({"error":"list_workspace_failed","message": e.to_string()}),
                    Vec::new(),
                ),
            };
            append_source_refs(state, refs).await;
            Ok(Some((
                "list_workspace_files".to_string(),
                render_json(result),
            )))
        }
        "project_metadata" if cfg.local_tools.enabled && cfg.project_metadata.enabled => {
            let requested_root = call.arguments.get("root").and_then(|v| v.as_str());
            let resolved_root = match resolve_tool_root(requested_root) {
                Ok(root) => root,
                Err(denied) => {
                    return Ok(Some(("project_metadata".to_string(), render_json(denied))));
                }
            };
            let include_scripts = call
                .arguments
                .get("include_scripts")
                .and_then(|v| v.as_bool())
                .unwrap_or(cfg.project_metadata.detect_scripts);
            let include_ci = call
                .arguments
                .get("include_ci")
                .and_then(|v| v.as_bool())
                .unwrap_or(cfg.project_metadata.detect_ci);
            let max_files_read =
                call.arguments
                    .get("max_files_read")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(cfg.project_metadata.max_files_read as u64)
                    .min(cfg.project_metadata.max_files_read as u64) as usize;
            let max_bytes_per_file =
                call.arguments
                    .get("max_bytes_per_file")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(cfg.project_metadata.max_bytes_per_file as u64)
                    .min(cfg.project_metadata.max_bytes_per_file as u64) as usize;
            let metadata_cache_key = if cfg.cache.enabled {
                Some(project_metadata_cache_key(
                    &resolved_root,
                    include_scripts,
                    include_ci,
                    max_files_read,
                    max_bytes_per_file,
                    cfg.project_metadata.detect_package_managers,
                ))
            } else {
                None
            };
            if let Some(key) = metadata_cache_key.as_ref()
                && let Some(cached) = state.project_metadata_cache.lock().await.get(key)
            {
                append_source_refs(state, cached.refs.clone()).await;
                return Ok(Some(("project_metadata".to_string(), cached.rendered)));
            }
            let (result, refs) = match termlm_local_tools::project_metadata(
                &resolved_root,
                termlm_local_tools::ProjectMetadataOptions {
                    max_files_read,
                    max_bytes_per_file,
                    detect_scripts: include_scripts,
                    detect_package_managers: cfg.project_metadata.detect_package_managers,
                    detect_ci: include_ci,
                },
            ) {
                Ok(r) => (
                    serde_json::to_value(&r).unwrap_or_else(|_| serde_json::json!({})),
                    vec![source_ledger::SourceRef {
                        source_type: "project_metadata".to_string(),
                        source_id: r.root.clone(),
                        hash: hash_prefix(
                            &serde_json::to_string(&r).unwrap_or_else(|_| String::new()),
                        ),
                        redacted: false,
                        truncated: false,
                        observed_at: chrono::Utc::now(),
                        detail: Some("project metadata snapshot".to_string()),
                        section: None,
                        offset_start: None,
                        offset_end: None,
                        extraction_method: None,
                        extracted_at: None,
                        index_version: None,
                    }],
                ),
                Err(e) => (
                    serde_json::json!({"error":"project_metadata_failed","message": e.to_string()}),
                    Vec::new(),
                ),
            };
            let rendered = render_json(result);
            if cfg.cache.enabled
                && let Some(key) = metadata_cache_key
            {
                state.project_metadata_cache.lock().await.insert(
                    key,
                    CachedToolResult {
                        rendered: rendered.clone(),
                        refs: refs.clone(),
                    },
                );
            }
            append_source_refs(state, refs).await;
            Ok(Some(("project_metadata".to_string(), rendered)))
        }
        "git_context" if cfg.local_tools.enabled && cfg.git_context.enabled => {
            let requested_root = call.arguments.get("root").and_then(|v| v.as_str());
            let resolved_root = match resolve_tool_root(requested_root) {
                Ok(root) => root,
                Err(denied) => {
                    return Ok(Some(("git_context".to_string(), render_json(denied))));
                }
            };
            let include_diff_summary = call
                .arguments
                .get("include_diff_summary")
                .and_then(|v| v.as_bool())
                .unwrap_or(cfg.git_context.include_diff_summary);
            let max_files = call
                .arguments
                .get("max_files")
                .and_then(|v| v.as_u64())
                .unwrap_or(cfg.git_context.max_changed_files as u64)
                .min(cfg.git_context.max_changed_files as u64) as usize;
            let max_recent_commits =
                call.arguments
                    .get("max_recent_commits")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(cfg.git_context.max_recent_commits as u64)
                    .min(cfg.git_context.max_recent_commits as u64) as usize;
            let max_diff_bytes =
                call.arguments
                    .get("max_diff_bytes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(cfg.git_context.max_diff_bytes as u64)
                    .min(cfg.git_context.max_diff_bytes as u64) as usize;
            let git_cache_key = if cfg.cache.enabled {
                Some(git_context_cache_key(
                    &resolved_root,
                    include_diff_summary,
                    max_files,
                    max_recent_commits,
                    max_diff_bytes,
                ))
            } else {
                None
            };
            if let Some(key) = git_cache_key.as_ref()
                && let Some(cached) = state.git_context_cache.lock().await.get(key)
            {
                append_source_refs(state, cached.refs.clone()).await;
                return Ok(Some(("git_context".to_string(), cached.rendered)));
            }
            let (result, refs) = match termlm_local_tools::git_context(
                &resolved_root,
                termlm_local_tools::GitContextOptions {
                    max_changed_files: max_files,
                    max_recent_commits,
                    include_diff_summary,
                    max_diff_bytes,
                },
            ) {
                Ok(r) => (
                    serde_json::to_value(&r).unwrap_or_else(|_| serde_json::json!({})),
                    vec![source_ledger::SourceRef {
                        source_type: "git_context".to_string(),
                        source_id: r
                            .root
                            .clone()
                            .unwrap_or_else(|| resolved_root.display().to_string()),
                        hash: hash_prefix(
                            &serde_json::to_string(&r).unwrap_or_else(|_| String::new()),
                        ),
                        redacted: false,
                        truncated: false,
                        observed_at: chrono::Utc::now(),
                        detail: Some(format!(
                            "branch={} dirty={} conflicts={}",
                            r.branch.clone().unwrap_or_else(|| "detached".to_string()),
                            r.dirty,
                            !r.conflict_files.is_empty()
                        )),
                        section: None,
                        offset_start: None,
                        offset_end: None,
                        extraction_method: None,
                        extracted_at: None,
                        index_version: None,
                    }],
                ),
                Err(e) => (
                    serde_json::json!({"error":"git_context_failed","message": e.to_string()}),
                    Vec::new(),
                ),
            };
            let rendered = render_json(result);
            if cfg.cache.enabled
                && let Some(key) = git_cache_key
            {
                state.git_context_cache.lock().await.insert(
                    key,
                    CachedToolResult {
                        rendered: rendered.clone(),
                        refs: refs.clone(),
                    },
                );
            }
            append_source_refs(state, refs).await;
            Ok(Some(("git_context".to_string(), rendered)))
        }
        "search_terminal_context" if cfg.local_tools.enabled => {
            let Some(query) = call.arguments.get("query").and_then(|v| v.as_str()) else {
                return Ok(Some((
                    "search_terminal_context".to_string(),
                    render_json(serde_json::json!({"error":"missing_query"})),
                )));
            };
            let max_results = call
                .arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(25) as usize;
            let entries = {
                let observed = state.observed.lock().await;
                observed
                    .iter()
                    .filter(|e| e.shell_id == payload.shell_id)
                    .cloned()
                    .map(|e| termlm_local_tools::ObservedTerminalEntry {
                        command_seq: e.command_seq,
                        command: e.command,
                        cwd: e.cwd,
                        started_at: e.started_at,
                        duration_ms: e.duration_ms,
                        exit_code: e.exit_code,
                        detected_urls: e.detected_urls,
                        stderr_head: e.stderr_head,
                        stderr_tail: e.stderr_tail,
                        stdout_head: e.stdout_head,
                        stdout_tail: e.stdout_tail,
                        stdout_full_ref: e.stdout_full_ref,
                        stderr_full_ref: e.stderr_full_ref,
                    })
                    .collect::<Vec<_>>()
            };
            let result = termlm_local_tools::search_terminal_context(&entries, query, max_results);
            let refs = result
                .results
                .iter()
                .take(25)
                .map(|m| source_ledger::SourceRef {
                    source_type: "terminal_context_match".to_string(),
                    source_id: format!("{}:{}", payload.shell_id, m.command_seq),
                    hash: hash_prefix(&format!(
                        "{}|{}|{}",
                        m.command, m.stderr_tail, m.stdout_tail
                    )),
                    redacted: true,
                    truncated: false,
                    observed_at: m.started_at,
                    detail: Some(format!("query={query} exit_code={}", m.exit_code)),
                    section: Some(m.cwd.clone()),
                    offset_start: None,
                    offset_end: None,
                    extraction_method: None,
                    extracted_at: None,
                    index_version: None,
                })
                .collect::<Vec<_>>();
            append_source_refs(state, refs).await;
            Ok(Some((
                "search_terminal_context".to_string(),
                render_json(serde_json::to_value(result).unwrap_or_else(|_| serde_json::json!({}))),
            )))
        }
        "web_search" if cfg.web.enabled && cfg.web.expose_tools => {
            let Some(query) = call.arguments.get("query").and_then(|v| v.as_str()) else {
                return Ok(Some((
                    "web_search".to_string(),
                    render_json(serde_json::json!({"error":"missing_query"})),
                )));
            };
            let (search_cache_ttl_secs, read_cache_ttl_secs) =
                effective_web_cache_ttls(cfg.as_ref());
            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(cfg.web.connect_timeout_secs))
                .timeout(std::time::Duration::from_secs(cfg.web.request_timeout_secs))
                .redirect(web_read_redirect_policy(
                    cfg.web.allow_plain_http,
                    cfg.web.allow_local_addresses,
                    DEFAULT_MAX_REDIRECTS,
                ))
                .build()?;
            let web_cfg = WebRuntimeConfig {
                enabled: cfg.web.enabled,
                provider: cfg.web.provider.clone(),
                search_endpoint: cfg.web.search_endpoint.clone(),
                search_api_key_env: cfg.web.search_api_key_env.clone(),
                request_timeout_secs: cfg.web.request_timeout_secs,
                connect_timeout_secs: cfg.web.connect_timeout_secs,
                max_results: cfg.web.max_results as usize,
                max_fetch_bytes: cfg.web.max_fetch_bytes,
                max_pages_per_task: cfg.web.max_pages_per_task,
                cache_ttl_secs: read_cache_ttl_secs,
                cache_max_bytes: effective_web_cache_bytes(cfg.as_ref()),
                allowed_schemes: cfg.web.allowed_schemes.clone(),
                allow_plain_http: cfg.web.allow_plain_http,
                allow_local_addresses: cfg.web.allow_local_addresses,
                obey_robots_txt: cfg.web.obey_robots_txt,
                citation_required: cfg.web.citation_required,
                freshness_required_terms: cfg.web.freshness_required_terms.clone(),
                min_delay_between_requests_ms: cfg.web.min_delay_between_requests_ms,
                search_cache_ttl_secs,
                user_agent: cfg.web.user_agent.clone(),
                extract: WebExtractRuntimeConfig {
                    strategy: cfg.web.extract.strategy.clone(),
                    output_format: cfg.web.extract.output_format.clone(),
                    include_images: cfg.web.extract.include_images,
                    include_links: cfg.web.extract.include_links,
                    include_tables: cfg.web.extract.include_tables,
                    max_table_rows: cfg.web.extract.max_table_rows,
                    max_table_cols: cfg.web.extract.max_table_cols,
                    preserve_code_blocks: cfg.web.extract.preserve_code_blocks,
                    strip_tracking_params: cfg.web.extract.strip_tracking_params,
                    max_html_bytes: cfg.web.extract.max_html_bytes,
                    max_markdown_bytes: cfg.web.extract.max_markdown_bytes,
                    min_extracted_chars: cfg.web.extract.min_extracted_chars,
                    dedupe_boilerplate: cfg.web.extract.dedupe_boilerplate,
                },
            };
            let provider = match build_web_search_provider(&web_cfg, client) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(Some((
                        "web_search".to_string(),
                        render_json(serde_json::json!({
                            "error":"provider_unavailable",
                            "provider": cfg.web.provider,
                            "message": e.to_string()
                        })),
                    )));
                }
            };
            let max_results = call
                .arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(cfg.web.max_results as u64) as usize;
            let search_req = SearchRequest {
                query: query.to_string(),
                freshness: call
                    .arguments
                    .get("freshness")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string),
                max_results: max_results.min(cfg.web.max_results as usize),
            };
            let cache_id = web_search_provider_cache_id(&web_cfg);
            let (results, refs) = match cached_web_search(
                state,
                &cache_id,
                provider.as_ref(),
                &search_req,
                cfg.web.min_delay_between_requests_ms,
            )
            .await
            {
                Ok((r, cached)) => {
                    let refs = r
                        .results
                        .iter()
                        .map(|entry| source_ledger::SourceRef {
                            source_type: "web_search_result".to_string(),
                            source_id: entry.normalized_url.clone(),
                            hash: entry.content_hash_prefix.clone(),
                            redacted: false,
                            truncated: false,
                            observed_at: entry.retrieved_at,
                            detail: Some(format!(
                                "provider={} rank={} status={} content_type={} final_url={} bytes={} extraction={}",
                                entry.provider,
                                entry.rank,
                                entry
                                    .status
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "unknown".to_string()),
                                entry
                                    .content_type
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string()),
                                entry
                                    .final_url
                                    .clone()
                                    .unwrap_or_else(|| "none".to_string()),
                                entry
                                    .response_bytes
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "unknown".to_string()),
                                entry
                                    .extraction_method
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string()),
                            )),
                            section: None,
                            offset_start: None,
                            offset_end: None,
                            extraction_method: entry.extraction_method.clone(),
                            extracted_at: None,
                            index_version: None,
                        })
                        .collect::<Vec<_>>();
                    let mut value =
                        serde_json::to_value(r).unwrap_or_else(|_| serde_json::json!({}));
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("cached".to_string(), serde_json::Value::Bool(cached));
                    }
                    (value, refs)
                }
                Err(e) => (
                    serde_json::json!({"error":"search_unavailable","message": e.to_string()}),
                    Vec::new(),
                ),
            };
            append_source_refs(state, refs).await;
            Ok(Some(("web_search".to_string(), render_json(results))))
        }
        "web_read" if cfg.web.enabled && cfg.web.expose_tools => {
            let Some(url) = call.arguments.get("url").and_then(|v| v.as_str()) else {
                return Ok(Some((
                    "web_read".to_string(),
                    render_json(serde_json::json!({"error":"missing_url"})),
                )));
            };
            let max_bytes = call
                .arguments
                .get("max_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(cfg.web.max_fetch_bytes as u64) as usize;
            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(cfg.web.connect_timeout_secs))
                .timeout(std::time::Duration::from_secs(cfg.web.request_timeout_secs))
                .redirect(web_read_redirect_policy(
                    cfg.web.allow_plain_http,
                    cfg.web.allow_local_addresses,
                    DEFAULT_MAX_REDIRECTS,
                ))
                .build()?;
            let read_req = WebReadRequest {
                url: url.to_string(),
                max_bytes: max_bytes.min(cfg.web.max_fetch_bytes),
                allow_plain_http: cfg.web.allow_plain_http,
                allow_local_addresses: cfg.web.allow_local_addresses,
                user_agent: cfg.web.user_agent.clone(),
                obey_robots_txt: cfg.web.obey_robots_txt,
                min_delay_between_requests_ms: cfg.web.min_delay_between_requests_ms,
                robots_cache_ttl_secs: effective_web_cache_ttls(cfg.as_ref()).1,
                extract_strategy: cfg.web.extract.strategy.clone(),
                include_images: cfg.web.extract.include_images,
                include_links: cfg.web.extract.include_links,
                include_tables: cfg.web.extract.include_tables,
                max_table_rows: cfg.web.extract.max_table_rows,
                max_table_cols: cfg.web.extract.max_table_cols,
                preserve_code_blocks: cfg.web.extract.preserve_code_blocks,
                strip_tracking_params: cfg.web.extract.strip_tracking_params,
                max_html_bytes: cfg.web.extract.max_html_bytes,
                max_markdown_bytes: cfg.web.extract.max_markdown_bytes,
                min_extracted_chars: cfg.web.extract.min_extracted_chars,
                dedupe_boilerplate: cfg.web.extract.dedupe_boilerplate,
            };
            let (result, refs) = match cached_web_read(
                state,
                &client,
                &read_req,
                cfg.web.min_delay_between_requests_ms,
            )
            .await
            {
                Ok((r, cached)) => {
                    let refs = vec![source_ledger::SourceRef {
                        source_type: "web_read_page".to_string(),
                        source_id: r.normalized_url.clone(),
                        hash: r.content_hash_prefix.clone(),
                        redacted: false,
                        truncated: r.truncated,
                        observed_at: r.retrieved_at,
                        detail: Some(format!(
                            "status={} extraction_status={} fetched_bytes={} extracted_bytes={} final_url={}",
                            r.status,
                            r.extraction_status,
                            r.fetched_bytes,
                            r.extracted_bytes,
                            r.final_url
                        )),
                        section: r.title.clone(),
                        offset_start: None,
                        offset_end: None,
                        extraction_method: Some(r.extraction_method.clone()),
                        extracted_at: None,
                        index_version: None,
                    }];
                    let mut value =
                        serde_json::to_value(r).unwrap_or_else(|_| serde_json::json!({}));
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("cached".to_string(), serde_json::Value::Bool(cached));
                    }
                    (value, refs)
                }
                Err(e) => (
                    serde_json::json!({"error":"web_read_failed","message": e.to_string()}),
                    Vec::new(),
                ),
            };
            append_source_refs(state, refs).await;
            Ok(Some(("web_read".to_string(), render_json(result))))
        }
        _ => Ok(None),
    }
}

async fn try_provider_orchestration(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: &termlm_protocol::StartTask,
    session: &ShellSession,
    classification: &tasks::ClassificationResult,
    drafting_prompt: &str,
    provider_continuation: bool,
) -> Result<bool> {
    let cfg = state.config_snapshot();
    let profile = context::determine_tool_exposure(&classification.classification, cfg.as_ref());
    let expose_execute = profile.execute_shell_command;
    let expose_lookup = profile.lookup_command_docs;

    let mut tools = Vec::<ToolSchema>::new();
    if expose_execute {
        tools.push(tool_schema_execute_shell_command());
    }
    if expose_lookup {
        tools.push(tool_schema_lookup_docs());
    }
    let local_tools = local_tool_schemas_for_profile(cfg.as_ref(), &profile);
    if !local_tools.is_empty() {
        tools.extend(local_tools);
    }
    if profile.web_tools && cfg.web.enabled && cfg.web.expose_tools {
        tools.extend(web_tool_schemas());
    }

    if tools.is_empty() {
        return Ok(false);
    }
    *state.last_provider_usage.lock().await = ProviderUsageSnapshot::default();

    let mut system = system_prompt::build_system_prompt(
        &format!("{:?}", payload.shell_kind).to_ascii_lowercase(),
        &cfg.inference.provider,
        &payload.cwd,
        &cfg.approval.mode,
        cfg.capture.enabled,
        &classification.classification,
        classification.confidence,
    );
    let progress_note = {
        let progress = state.index_progress.lock().await;
        indexing_progress_banner(&progress)
    };
    if let Some(note) = progress_note {
        system.push('\n');
        system.push_str(&note);
    }
    if profile.web_tools && cfg.web.enabled && cfg.web.expose_tools && cfg.web.citation_required {
        system.push_str(
            "\nWhen using web sources, include explicit URL citations in your final answer. \
             If no reliable source is available, say so instead of claiming unsupported facts.",
        );
    }
    let alias_defs = session
        .context
        .as_ref()
        .map(|ctx| {
            ctx.aliases
                .iter()
                .map(|a| (a.name.clone(), a.expansion.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let function_names = session
        .context
        .as_ref()
        .map(|ctx| {
            ctx.functions
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let cheatsheet = {
        let runtime = state.index_runtime.lock().await;
        termlm_indexer::cheatsheet::build_cheatsheet(
            &runtime.chunks,
            cfg.indexer.cheatsheet_static_count,
            &alias_defs,
            &function_names,
            cfg.context_budget.cheat_sheet_tokens,
        )
    };
    system.push('\n');
    system.push_str(&cheatsheet);

    let messages = {
        let mut conversations = state.task_conversations.lock().await;
        if provider_continuation {
            let entry = conversations
                .entry(payload.task_id)
                .or_insert_with(|| vec![ChatMessage::system(system.clone())]);
            if entry.is_empty() {
                entry.push(ChatMessage::system(system.clone()));
            } else if entry[0].role == "system" {
                entry[0] = ChatMessage::system(system.clone());
            } else {
                entry.insert(0, ChatMessage::system(system.clone()));
            }
            entry.push(ChatMessage::user(drafting_prompt.to_string()));
            entry.clone()
        } else {
            let mut seed = vec![ChatMessage::system(system.clone())];
            if payload.mode == "/p" {
                drop(conversations);
                let prior = session_messages_for_shell(state, payload.shell_id, &system).await;
                let mut conversations = state.task_conversations.lock().await;
                seed.extend(prior);
                seed.push(ChatMessage::user(drafting_prompt.to_string()));
                conversations.insert(payload.task_id, seed.clone());
                seed
            } else {
                seed.push(ChatMessage::user(drafting_prompt.to_string()));
                conversations.insert(payload.task_id, seed.clone());
                seed
            }
        }
    };

    let chat_request = ChatRequest {
        task_id: Some(payload.task_id.to_string()),
        model: active_model_name(state.config_snapshot().as_ref()),
        messages,
        tools,
        stream: cfg.inference.stream,
        think: cfg.behavior.thinking,
        options: provider_request_options(cfg.as_ref()),
    };
    let stream_result = {
        let provider = state.provider.lock().await;
        provider.chat_stream(chat_request).await
    };
    let mut stream = match stream_result {
        Ok(stream) => stream,
        Err(e) => {
            cancel_provider_task(state, payload.task_id).await;
            transport
                .send(ServerMessage::Error {
                    task_id: Some(payload.task_id),
                    kind: ErrorKind::InferenceProviderUnavailable,
                    message: format!("provider request failed: {e}"),
                    matched_pattern: None,
                })
                .await?;
            state
                .task_conversations
                .lock()
                .await
                .remove(&payload.task_id);
            return Ok(false);
        }
    };

    let mut text_buffer = String::new();
    let mut tool_calls = Vec::new();
    let mut pending_stream_chunk = String::new();
    let mut last_stream_emit = std::time::Instant::now();
    let mut provider_usage = ProviderUsageSnapshot::default();
    let idle =
        std::time::Duration::from_secs(state.config_snapshot().inference.token_idle_timeout_secs);

    loop {
        let next = tokio::time::timeout(idle, stream.next()).await;
        let evt = match next {
            Ok(Some(Ok(e))) => e,
            Ok(Some(Err(e))) => {
                cancel_provider_task(state, payload.task_id).await;
                transport
                    .send(ServerMessage::Error {
                        task_id: Some(payload.task_id),
                        kind: ErrorKind::Internal,
                        message: format!("provider stream error: {e}"),
                        matched_pattern: None,
                    })
                    .await?;
                state
                    .task_conversations
                    .lock()
                    .await
                    .remove(&payload.task_id);
                return Ok(false);
            }
            Ok(None) => break,
            Err(_) => {
                cancel_provider_task(state, payload.task_id).await;
                transport
                    .send(ServerMessage::Error {
                        task_id: Some(payload.task_id),
                        kind: ErrorKind::ModelStalled,
                        message: "provider token stream idle timeout".to_string(),
                        matched_pattern: None,
                    })
                    .await?;
                transport
                    .send(ServerMessage::TaskComplete {
                        task_id: payload.task_id,
                        reason: TaskCompleteReason::Timeout,
                        summary: "Provider timed out.".to_string(),
                    })
                    .await?;
                append_session_turn_if_session_mode(
                    state,
                    &payload.mode,
                    payload.shell_id,
                    payload.prompt.clone(),
                    "Provider timed out.".to_string(),
                )
                .await;
                state
                    .task_conversations
                    .lock()
                    .await
                    .remove(&payload.task_id);
                return Ok(true);
            }
        };

        match evt {
            ProviderEvent::TextChunk { content } => {
                text_buffer.push_str(&content);
                if tool_calls.is_empty() && text_buffer.contains("tool_call") {
                    if let Ok(parsed_tagged) = parse_tagged_tool_calls(&text_buffer)
                        && !parsed_tagged.is_empty()
                    {
                        tool_calls = parsed_tagged;
                        cancel_provider_task(state, payload.task_id).await;
                        break;
                    }
                    if let Some(partial_call) =
                        extract_execute_shell_command_from_partial_tagged_call(&text_buffer)
                    {
                        tool_calls.push(partial_call);
                        cancel_provider_task(state, payload.task_id).await;
                        break;
                    }
                }
                if cfg.inference.stream {
                    pending_stream_chunk.push_str(&content);
                    let should_emit = pending_stream_chunk.chars().count() >= 16
                        || pending_stream_chunk.contains('\n')
                        || last_stream_emit.elapsed() >= std::time::Duration::from_millis(25);
                    if !should_emit {
                        continue;
                    }
                    transport
                        .send(ServerMessage::ModelText {
                            task_id: payload.task_id,
                            chunk: pending_stream_chunk.clone(),
                        })
                        .await?;
                    pending_stream_chunk.clear();
                    last_stream_emit = std::time::Instant::now();
                }
            }
            ProviderEvent::ThinkingChunk { .. } => {}
            ProviderEvent::ToolCall { call } => {
                tool_calls.push(call);
            }
            ProviderEvent::Usage { usage } => {
                provider_usage = ProviderUsageSnapshot {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    reported: true,
                };
            }
            ProviderEvent::Done => break,
        }
    }
    *state.last_provider_usage.lock().await = provider_usage;

    if cfg.inference.stream && !pending_stream_chunk.is_empty() {
        transport
            .send(ServerMessage::ModelText {
                task_id: payload.task_id,
                chunk: pending_stream_chunk.clone(),
            })
            .await?;
    }

    if tool_calls.is_empty()
        && let Ok(parsed_tagged) = parse_tagged_tool_calls(&text_buffer)
        && !parsed_tagged.is_empty()
    {
        tool_calls.extend(parsed_tagged);
    }
    if tool_calls.is_empty()
        && let Some(partial_call) =
            extract_execute_shell_command_from_partial_tagged_call(&text_buffer)
    {
        tool_calls.push(partial_call);
    }

    if tool_calls.is_empty()
        && matches!(
            state.provider_caps.structured_mode,
            StructuredOutputMode::StrictJsonFallback
        )
        && let Ok(parsed) = parse_json_tool_call(&text_buffer)
    {
        tool_calls.push(parsed);
    }

    if !text_buffer.trim().is_empty() || !tool_calls.is_empty() {
        let mut conversations = state.task_conversations.lock().await;
        if let Some(history) = conversations.get_mut(&payload.task_id) {
            let mut assistant = ChatMessage::assistant(text_buffer.clone());
            assistant.tool_calls = tool_calls.clone();
            history.push(assistant);
        }
    }

    if tool_calls.is_empty() {
        if cfg.behavior.allow_clarifications && text_buffer.trim_end().ends_with('?') {
            let shell_override = approval_override_for_shell(state, payload.shell_id).await;
            state.tasks.lock().await.insert(
                payload.task_id,
                InFlightTask {
                    task_id: payload.task_id,
                    shell_id: payload.shell_id,
                    mode: payload.mode.clone(),
                    original_prompt: payload.prompt.clone(),
                    proposed_command: String::new(),
                    classification: classification.classification.clone(),
                    classification_confidence: classification.confidence,
                    approval_override: shell_override,
                    awaiting_clarification: true,
                    provider_continuation,
                    tool_round: 0,
                    created_at: std::time::Instant::now(),
                },
            );
            transport
                .send(ServerMessage::NeedsClarification {
                    task_id: payload.task_id,
                    question: text_buffer.trim().trim_end_matches('\n').to_string(),
                })
                .await?;
            return Ok(true);
        }

        if text_buffer.trim().is_empty() {
            state
                .task_conversations
                .lock()
                .await
                .remove(&payload.task_id);
            return Ok(false);
        }
        let mut final_text = text_buffer.clone();
        let mut appended_citations = String::new();
        if profile.web_tools && cfg.web.enabled && cfg.web.expose_tools && cfg.web.citation_required
        {
            let ledger_snapshot = state.last_source_ledger.lock().await.clone();
            let urls = citation_urls_from_ledger(&ledger_snapshot);
            if !urls.is_empty() && !has_citation_block(&final_text) {
                appended_citations = build_citation_block(&urls);
                final_text.push_str(&appended_citations);
            }
        }
        if !cfg.inference.stream {
            transport
                .send(ServerMessage::ModelText {
                    task_id: payload.task_id,
                    chunk: final_text.clone(),
                })
                .await?;
        } else if !appended_citations.is_empty() {
            transport
                .send(ServerMessage::ModelText {
                    task_id: payload.task_id,
                    chunk: appended_citations.clone(),
                })
                .await?;
        }
        if final_text != text_buffer {
            let mut conversations = state.task_conversations.lock().await;
            if let Some(history) = conversations.get_mut(&payload.task_id)
                && let Some(last) = history.last_mut()
                && last.role == "assistant"
            {
                last.content = final_text.clone();
            }
        }
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: truncate_string(&final_text, 500),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            truncate_string(&final_text, 1200),
        )
        .await;
        state
            .task_conversations
            .lock()
            .await
            .remove(&payload.task_id);
        return Ok(true);
    }

    let mut sent_docs = false;
    for call in &tool_calls {
        if call.name.as_str() == "lookup_command_docs" {
            let name = call
                .arguments
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if name.is_empty() {
                continue;
            }
            let lookup = {
                let runtime = state.index_runtime.lock().await;
                lookup_command_docs(
                    &runtime.chunks,
                    &name,
                    call.arguments.get("section").and_then(|v| v.as_str()),
                    state.config_snapshot().indexer.lookup_max_bytes,
                )
            };
            let (tool_response, display_chunk) = match lookup {
                Ok(found) => {
                    let tool = serde_json::json!({
                        "name": found.name,
                        "section": found.section,
                        "text": found.text,
                        "truncated": found.truncated
                    });
                    let serialized =
                        serde_json::to_string(&tool).unwrap_or_else(|_| "{}".to_string());
                    let display = format!(
                        "## {}\n{}",
                        found.name,
                        truncate_string(
                            tool.get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default(),
                            4000
                        )
                    );
                    (
                        context::trim_to_tokens(
                            &serialized,
                            cfg.context_budget.docs_rag_tokens.max(256),
                        ),
                        context::trim_to_tokens(
                            &display,
                            cfg.context_budget.docs_rag_tokens.max(256),
                        ),
                    )
                }
                Err(s) => {
                    let tool = serde_json::json!({
                        "error": "unknown_command",
                        "command": name,
                        "suggestions": s
                    });
                    let serialized =
                        serde_json::to_string(&tool).unwrap_or_else(|_| "{}".to_string());
                    (
                        context::trim_to_tokens(
                            &serialized,
                            cfg.context_budget.docs_rag_tokens.max(256),
                        ),
                        context::trim_to_tokens(
                            &format!(
                                "unknown command: {}; suggestions: {}",
                                name,
                                tool.get("suggestions")
                                    .and_then(|v| v.as_array())
                                    .map(|arr| arr
                                        .iter()
                                        .filter_map(|v| v.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", "))
                                    .unwrap_or_default()
                            ),
                            cfg.context_budget.docs_rag_tokens.max(256),
                        ),
                    )
                }
            };
            {
                let mut conversations = state.task_conversations.lock().await;
                if let Some(history) = conversations.get_mut(&payload.task_id) {
                    history.push(ChatMessage::tool("lookup_command_docs", tool_response));
                }
            }
            transport
                .send(ServerMessage::ModelText {
                    task_id: payload.task_id,
                    chunk: display_chunk,
                })
                .await?;
            sent_docs = true;
        }
    }

    let mut sent_readonly = false;
    for call in &tool_calls {
        if let Some((tool_name, tool_output)) =
            handle_readonly_tool_call(state, payload, session, call).await?
        {
            let budgeted_output = context::trim_to_tokens(
                &tool_output,
                tool_output_budget_tokens(cfg.as_ref(), &tool_name).max(256),
            );
            {
                let mut conversations = state.task_conversations.lock().await;
                if let Some(history) = conversations.get_mut(&payload.task_id) {
                    history.push(ChatMessage::tool(tool_name, budgeted_output.clone()));
                }
            }
            transport
                .send(ServerMessage::ModelText {
                    task_id: payload.task_id,
                    chunk: truncate_string(&budgeted_output, 6000),
                })
                .await?;
            sent_readonly = true;
        }
    }

    for call in tool_calls {
        if call.name.as_str() != "execute_shell_command" {
            continue;
        }
        let cmd = call
            .arguments
            .get("cmd")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if cmd.is_empty() {
            continue;
        }
        let commands_used = call
            .arguments
            .get("commands_used")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        propose_command_for_execution(
            state,
            transport,
            payload,
            session,
            provider_continuation,
            classification,
            CommandPlan {
                cmd,
                rationale: "provider_tool_call".to_string(),
                intent: "provider generated command".to_string(),
                expected_effect: "execute validated shell command".to_string(),
                commands_used,
            },
        )
        .await?;
        return Ok(true);
    }

    if sent_docs || sent_readonly {
        transport
            .send(ServerMessage::TaskComplete {
                task_id: payload.task_id,
                reason: TaskCompleteReason::ModelDone,
                summary: "read-only tool calls completed.".to_string(),
            })
            .await?;
        append_session_turn_if_session_mode(
            state,
            &payload.mode,
            payload.shell_id,
            payload.prompt.clone(),
            "Read-only tools completed.".to_string(),
        )
        .await;
        state
            .task_conversations
            .lock()
            .await
            .remove(&payload.task_id);
        return Ok(true);
    }

    state
        .task_conversations
        .lock()
        .await
        .remove(&payload.task_id);
    Ok(false)
}

async fn maybe_run_runtime_stub_provider(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: &termlm_protocol::StartTask,
    session: &ShellSession,
    provider_continuation: bool,
    classification: &tasks::ClassificationResult,
    drafting_prompt: &str,
) -> Result<bool> {
    let enabled = runtime_stub_provider_enabled();
    if !enabled {
        return Ok(false);
    }

    let Some(draft) = tasks::draft_command_for_prompt(drafting_prompt) else {
        return Ok(false);
    };

    propose_command_for_execution(
        state,
        transport,
        payload,
        session,
        provider_continuation,
        classification,
        CommandPlan {
            cmd: draft.cmd,
            rationale: draft.rationale,
            intent: draft.intent,
            expected_effect: draft.expected_effect,
            commands_used: draft.commands_used,
        },
    )
    .await?;
    Ok(true)
}

async fn maybe_run_heuristic_command_fallback(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: &termlm_protocol::StartTask,
    session: &ShellSession,
    classification: &tasks::ClassificationResult,
) -> Result<bool> {
    let Some(draft) = tasks::draft_command_for_prompt(&payload.prompt) else {
        return Ok(false);
    };

    propose_command_for_execution(
        state,
        transport,
        payload,
        session,
        true,
        classification,
        CommandPlan {
            cmd: draft.cmd,
            rationale: draft.rationale,
            intent: draft.intent,
            expected_effect: draft.expected_effect,
            commands_used: draft.commands_used,
        },
    )
    .await?;
    Ok(true)
}

fn runtime_stub_provider_enabled() -> bool {
    cfg!(feature = "runtime-stub")
}

fn tool_schema_execute_shell_command() -> ToolSchema {
    ToolSchema {
        name: "execute_shell_command".to_string(),
        description: "Propose exactly one shell command for the user to run.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "cmd": {"type":"string"},
                "intent": {
                    "type":"string",
                    "description":"What the command is meant to accomplish."
                },
                "expected_effect": {
                    "type":"string",
                    "description":"Expected filesystem/process/network effect in plain language."
                },
                "commands_used": {
                    "type":"array",
                    "items": {"type":"string"}
                }
            },
            "required": ["cmd"]
        }),
    }
}

fn tool_schema_lookup_docs() -> ToolSchema {
    ToolSchema {
        name: "lookup_command_docs".to_string(),
        description: "Look up local docs for an installed command.".to_string(),
        parameters: serde_json::json!({
            "type":"object",
            "properties": {
                "name": {"type":"string"},
                "section": {"type":"string"}
            },
            "required": ["name"]
        }),
    }
}

fn local_tool_schemas(cfg: &AppConfig) -> Vec<ToolSchema> {
    let mut out = vec![
        ToolSchema {
            name: "read_file".to_string(),
            description: "Read a plaintext local file without executing shell commands."
                .to_string(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "path": {"type":"string"},
                    "start_line": {"type":"integer"},
                    "max_lines": {"type":"integer"},
                    "max_bytes": {"type":"integer"}
                },
                "required": ["path"]
            }),
        },
        ToolSchema {
            name: "search_files".to_string(),
            description: "Search text in workspace files.".to_string(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "query": {"type":"string"},
                    "root": {"type":"string"},
                    "glob": {"type":"string"},
                    "regex": {"type":"boolean"},
                    "max_results": {"type":"integer"},
                    "max_bytes_per_file": {"type":"integer"},
                    "include_hidden": {"type":"boolean"},
                    "respect_gitignore": {"type":"boolean"}
                },
                "required": ["query"]
            }),
        },
        ToolSchema {
            name: "list_workspace_files".to_string(),
            description: "List workspace files and directories.".to_string(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "root": {"type":"string"},
                    "max_entries": {"type":"integer"},
                    "max_depth": {"type":"integer"},
                    "include_hidden": {"type":"boolean"}
                }
            }),
        },
        ToolSchema {
            name: "search_terminal_context".to_string(),
            description: "Search previously observed terminal commands and output.".to_string(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "query": {"type":"string"},
                    "max_results": {"type":"integer"}
                },
                "required": ["query"]
            }),
        },
    ];

    if cfg.project_metadata.enabled {
        out.push(ToolSchema {
            name: "project_metadata".to_string(),
            description: "Detect project metadata from workspace files.".to_string(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "root": {"type":"string"},
                    "include_scripts": {"type":"boolean"},
                    "include_ci": {"type":"boolean"},
                    "max_files_read": {"type":"integer"},
                    "max_bytes_per_file": {"type":"integer"}
                }
            }),
        });
    }

    if cfg.git_context.enabled {
        out.push(ToolSchema {
            name: "git_context".to_string(),
            description: "Inspect git repository context without modifying state.".to_string(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "root": {"type":"string"},
                    "include_diff_summary": {"type":"boolean"},
                    "max_files": {"type":"integer"},
                    "max_recent_commits": {"type":"integer"},
                    "max_diff_bytes": {"type":"integer"}
                }
            }),
        });
    }

    out
}

fn local_tool_schemas_for_profile(
    cfg: &AppConfig,
    profile: &context::ToolExposureProfile,
) -> Vec<ToolSchema> {
    if !cfg.local_tools.enabled {
        return Vec::new();
    }
    local_tool_schemas(cfg)
        .into_iter()
        .filter(|tool| {
            if tool.name == "search_terminal_context" {
                profile.terminal_context_tool
            } else {
                profile.local_file_tools
            }
        })
        .collect()
}

fn web_tool_schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "web_search".to_string(),
            description: "Search current public web sources for recent information.".to_string(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "query": {"type":"string"},
                    "freshness": {"type":"string"},
                    "max_results": {"type":"integer"}
                },
                "required": ["query"]
            }),
        },
        ToolSchema {
            name: "web_read".to_string(),
            description: "Fetch and extract readable markdown from a URL.".to_string(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "url": {"type":"string"},
                    "max_bytes": {"type":"integer"}
                },
                "required": ["url"]
            }),
        },
    ]
}

async fn propose_command_for_execution(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: &termlm_protocol::StartTask,
    session: &ShellSession,
    provider_continuation: bool,
    classification: &tasks::ClassificationResult,
    plan: CommandPlan,
) -> Result<()> {
    let mut effective_plan = plan;
    let mut working_cmd = effective_plan.cmd.trim().to_string();
    let max_rounds = state.config_snapshot().behavior.max_planning_rounds.max(1);
    let mut planning_round = 1u32;
    let mut last_findings = Vec::new();
    let mut validation_status = "passed".to_string();
    let critical_matcher =
        CriticalMatcher::from_patterns(&state.config_snapshot().approval.critical_patterns);
    let shell_override = approval_override_for_shell(state, payload.shell_id).await;
    let (prior_override, prior_provider_continuation, prior_tool_round) = {
        let tasks = state.tasks.lock().await;
        if let Some(t) = tasks.get(&payload.task_id) {
            (t.approval_override, t.provider_continuation, t.tool_round)
        } else {
            (shell_override, false, 0)
        }
    };
    let task_provider_continuation = provider_continuation || prior_provider_continuation;

    let (first, grounding_refs, final_parse) = loop {
        if let Some(matched) = matches_safety_floor(&working_cmd) {
            let refusal = format!(
                "Refused: command matched the immutable safety floor pattern `{}`. Propose a safer alternative.",
                matched.pattern
            );
            transport
                .send(ServerMessage::Error {
                    task_id: Some(payload.task_id),
                    kind: ErrorKind::SafetyFloor,
                    message: "Refused: command matched immutable safety floor".to_string(),
                    matched_pattern: Some(matched.pattern.to_string()),
                })
                .await?;

            {
                let mut conversations = state.task_conversations.lock().await;
                let history = conversations
                    .entry(payload.task_id)
                    .or_insert_with(Vec::new);
                history.push(ChatMessage::tool("execute_shell_command", refusal));
            }

            if state.config_snapshot().behavior.allow_clarifications {
                state.tasks.lock().await.insert(
                    payload.task_id,
                    InFlightTask {
                        task_id: payload.task_id,
                        shell_id: payload.shell_id,
                        mode: payload.mode.clone(),
                        original_prompt: payload.prompt.clone(),
                        proposed_command: String::new(),
                        classification: classification.classification.clone(),
                        classification_confidence: classification.confidence,
                        approval_override: prior_override,
                        awaiting_clarification: true,
                        provider_continuation: true,
                        tool_round: prior_tool_round,
                        created_at: std::time::Instant::now(),
                    },
                );
                transport
                    .send(ServerMessage::NeedsClarification {
                        task_id: payload.task_id,
                        question:
                            "That command is blocked by the immutable safety floor. What safer outcome should I achieve?"
                                .to_string(),
                    })
                    .await?;
                return Ok(());
            }

            transport
                .send(ServerMessage::TaskComplete {
                    task_id: payload.task_id,
                    reason: TaskCompleteReason::SafetyFloor,
                    summary: "Command blocked by immutable safety floor.".to_string(),
                })
                .await?;
            append_session_turn_if_session_mode(
                state,
                &payload.mode,
                payload.shell_id,
                payload.prompt.clone(),
                "Command blocked by immutable safety floor.".to_string(),
            )
            .await;
            clear_task_state(state, payload.task_id).await;
            return Ok(());
        }

        let parsed = parse_command(&working_cmd);
        let first = parsed.first_token.clone().unwrap_or_default();
        let exists = !first.is_empty() && cached_command_exists(state, session, &first).await;
        let suggestions = if exists {
            Vec::new()
        } else {
            suggest_known_commands(state, &first).await
        };
        let docs_excerpt = if first.is_empty() {
            String::new()
        } else {
            lookup_docs_excerpt(state, &first).await
        };

        let grounding_refs =
            build_grounding_refs(state, &payload.prompt, &first, &working_cmd).await;
        let grounded = planning::GroundedProposal {
            command: working_cmd.clone(),
            intent: effective_plan.intent.clone(),
            expected_effect: effective_plan.expected_effect.clone(),
            commands_used: vec![first.clone()],
            risk_level: if working_cmd.contains("rm -") {
                "elevated".to_string()
            } else {
                "read_only".to_string()
            },
            destructive: working_cmd.contains(" rm ") || working_cmd.starts_with("rm "),
            requires_approval: true,
            grounding: grounding_refs
                .iter()
                .map(|g| format!("{}#{}", g.source, g.command))
                .collect(),
            validation: Vec::new(),
        };
        let mut findings = planning::validate_round(
            &grounded,
            &planning::ValidationContext {
                prompt: payload.prompt.clone(),
                command_exists: exists,
                docs_excerpt,
                validate_command_flags: state.config_snapshot().indexer.validate_command_flags,
                parse_ambiguous: parsed.ambiguous,
                parse_warnings: parsed.warnings.clone(),
                parse_risky_constructs: parsed.has_risky_constructs(),
            },
        );
        if effective_plan.rationale != "provider_tool_call" {
            findings.retain(|f| f.kind != "unsupported_flag");
        }
        if critical_matcher.is_critical(&working_cmd) {
            findings.retain(|f| f.kind != "unsupported_flag");
        }
        if findings.is_empty() {
            break (first, grounding_refs, parsed);
        }
        last_findings = findings.clone();
        let tool_feedback = validation_findings_tool_response(
            &working_cmd,
            &first,
            &findings,
            &suggestions,
            planning_round,
            max_rounds,
        );
        {
            let mut conversations = state.task_conversations.lock().await;
            let history = conversations
                .entry(payload.task_id)
                .or_insert_with(Vec::new);
            history.push(ChatMessage::tool("execute_shell_command", tool_feedback));
        }
        if findings.iter().any(|f| f.kind == "unknown_command") {
            let msg = if first.is_empty() {
                "validation blocked: unknown command token".to_string()
            } else if suggestions.is_empty() {
                format!("validation blocked: unknown command `{first}`")
            } else {
                format!(
                    "validation blocked: unknown command `{first}`; suggestions: {}",
                    suggestions
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            transport
                .send(ServerMessage::Error {
                    task_id: Some(payload.task_id),
                    kind: ErrorKind::UnknownCommand,
                    message: msg,
                    matched_pattern: None,
                })
                .await?;
        }

        if let Some(redraft_cmd) =
            provider_redraft_from_validation(state, payload, classification).await?
            && redraft_cmd.trim() != working_cmd.trim()
        {
            working_cmd = redraft_cmd.trim().to_string();
            planning_round += 1;
            continue;
        }

        if planning_round >= max_rounds {
            let message = summarize_validation_findings(&findings);
            if state.config_snapshot().behavior.allow_clarifications {
                state.tasks.lock().await.insert(
                    payload.task_id,
                    InFlightTask {
                        task_id: payload.task_id,
                        shell_id: payload.shell_id,
                        mode: payload.mode.clone(),
                        original_prompt: payload.prompt.clone(),
                        proposed_command: String::new(),
                        classification: classification.classification.clone(),
                        classification_confidence: classification.confidence,
                        approval_override: prior_override,
                        awaiting_clarification: true,
                        provider_continuation: task_provider_continuation,
                        tool_round: prior_tool_round,
                        created_at: std::time::Instant::now(),
                    },
                );
                transport
                    .send(ServerMessage::NeedsClarification {
                        task_id: payload.task_id,
                        question: format!(
                            "validation_incomplete: I could not validate a safe command within {max_rounds} planning rounds ({message}). What exact behavior should I run?"
                        ),
                    })
                    .await?;
                return Ok(());
            }

            let fallback = tasks::validation_incomplete_fallback(&payload.prompt);
            effective_plan = CommandPlan {
                cmd: fallback.cmd.trim().to_string(),
                rationale: fallback.rationale,
                intent: fallback.intent,
                expected_effect: fallback.expected_effect,
                commands_used: fallback.commands_used,
            };
            working_cmd = effective_plan.cmd.clone();

            let fallback_parsed = parse_command(&working_cmd);
            let fallback_first = fallback_parsed.first_token.clone().unwrap_or_default();
            let fallback_exists = !fallback_first.is_empty()
                && cached_command_exists(state, session, &fallback_first).await;
            let fallback_docs = if fallback_first.is_empty() {
                String::new()
            } else {
                lookup_docs_excerpt(state, &fallback_first).await
            };
            let fallback_grounding =
                build_grounding_refs(state, &payload.prompt, &fallback_first, &working_cmd).await;
            let fallback_grounded = planning::GroundedProposal {
                command: working_cmd.clone(),
                intent: effective_plan.intent.clone(),
                expected_effect: effective_plan.expected_effect.clone(),
                commands_used: vec![fallback_first.clone()],
                risk_level: "read_only".to_string(),
                destructive: false,
                requires_approval: true,
                grounding: fallback_grounding
                    .iter()
                    .map(|g| format!("{}#{}", g.source, g.command))
                    .collect(),
                validation: Vec::new(),
            };
            let mut fallback_findings = planning::validate_round(
                &fallback_grounded,
                &planning::ValidationContext {
                    prompt: payload.prompt.clone(),
                    command_exists: fallback_exists,
                    docs_excerpt: fallback_docs,
                    validate_command_flags: state.config_snapshot().indexer.validate_command_flags,
                    parse_ambiguous: fallback_parsed.ambiguous,
                    parse_warnings: fallback_parsed.warnings.clone(),
                    parse_risky_constructs: fallback_parsed.has_risky_constructs(),
                },
            );
            if effective_plan.rationale != "provider_tool_call" {
                fallback_findings.retain(|f| f.kind != "unsupported_flag");
            }

            if fallback_findings.is_empty() {
                validation_status = "validation_incomplete".to_string();
                last_findings.push(planning::ValidationFinding {
                    kind: "validation_incomplete".to_string(),
                    detail: format!("max planning rounds ({max_rounds}) exhausted: {message}"),
                });
                break (fallback_first, fallback_grounding, fallback_parsed);
            }

            let fallback_message = summarize_validation_findings(&fallback_findings);
            transport
                .send(ServerMessage::Error {
                    task_id: Some(payload.task_id),
                    kind: ErrorKind::BadToolCall,
                    message: format!(
                        "planning validation failed: {message}; validation_incomplete fallback failed: {fallback_message}"
                    ),
                    matched_pattern: None,
                })
                .await?;
            transport
                .send(ServerMessage::TaskComplete {
                    task_id: payload.task_id,
                    reason: TaskCompleteReason::ModelDone,
                    summary: format!(
                        "validation_incomplete: no validated command after {max_rounds} planning rounds."
                    ),
                })
                .await?;
            append_session_turn_if_session_mode(
                state,
                &payload.mode,
                payload.shell_id,
                payload.prompt.clone(),
                format!(
                    "validation_incomplete: no validated command after {max_rounds} planning rounds ({message})"
                ),
            )
            .await;
            return Ok(());
        }

        let revised = planning::revise_command(
            &working_cmd,
            &payload.prompt,
            &findings,
            suggestions.first().map(String::as_str),
        );
        let Some(next_cmd) = revised else {
            let message = summarize_validation_findings(&findings);
            if state.config_snapshot().behavior.allow_clarifications {
                state.tasks.lock().await.insert(
                    payload.task_id,
                    InFlightTask {
                        task_id: payload.task_id,
                        shell_id: payload.shell_id,
                        mode: payload.mode.clone(),
                        original_prompt: payload.prompt.clone(),
                        proposed_command: String::new(),
                        classification: classification.classification.clone(),
                        classification_confidence: classification.confidence,
                        approval_override: prior_override,
                        awaiting_clarification: true,
                        provider_continuation: task_provider_continuation,
                        tool_round: prior_tool_round,
                        created_at: std::time::Instant::now(),
                    },
                );
                transport
                    .send(ServerMessage::NeedsClarification {
                        task_id: payload.task_id,
                        question: format!(
                            "I couldn't validate a safe command ({message}). What exact behavior do you want?"
                        ),
                    })
                    .await?;
                return Ok(());
            }
            transport
                .send(ServerMessage::Error {
                    task_id: Some(payload.task_id),
                    kind: ErrorKind::BadToolCall,
                    message: format!("planning validation failed: {message}"),
                    matched_pattern: None,
                })
                .await?;
            transport
                .send(ServerMessage::TaskComplete {
                    task_id: payload.task_id,
                    reason: TaskCompleteReason::ModelDone,
                    summary: "Planning validation failed and no safe revision was found."
                        .to_string(),
                })
                .await?;
            append_session_turn_if_session_mode(
                state,
                &payload.mode,
                payload.shell_id,
                payload.prompt.clone(),
                format!("Planning validation failed and no safe revision was found: {message}"),
            )
            .await;
            return Ok(());
        };
        if next_cmd == working_cmd {
            planning_round += 1;
            continue;
        }
        working_cmd = next_cmd;
        planning_round += 1;
    };

    let matched_critical_pattern = critical_matcher.is_critical(&working_cmd);
    let parser_critical = final_parse.ambiguous && final_parse.has_risky_constructs();
    let critical = matched_critical_pattern || parser_critical;
    let commands_used = if !effective_plan.commands_used.is_empty() {
        effective_plan.commands_used.clone()
    } else if first.is_empty() {
        Vec::new()
    } else {
        vec![first.clone()]
    };
    let mut requires_approval = match state.config_snapshot().approval.mode.as_str() {
        "auto" => false,
        "manual_critical" => critical,
        _ => true,
    };
    if prior_override && !critical {
        requires_approval = false;
    }
    let proposal = ProposedCommand {
        task_id: payload.task_id,
        cmd: working_cmd.clone(),
        rationale: effective_plan.rationale,
        intent: effective_plan.intent,
        expected_effect: effective_plan.expected_effect,
        commands_used,
        risk_level: if critical {
            "critical".to_string()
        } else if final_parse.has_risky_constructs() {
            "elevated".to_string()
        } else {
            "read_only".to_string()
        },
        requires_approval,
        critical_match: if critical {
            Some(if parser_critical && !matched_critical_pattern {
                "parser_ambiguity_risk".to_string()
            } else {
                "critical_pattern".to_string()
            })
        } else {
            None
        },
        grounding: grounding_refs,
        validation: termlm_protocol::ValidationSummary {
            status: if validation_status == "validation_incomplete" {
                validation_status
            } else if last_findings.is_empty() {
                "passed".to_string()
            } else {
                "revised".to_string()
            },
            planning_rounds: planning_round,
        },
        round: planning_round,
    };
    let log_cmd = command_for_log(state.config_snapshot().as_ref(), &proposal.cmd, critical);
    info!(
        task_id = %payload.task_id,
        shell_id = %payload.shell_id,
        cmd = %log_cmd,
        risk_level = %proposal.risk_level,
        requires_approval = proposal.requires_approval,
        "proposal prepared"
    );

    let mut proposal_refs = Vec::<source_ledger::SourceRef>::new();
    for g in &proposal.grounding {
        proposal_refs.push(source_ledger::SourceRef {
            source_type: "docs_chunk".to_string(),
            source_id: format!("{}::{}", g.source, g.command),
            hash: g.doc_hash.clone().unwrap_or_default(),
            redacted: false,
            truncated: false,
            observed_at: chrono::Utc::now(),
            detail: Some(format!("grounding for {}", proposal.cmd)),
            section: g.sections.first().cloned(),
            offset_start: None,
            offset_end: None,
            extraction_method: g.extraction_method.clone(),
            extracted_at: g.extracted_at,
            index_version: g.index_version,
        });
    }
    if !last_findings.is_empty() {
        for finding in &last_findings {
            proposal_refs.push(source_ledger::SourceRef {
                source_type: "validation_finding".to_string(),
                source_id: finding.kind.clone(),
                hash: format!("{:x}", sha2::Sha256::digest(finding.detail.as_bytes())),
                redacted: false,
                truncated: false,
                observed_at: chrono::Utc::now(),
                detail: Some(finding.detail.clone()),
                section: None,
                offset_start: None,
                offset_end: None,
                extraction_method: None,
                extracted_at: None,
                index_version: None,
            });
        }
    }
    append_source_refs(state, proposal_refs).await;

    transport
        .send(ServerMessage::ProposedCommand { payload: proposal })
        .await?;
    state.tasks.lock().await.insert(
        payload.task_id,
        InFlightTask {
            task_id: payload.task_id,
            shell_id: payload.shell_id,
            mode: payload.mode.clone(),
            original_prompt: payload.prompt.clone(),
            proposed_command: working_cmd,
            classification: classification.classification.clone(),
            classification_confidence: classification.confidence,
            approval_override: prior_override,
            awaiting_clarification: false,
            provider_continuation: task_provider_continuation,
            tool_round: prior_tool_round,
            created_at: std::time::Instant::now(),
        },
    );
    Ok(())
}

async fn provider_redraft_from_validation(
    state: &Arc<DaemonState>,
    payload: &termlm_protocol::StartTask,
    classification: &tasks::ClassificationResult,
) -> Result<Option<String>> {
    let cfg = state.config_snapshot();
    let profile = context::determine_tool_exposure(&classification.classification, cfg.as_ref());

    let mut tools = Vec::<ToolSchema>::new();
    if profile.execute_shell_command {
        tools.push(tool_schema_execute_shell_command());
    } else {
        // Planning redraft requires command emission through the same execute tool contract.
        tools.push(tool_schema_execute_shell_command());
    }
    if profile.lookup_command_docs {
        tools.push(tool_schema_lookup_docs());
    }
    let local_tools = local_tool_schemas_for_profile(cfg.as_ref(), &profile);
    if !local_tools.is_empty() {
        tools.extend(local_tools);
    }
    if profile.web_tools && cfg.web.enabled && cfg.web.expose_tools {
        tools.extend(web_tool_schemas());
    }

    let redraft_prompt = "Validation feedback was provided via tool results. Retry by emitting one corrected execute_shell_command tool call only.";
    let messages = {
        let mut conversations = state.task_conversations.lock().await;
        let history = conversations
            .entry(payload.task_id)
            .or_insert_with(Vec::new);
        history.push(ChatMessage::user(redraft_prompt.to_string()));
        history.clone()
    };

    let request = ChatRequest {
        task_id: Some(payload.task_id.to_string()),
        model: active_model_name(cfg.as_ref()),
        messages,
        tools,
        stream: false,
        think: cfg.behavior.thinking,
        options: provider_request_options(cfg.as_ref()),
    };

    let mut stream = {
        let provider = state.provider.lock().await;
        match provider.chat_stream(request).await {
            Ok(s) => s,
            Err(e) => {
                warn!("validation redraft provider request failed: {e:#}");
                return Ok(None);
            }
        }
    };

    let idle =
        std::time::Duration::from_secs(state.config_snapshot().inference.token_idle_timeout_secs);
    let mut text_buffer = String::new();
    let mut tool_calls = Vec::<termlm_inference::ToolCall>::new();
    loop {
        let next = tokio::time::timeout(idle, stream.next()).await;
        let evt = match next {
            Ok(Some(Ok(e))) => e,
            Ok(Some(Err(e))) => {
                warn!("validation redraft stream error: {e:#}");
                return Ok(None);
            }
            Ok(None) => break,
            Err(_) => {
                warn!("validation redraft stream idle timeout");
                return Ok(None);
            }
        };

        match evt {
            ProviderEvent::TextChunk { content } => {
                text_buffer.push_str(&content);
                if tool_calls.is_empty() && text_buffer.contains("tool_call") {
                    if let Ok(parsed_tagged) = parse_tagged_tool_calls(&text_buffer)
                        && !parsed_tagged.is_empty()
                    {
                        tool_calls = parsed_tagged;
                        cancel_provider_task(state, payload.task_id).await;
                        break;
                    }
                    if let Some(partial_call) =
                        extract_execute_shell_command_from_partial_tagged_call(&text_buffer)
                    {
                        tool_calls.push(partial_call);
                        cancel_provider_task(state, payload.task_id).await;
                        break;
                    }
                }
            }
            ProviderEvent::ThinkingChunk { .. } => {}
            ProviderEvent::ToolCall { call } => tool_calls.push(call),
            ProviderEvent::Usage { .. } => {}
            ProviderEvent::Done => break,
        }
    }

    if tool_calls.is_empty()
        && let Ok(parsed_tagged) = parse_tagged_tool_calls(&text_buffer)
        && !parsed_tagged.is_empty()
    {
        tool_calls.extend(parsed_tagged);
    }
    if tool_calls.is_empty()
        && let Some(partial_call) =
            extract_execute_shell_command_from_partial_tagged_call(&text_buffer)
    {
        tool_calls.push(partial_call);
    }

    if tool_calls.is_empty()
        && matches!(
            state.provider_caps.structured_mode,
            StructuredOutputMode::StrictJsonFallback
        )
        && let Ok(parsed) = parse_json_tool_call(&text_buffer)
    {
        tool_calls.push(parsed);
    }

    if !text_buffer.trim().is_empty() || !tool_calls.is_empty() {
        let mut conversations = state.task_conversations.lock().await;
        if let Some(history) = conversations.get_mut(&payload.task_id) {
            let mut assistant = ChatMessage::assistant(text_buffer);
            assistant.tool_calls = tool_calls.clone();
            history.push(assistant);
        }
    }

    for call in tool_calls {
        if call.name != "execute_shell_command" {
            continue;
        }
        let cmd = call
            .arguments
            .get("cmd")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if !cmd.is_empty() {
            return Ok(Some(cmd));
        }
    }

    Ok(None)
}

fn summarize_validation_findings(findings: &[planning::ValidationFinding]) -> String {
    findings
        .iter()
        .map(|f| format!("{}: {}", f.kind, f.detail))
        .collect::<Vec<_>>()
        .join("; ")
}

fn validation_findings_tool_response(
    command: &str,
    first_token: &str,
    findings: &[planning::ValidationFinding],
    suggestions: &[String],
    planning_round: u32,
    max_rounds: u32,
) -> String {
    let payload = if findings.iter().any(|f| f.kind == "unknown_command") {
        serde_json::json!({
            "error": "unknown_command",
            "command": first_token,
            "suggestions": suggestions.iter().take(5).cloned().collect::<Vec<_>>(),
            "findings": findings,
            "draft": command,
            "planning_round": planning_round,
            "max_planning_rounds": max_rounds
        })
    } else if findings.iter().any(|f| f.kind == "unsupported_flag") {
        serde_json::json!({
            "error": "unsupported_flag",
            "command": first_token,
            "findings": findings,
            "draft": command,
            "planning_round": planning_round,
            "max_planning_rounds": max_rounds
        })
    } else if findings.iter().any(|f| f.kind == "insufficient_draft") {
        serde_json::json!({
            "error": "insufficient_draft",
            "findings": findings,
            "draft": command,
            "planning_round": planning_round,
            "max_planning_rounds": max_rounds
        })
    } else {
        serde_json::json!({
            "error": "validation_failed",
            "findings": findings,
            "draft": command,
            "planning_round": planning_round,
            "max_planning_rounds": max_rounds
        })
    };
    payload.to_string()
}

async fn build_grounding_refs(
    state: &Arc<DaemonState>,
    prompt: &str,
    command_name: &str,
    draft_command: &str,
) -> Vec<termlm_protocol::GroundingRef> {
    let cfg = state.config_snapshot();
    let query_text = if cfg.indexer.command_aware_retrieval {
        build_command_aware_retrieval_query(prompt, command_name, draft_command)
    } else {
        prompt.to_string()
    };
    let retrieval_top_k = if cfg.indexer.command_aware_retrieval {
        cfg.indexer.command_aware_top_k.max(1)
    } else {
        cfg.indexer.rag_top_k.max(1)
    };
    let (use_external_embeddings, index_revision) = {
        let runtime = state.index_runtime.lock().await;
        (runtime.uses_external_embeddings, runtime.revision)
    };
    let cache_enabled = cfg.cache.enabled && cfg.indexer.cache_retrieval_results;
    let cache_semantics = DocsRetrievalCacheSemantics {
        rag_top_k: retrieval_top_k,
        rag_min_similarity: cfg.indexer.rag_min_similarity,
        hybrid_retrieval_enabled: cfg.indexer.hybrid_retrieval_enabled,
        lexical_index_enabled: cfg.indexer.lexical_index_enabled,
        lexical_top_k: cfg.indexer.lexical_top_k,
        command_aware_retrieval: cfg.indexer.command_aware_retrieval,
        command_aware_top_k: cfg.indexer.command_aware_top_k,
        index_revision,
    };
    let retrieval_cache_key = docs_retrieval_cache_key(&query_text, command_name, cache_semantics);
    if cache_enabled
        && let Some(cached) = state.retrieval_cache.lock().await.get(&retrieval_cache_key)
    {
        return cached;
    }
    let query_embedding = if use_external_embeddings {
        embed_query_vector(state, cfg.as_ref(), &query_text).await
    } else {
        None
    };
    let runtime = state.index_runtime.lock().await;
    if runtime.chunks.is_empty() {
        return vec![termlm_protocol::GroundingRef {
            command: command_name.to_string(),
            source: "heuristic_planner".to_string(),
            sections: Vec::new(),
            extraction_method: Some("heuristic".to_string()),
            doc_hash: None,
            extracted_at: None,
            index_version: Some(current_index_version()),
        }];
    }

    let mut query =
        RetrievalQuery::new(query_text, retrieval_top_k, cfg.indexer.rag_min_similarity);
    query.hybrid_enabled = cfg.indexer.hybrid_retrieval_enabled;
    query.lexical_enabled = cfg.indexer.lexical_index_enabled;
    query.lexical_top_k = cfg.indexer.lexical_top_k;
    query.exact_command_boost = cfg.indexer.exact_command_boost;
    query.exact_flag_boost = cfg.indexer.exact_flag_boost;
    query.section_boost_options = cfg.indexer.section_boost_options;
    query.command_aware = cfg.indexer.command_aware_retrieval;
    let refs = runtime
        .retriever
        .search_with_embedding(
            &query,
            if use_external_embeddings {
                query_embedding.as_deref()
            } else {
                None
            },
        )
        .into_iter()
        .take(query.top_k)
        .map(|hit| termlm_protocol::GroundingRef {
            command: hit.chunk.command_name,
            source: hit.chunk.path,
            sections: vec![hit.chunk.section_name],
            extraction_method: Some(hit.chunk.extraction_method),
            doc_hash: Some(hit.chunk.doc_hash),
            extracted_at: Some(hit.chunk.extracted_at),
            index_version: Some(current_index_version()),
        })
        .collect::<Vec<_>>();
    if cache_enabled {
        state
            .retrieval_cache
            .lock()
            .await
            .insert(retrieval_cache_key, refs.clone());
    }
    refs
}

#[derive(Debug, Default)]
struct CommandAwareRetrievalFeatures {
    flags: Vec<String>,
    subcommands: Vec<String>,
    paths: Vec<String>,
    risk_markers: Vec<String>,
}

fn build_command_aware_retrieval_query(
    prompt: &str,
    command_name: &str,
    draft_command: &str,
) -> String {
    let features = extract_command_aware_features(command_name, draft_command);
    let mut out = String::new();
    out.push_str(prompt);
    out.push('\n');
    out.push_str("command: ");
    out.push_str(command_name.trim());
    out.push('\n');
    out.push_str("draft_command: ");
    out.push_str(draft_command.trim());

    if !features.flags.is_empty() {
        out.push('\n');
        out.push_str("flags: ");
        out.push_str(&features.flags.join(" "));
    }
    if !features.subcommands.is_empty() {
        out.push('\n');
        out.push_str("subcommands: ");
        out.push_str(&features.subcommands.join(" "));
    }
    if !features.paths.is_empty() {
        out.push('\n');
        out.push_str("paths: ");
        out.push_str(&features.paths.join(" "));
    }
    if !features.risk_markers.is_empty() {
        out.push('\n');
        out.push_str("risk_markers: ");
        out.push_str(&features.risk_markers.join(" "));
    }
    out
}

fn extract_command_aware_features(
    command_name: &str,
    draft_command: &str,
) -> CommandAwareRetrievalFeatures {
    let parsed = parse_command(draft_command);
    let significant = parsed
        .first_token
        .as_deref()
        .unwrap_or(command_name)
        .to_string();
    let has_pipeline = parsed.has_pipeline;
    let has_control_operators = parsed.has_control_operators;
    let has_redirection = parsed.has_redirection;
    let has_command_substitution = parsed.has_command_substitution;
    let tokens = parsed.tokens;
    let mut flags = std::collections::BTreeSet::<String>::new();
    let mut subcommands = std::collections::BTreeSet::<String>::new();
    let mut paths = std::collections::BTreeSet::<String>::new();
    let mut risk_markers = std::collections::BTreeSet::<String>::new();

    if has_pipeline {
        risk_markers.insert("pipeline".to_string());
    }
    if has_control_operators {
        risk_markers.insert("control_operators".to_string());
    }
    if has_redirection {
        risk_markers.insert("redirection".to_string());
    }
    if has_command_substitution {
        risk_markers.insert("command_substitution".to_string());
    }

    let mut seen_command = false;
    for token in tokens {
        let cleaned = token
            .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
            .to_string();
        if cleaned.is_empty() {
            continue;
        }
        if !seen_command && cleaned == significant {
            seen_command = true;
            continue;
        }
        if cleaned.starts_with("--") && cleaned.len() > 2 {
            let key = cleaned.split('=').next().unwrap_or(cleaned.as_str());
            flags.insert(key.to_string());
            continue;
        }
        if cleaned.starts_with('-') && cleaned.len() > 1 {
            if cleaned.len() > 2
                && cleaned.len() <= 4
                && !cleaned.contains('=')
                && !cleaned.starts_with("--")
                && cleaned[1..].chars().all(|c| c.is_ascii_alphabetic())
            {
                for ch in cleaned[1..].chars() {
                    flags.insert(format!("-{ch}"));
                }
            } else {
                flags.insert(cleaned.clone());
            }
            continue;
        }
        if looks_like_path_like_token(&cleaned) {
            paths.insert(cleaned);
            continue;
        }
        if seen_command && !cleaned.contains('=') {
            subcommands.insert(cleaned);
        }
    }

    CommandAwareRetrievalFeatures {
        flags: flags.into_iter().take(24).collect(),
        subcommands: subcommands.into_iter().take(16).collect(),
        paths: paths.into_iter().take(12).collect(),
        risk_markers: risk_markers.into_iter().collect(),
    }
}

fn looks_like_path_like_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    token == "."
        || token == ".."
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
        || token.starts_with('/')
        || token.contains('/')
}

async fn lookup_docs_excerpt(state: &Arc<DaemonState>, command_name: &str) -> String {
    if command_name.trim().is_empty() {
        return String::new();
    }
    let runtime = state.index_runtime.lock().await;
    match lookup_command_docs(
        &runtime.chunks,
        command_name,
        Some("OPTIONS"),
        state.config_snapshot().indexer.lookup_max_bytes,
    ) {
        Ok(found) => truncate_string(&found.text, 4_000),
        Err(_) => String::new(),
    }
}

async fn assemble_task_prompt(
    state: &Arc<DaemonState>,
    payload: &termlm_protocol::StartTask,
    classification: &tasks::ClassificationResult,
) -> context::ContextAssembly {
    let observed = {
        let observed = state.observed.lock().await;
        observed
            .iter()
            .filter(|e| e.shell_id == payload.shell_id)
            .cloned()
            .map(|e| context::TerminalSnippet {
                command: e.command,
                cwd: e.cwd,
                started_at: e.started_at,
                duration_ms: e.duration_ms,
                exit_code: e.exit_code,
                output_capture_status: e.output_capture_status,
                stdout_truncated: e.stdout_truncated,
                stderr_truncated: e.stderr_truncated,
                redactions_applied: e.redactions_applied,
                detected_error_lines: e.detected_error_lines,
                detected_paths: e.detected_paths,
                detected_urls: e.detected_urls,
                detected_commands: e.detected_commands,
                stdout_head: e.stdout_head,
                stdout_tail: e.stdout_tail,
                stderr_head: e.stderr_head,
                stderr_tail: e.stderr_tail,
                stdout_full_ref: e.stdout_full_ref,
                stderr_full_ref: e.stderr_full_ref,
            })
            .collect::<Vec<_>>()
    };
    let session_memory = {
        let memory = state.session_conversations.lock().await;
        memory
            .get(&payload.shell_id)
            .map(session_turn_lines)
            .unwrap_or_default()
    };

    context::assemble_user_prompt(
        &payload.prompt,
        classification,
        &observed,
        &session_memory,
        state.config_snapshot().as_ref(),
    )
}

async fn append_session_turn(
    state: &Arc<DaemonState>,
    shell_id: Uuid,
    user: String,
    assistant: String,
) {
    if user.trim().is_empty() && assistant.trim().is_empty() {
        return;
    }
    let cfg = state.config_snapshot();
    let token_budget = cfg.session.context_window_tokens.max(256) as usize;
    let mut sessions = state.session_conversations.lock().await;
    let conv = sessions.entry(shell_id).or_default();
    conv.turns.push_back(SessionTurn { user, assistant });
    trim_session_conversation(conv, token_budget);
}

fn trim_session_conversation(conv: &mut SessionConversation, token_budget: usize) {
    let mut total_tokens = context::estimate_tokens(&conv.system_prompt);
    for turn in &conv.turns {
        total_tokens = total_tokens
            .saturating_add(context::estimate_tokens(&turn.user))
            .saturating_add(context::estimate_tokens(&turn.assistant));
    }
    while total_tokens > token_budget && !conv.turns.is_empty() {
        if let Some(removed) = conv.turns.pop_front() {
            total_tokens = total_tokens
                .saturating_sub(context::estimate_tokens(&removed.user))
                .saturating_sub(context::estimate_tokens(&removed.assistant));
        } else {
            break;
        }
    }
}

fn session_turn_lines(conv: &SessionConversation) -> Vec<String> {
    conv.turns
        .iter()
        .map(|turn| format!("User: {}\nAssistant: {}", turn.user, turn.assistant))
        .collect()
}

fn session_turn_messages(conv: &SessionConversation) -> Vec<ChatMessage> {
    let mut out = Vec::with_capacity(conv.turns.len().saturating_mul(2));
    for turn in &conv.turns {
        out.push(ChatMessage::user(turn.user.clone()));
        out.push(ChatMessage::assistant(turn.assistant.clone()));
    }
    out
}

async fn append_session_turn_if_session_mode(
    state: &Arc<DaemonState>,
    mode: &str,
    shell_id: Uuid,
    user: String,
    assistant: String,
) {
    if mode == "/p" {
        append_session_turn(state, shell_id, user, assistant).await;
    }
}

async fn session_messages_for_shell(
    state: &Arc<DaemonState>,
    shell_id: Uuid,
    system_prompt: &str,
) -> Vec<ChatMessage> {
    let cfg = state.config_snapshot();
    let token_budget = cfg.session.context_window_tokens.max(256) as usize;
    let mut sessions = state.session_conversations.lock().await;
    let conv = sessions.entry(shell_id).or_default();
    conv.system_prompt = system_prompt.to_string();
    trim_session_conversation(conv, token_budget);
    session_turn_messages(conv)
}

async fn suggest_known_commands(state: &Arc<DaemonState>, needle: &str) -> Vec<String> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut names = {
        let runtime = state.index_runtime.lock().await;
        runtime
            .chunks
            .iter()
            .map(|c| c.command_name.clone())
            .collect::<Vec<_>>()
    };
    names.sort();
    names.dedup();
    names.sort_by_key(|name| levenshtein(name, needle));
    names.into_iter().take(5).collect()
}

async fn process_observed_command(
    state: &Arc<DaemonState>,
    payload: termlm_protocol::ObservedCommand,
) -> Result<()> {
    let cfg = state.config_snapshot();
    if !cfg.terminal_context.enabled || !cfg.terminal_context.capture_all_interactive_commands {
        return Ok(());
    }

    let excluded = should_exclude_observed(
        &payload.expanded_command,
        cfg.terminal_context.exclude_tui_commands,
        &cfg.terminal_context.exclude_command_patterns,
    );

    let max_entries = cfg.terminal_context.max_entries.max(1);
    let max_capture_chars = cfg.terminal_context.max_output_bytes_per_command.max(256);
    let snippet_chars = max_capture_chars.min(2048);
    let capture_status =
        normalize_observed_capture_status(&payload.output_capture_status, excluded);

    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let mut capture_env_redacted = false;
    if capture_status == "captured" {
        stdout_text = decode_b64_maybe(&payload.stdout_b64);
        stderr_text = decode_b64_maybe(&payload.stderr_b64);
        if cfg.terminal_context.redact_secrets {
            stdout_text = termlm_local_tools::redaction::redact_secrets(&stdout_text);
            stderr_text = termlm_local_tools::redaction::redact_secrets(&stderr_text);
        }
        let (stdout_redacted, stdout_env_redactions) =
            redact_capture_env_values(&stdout_text, &cfg.capture.redact_env);
        let (stderr_redacted, stderr_env_redactions) =
            redact_capture_env_values(&stderr_text, &cfg.capture.redact_env);
        capture_env_redacted =
            !stdout_env_redactions.is_empty() || !stderr_env_redactions.is_empty();
        stdout_text = stdout_redacted;
        stderr_text = stderr_redacted;
    }
    let (stdout_head, stdout_tail) = head_tail_snippets(&stdout_text, snippet_chars);
    let (stderr_head, stderr_tail) = head_tail_snippets(&stderr_text, snippet_chars);

    let combined = if stderr_tail.is_empty() {
        stdout_tail.clone()
    } else if stdout_tail.is_empty() {
        stderr_tail.clone()
    } else {
        format!("{stderr_tail}\n{stdout_tail}")
    };
    let detected_error_lines = extract_error_lines(&combined, 6);
    let command_text = if cfg.terminal_context.redact_secrets {
        termlm_local_tools::redaction::redact_secrets(&payload.expanded_command)
    } else {
        payload.expanded_command.clone()
    };
    let detect_text = format!(
        "{}\n{}\n{}\n{}\n{}",
        command_text, stdout_head, stdout_tail, stderr_head, stderr_tail
    );
    let detected_paths = extract_paths(&detect_text, 6);
    let detected_urls = extract_urls(&detect_text, 6);
    let detected_commands = extract_command_names(&payload.expanded_command, 6);

    let observed_root = observed_output_dir(cfg.as_ref());
    let stdout_full_ref = if capture_status == "captured" {
        write_observed_output_ref(
            &observed_root,
            payload.shell_id,
            payload.command_seq,
            "stdout",
            &stdout_text,
        )
    } else {
        None
    };
    let stderr_full_ref = if capture_status == "captured" {
        write_observed_output_ref(
            &observed_root,
            payload.shell_id,
            payload.command_seq,
            "stderr",
            &stderr_text,
        )
    } else {
        None
    };

    let entry = ObservedEntry {
        shell_id: payload.shell_id,
        command_seq: payload.command_seq,
        command: command_text,
        cwd: payload.cwd_after,
        started_at: payload.started_at,
        duration_ms: payload.duration_ms,
        exit_code: payload.exit_status,
        output_capture_status: capture_status,
        stdout_truncated: payload.stdout_truncated,
        stderr_truncated: payload.stderr_truncated,
        redactions_applied: cfg.terminal_context.redact_secrets || capture_env_redacted,
        detected_error_lines,
        detected_paths,
        detected_urls,
        detected_commands,
        stdout_head,
        stdout_tail,
        stderr_head,
        stderr_tail,
        stdout_full_ref,
        stderr_full_ref,
    };

    let mut evicted = Vec::new();
    {
        let mut observed = state.observed.lock().await;
        observed.push_back(entry);
        while observed.len() > max_entries {
            if let Some(old) = observed.pop_front() {
                evicted.push(old);
            }
        }
    }
    for old in &evicted {
        cleanup_observed_entry_refs(&observed_root, old);
    }
    Ok(())
}

fn normalize_observed_capture_status(raw: &str, excluded: bool) -> String {
    if excluded {
        return "excluded_interactive".to_string();
    }
    match raw.trim().to_ascii_lowercase().as_str() {
        "captured" => "captured".to_string(),
        "excluded_interactive" => "excluded_interactive".to_string(),
        "skipped_interactive_tty" | "skipped_not_captured" | "none" | "" => {
            "skipped_interactive_tty".to_string()
        }
        _ => "skipped_interactive_tty".to_string(),
    }
}

fn observed_output_dir(cfg: &AppConfig) -> PathBuf {
    let runtime_pid = resolve_runtime_path(&cfg.daemon.pid_file);
    if let Some(parent) = runtime_pid.parent() {
        return parent.join("observed-output");
    }
    PathBuf::from(".").join("observed-output")
}

fn write_observed_output_ref(
    root: &Path,
    shell_id: Uuid,
    command_seq: u64,
    stream: &str,
    text: &str,
) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    if std::fs::create_dir_all(root).is_err() {
        return None;
    }
    let path = root.join(format!("{shell_id}.{command_seq}.{stream}.txt"));
    if std::fs::write(&path, text.as_bytes()).is_err() {
        return None;
    }
    Some(path.display().to_string())
}

fn cleanup_observed_entry_refs(root: &Path, entry: &ObservedEntry) {
    cleanup_observed_output_ref(root, entry.stdout_full_ref.as_deref());
    cleanup_observed_output_ref(root, entry.stderr_full_ref.as_deref());
}

fn cleanup_observed_output_ref(root: &Path, reference: Option<&str>) {
    let Some(reference) = reference else {
        return;
    };
    let path = PathBuf::from(reference);
    if !path.starts_with(root) {
        return;
    }
    let _ = std::fs::remove_file(path);
}

fn head_tail_snippets(text: &str, max_chars: usize) -> (String, String) {
    if text.is_empty() || max_chars == 0 {
        return (String::new(), String::new());
    }
    let head = truncate_string(text, max_chars);
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return (head, text.to_string());
    }
    let mut tail = text
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    tail.insert(0, '…');
    (head, tail)
}

fn should_exclude_observed(
    command: &str,
    exclude_tui_commands: bool,
    configured_patterns: &[String],
) -> bool {
    if exclude_tui_commands {
        let first = command.split_whitespace().next().unwrap_or_default();
        if matches!(
            first,
            "vim"
                | "nvim"
                | "vi"
                | "emacs"
                | "nano"
                | "less"
                | "more"
                | "man"
                | "watch"
                | "top"
                | "htop"
                | "btop"
                | "tmux"
                | "screen"
                | "fzf"
                | "sk"
                | "ssh"
                | "sftp"
                | "scp"
                | "mosh"
                | "node"
                | "python"
                | "python3"
                | "ruby"
                | "irb"
                | "lua"
                | "julia"
                | "mysql"
                | "psql"
                | "sqlite3"
                | "redis-cli"
                | "mongosh"
        ) {
            return true;
        }
    }

    let defaults = [
        r"^\s*(env|printenv)(\s|$)",
        r"^\s*security\s+find-.*password",
        r"^\s*(op|pass)\s+.*(show|get)",
        r"^\s*gcloud\s+auth\s+print-access-token",
        r"^\s*aws\s+configure\s+get",
    ];
    if defaults.iter().any(|p| {
        regex::RegexBuilder::new(p)
            .case_insensitive(true)
            .build()
            .map(|re| re.is_match(command))
            .unwrap_or(false)
    }) {
        return true;
    }

    configured_patterns.iter().any(|p| {
        regex::RegexBuilder::new(p)
            .case_insensitive(true)
            .build()
            .map(|re| re.is_match(command))
            .unwrap_or(false)
    })
}

fn extract_error_lines(text: &str, max_lines: usize) -> Vec<String> {
    static ERROR_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ERROR_RE.get_or_init(|| {
        regex::RegexBuilder::new(
            r"(?i)\b(error|failed|exception|traceback|not found|permission denied|fatal)\b",
        )
        .build()
        .expect("error regex")
    });

    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || !re.is_match(line) {
            continue;
        }
        if !out.iter().any(|e| e == line) {
            out.push(line.to_string());
            if out.len() >= max_lines {
                break;
            }
        }
    }
    out
}

fn extract_paths(text: &str, max_paths: usize) -> Vec<String> {
    static PATH_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = PATH_RE.get_or_init(|| {
        regex::Regex::new(
            r"(?x)
            (?P<path>
              (?:\./|\../|/)[A-Za-z0-9._/\-]+
            )
        ",
        )
        .expect("path regex")
    });

    let mut out = Vec::new();
    for caps in re.captures_iter(text) {
        let Some(m) = caps.name("path") else {
            continue;
        };
        let path = m.as_str().to_string();
        if !out.iter().any(|p| p == &path) {
            out.push(path);
            if out.len() >= max_paths {
                break;
            }
        }
    }
    out
}

fn extract_urls(text: &str, max_urls: usize) -> Vec<String> {
    static URL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = URL_RE
        .get_or_init(|| regex::Regex::new(r#"(?i)\bhttps?://[^\s<>()"']+"#).expect("url regex"));

    let mut out = Vec::new();
    for m in re.find_iter(text) {
        let candidate = m.as_str().trim_end_matches(['.', ',', ';', ')']);
        if !out.iter().any(|u| u == candidate) {
            out.push(candidate.to_string());
            if out.len() >= max_urls {
                break;
            }
        }
    }
    out
}

fn extract_command_names(command: &str, max_commands: usize) -> Vec<String> {
    let mut out = Vec::new();
    for segment in command.split(|c| ['|', '&', ';'].contains(&c)) {
        let parsed = parse_command(segment);
        let Some(token) = parsed.first_token else {
            continue;
        };
        if !out.iter().any(|c| c == &token) {
            out.push(token);
            if out.len() >= max_commands {
                break;
            }
        }
    }
    out
}

async fn process_reindex_request(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    mode: termlm_protocol::ReindexMode,
) -> Result<()> {
    if matches!(mode, termlm_protocol::ReindexMode::Delta) {
        let progress = state.index_progress.lock().await.clone();
        if progress.phase == "idle" || progress.phase == "complete" {
            {
                let mut next = state.index_progress.lock().await;
                if next.phase == "idle" || next.phase == "complete" {
                    next.phase = "scan".to_string();
                    next.percent = 0.0;
                }
                transport
                    .send(ServerMessage::IndexProgress(next.clone()))
                    .await?;
            }
            let state_for_reindex = Arc::clone(state);
            tokio::spawn(async move {
                if let Err(e) = run_delta_indexing(&state_for_reindex, false).await {
                    warn!("delta reindex request failed: {e:#}");
                }
            });
            return Ok(());
        } else {
            transport
                .send(ServerMessage::IndexProgress(progress))
                .await?;
            return Ok(());
        }
    }

    if matches!(mode, termlm_protocol::ReindexMode::Compact) {
        compact_index(state).await?;
    } else if matches!(mode, termlm_protocol::ReindexMode::Full) {
        if state.index_store.root.exists() {
            let _ = std::fs::remove_dir_all(&state.index_store.root);
        }
        run_delta_indexing(state, true).await?;
    }
    transport
        .send(ServerMessage::IndexProgress(
            state.index_progress.lock().await.clone(),
        ))
        .await?;
    let last_update = state.last_index_update.lock().await.clone();
    if !last_update.added.is_empty()
        || !last_update.updated.is_empty()
        || !last_update.removed.is_empty()
    {
        transport
            .send(ServerMessage::IndexUpdate {
                added: last_update.added,
                updated: last_update.updated,
                removed: last_update.removed,
            })
            .await?;
    }
    Ok(())
}

async fn process_retrieve_request(
    state: &Arc<DaemonState>,
    transport: &mut ipc::ServerTransport,
    payload: termlm_protocol::RetrieveRequest,
) -> Result<()> {
    let cfg = state.config_snapshot();
    let prompt = payload.prompt;
    let top_k = payload.top_k.unwrap_or(cfg.indexer.rag_top_k as u32) as usize;
    let mut query =
        RetrievalQuery::new(prompt.clone(), top_k.max(1), cfg.indexer.rag_min_similarity);
    query.hybrid_enabled = cfg.indexer.hybrid_retrieval_enabled;
    query.lexical_enabled = cfg.indexer.lexical_index_enabled;
    query.lexical_top_k = cfg.indexer.lexical_top_k;
    query.exact_command_boost = cfg.indexer.exact_command_boost;
    query.exact_flag_boost = cfg.indexer.exact_flag_boost;
    query.section_boost_options = cfg.indexer.section_boost_options;
    query.command_aware = cfg.indexer.command_aware_retrieval;
    let use_external_embeddings = { state.index_runtime.lock().await.uses_external_embeddings };
    let query_embedding = if use_external_embeddings {
        embed_query_vector(state, cfg.as_ref(), &prompt).await
    } else {
        None
    };
    let hints = command_hints_from_prompt(&prompt);
    let chunks = {
        let runtime = state.index_runtime.lock().await;
        let mut out = Vec::<RetrievedChunk>::new();
        let mut seen_commands = BTreeSet::<String>::new();

        for hint in hints {
            if out.len() >= top_k {
                break;
            }
            if let Some(chunk) = runtime.chunks.iter().find(|c| c.command_name == hint)
                && seen_commands.insert(chunk.command_name.clone())
            {
                out.push(render_retrieval_chunk(chunk));
                continue;
            }
            if seen_commands.insert(hint.clone())
                && let Some(rendered) = render_retrieval_hint_fallback(&hint)
            {
                out.push(rendered);
            }
        }

        for hit in runtime.retriever.search_with_embedding(
            &query,
            if use_external_embeddings {
                query_embedding.as_deref()
            } else {
                None
            },
        ) {
            if out.len() >= top_k {
                break;
            }
            if !seen_commands.insert(hit.chunk.command_name.clone()) {
                continue;
            }
            out.push(render_retrieval_chunk(&hit.chunk));
        }
        let mut remaining_tokens = cfg.indexer.rag_max_tokens.max(1);
        let mut budgeted = Vec::<RetrievedChunk>::new();
        for mut chunk in out {
            if remaining_tokens == 0 {
                break;
            }
            let est = context::estimate_tokens(&chunk.text);
            if est > remaining_tokens {
                chunk.text = context::trim_to_tokens(&chunk.text, remaining_tokens);
                if chunk.text.trim().is_empty() {
                    break;
                }
                budgeted.push(chunk);
                break;
            }
            remaining_tokens = remaining_tokens.saturating_sub(est);
            budgeted.push(chunk);
        }
        budgeted
    };
    transport
        .send(ServerMessage::RetrievalResult { chunks })
        .await?;
    Ok(())
}

fn render_retrieval_chunk(chunk: &Chunk) -> RetrievedChunk {
    RetrievedChunk {
        command_name: chunk.command_name.clone(),
        section_name: chunk.section_name.clone(),
        path: chunk.path.clone(),
        extraction_method: chunk.extraction_method.clone(),
        chunk_index: chunk.chunk_index,
        total_chunks: chunk.total_chunks,
        doc_hash: chunk.doc_hash.clone(),
        extracted_at: chunk.extracted_at,
        text: truncate_string(&chunk.text, 800),
    }
}

fn build_retriever_for_chunks(
    store: &IndexStore,
    cfg: &AppConfig,
    chunks: Vec<Chunk>,
    embedding_mode: &str,
) -> HybridRetriever {
    let persisted_lexical = if cfg.indexer.lexical_index_enabled {
        match store.load_lexical_index() {
            Ok(index) => index,
            Err(e) => {
                warn!("failed to load persisted lexical index: {e:#}");
                None
            }
        }
    } else {
        None
    };
    let apply_lexical =
        |mut retriever: HybridRetriever,
         lexical: &Option<termlm_indexer::lexical::LexicalIndex>| {
            if let Some(index) = lexical.clone() {
                retriever.set_lexical_index(index);
            }
            retriever
        };

    if embedding_mode != "provider" {
        return apply_lexical(HybridRetriever::lexical_only(chunks), &persisted_lexical);
    }
    if cfg.indexer.vector_storage == "f16"
        && let Ok(Some(mmap)) = store.mmap_file("vectors.f16")
        && let Some(retriever) = HybridRetriever::with_mmap_f16(
            chunks.clone(),
            cfg.indexer.embed_dim,
            cfg.indexer.embed_query_prefix.clone(),
            Arc::new(mmap),
        )
    {
        return apply_lexical(retriever, &persisted_lexical);
    }
    if cfg.indexer.vector_storage == "f32"
        && let Ok(Some(mmap)) = store.mmap_file("vectors.f32")
        && let Some(retriever) = HybridRetriever::with_mmap_f32(
            chunks.clone(),
            cfg.indexer.embed_dim,
            cfg.indexer.embed_query_prefix.clone(),
            Arc::new(mmap),
        )
    {
        return apply_lexical(retriever, &persisted_lexical);
    }
    apply_lexical(HybridRetriever::lexical_only(chunks), &persisted_lexical)
}

fn command_hints_from_prompt(prompt: &str) -> Vec<String> {
    let p = prompt.to_ascii_lowercase();
    let mut hints = Vec::<String>::new();
    let mut push = |cmd: &str| {
        if !hints.iter().any(|c| c == cmd) {
            hints.push(cmd.to_string());
        }
    };

    if p.contains("git") {
        push("git");
    }
    if p.contains("commit")
        || p.contains("branch")
        || p.contains("rebase")
        || p.contains("checkout")
        || p.contains("switch")
        || p.contains("reset")
        || p.contains("stage")
        || p.contains("staged")
        || p.contains("working tree")
    {
        push("git");
    }
    if p.contains("list")
        || p.contains("files")
        || p.contains("directory")
        || p.contains("hidden")
        || p.contains("folders")
    {
        push("ls");
    }
    if p.contains("largest") || p.contains("disk usage") || p.contains("total usage") {
        push("du");
    }
    if p.contains("filesystem")
        || p.contains("filesystems")
        || p.contains("free disk")
        || p.contains("disk free")
        || p.contains("free space")
        || p.contains("capacity")
    {
        push("df");
    }
    if p.contains("find")
        || p.contains("search")
        || p.contains("under")
        || p.contains("modified")
        || p.contains("mtime")
    {
        push("find");
    }
    if p.contains("grep")
        || p.contains("search")
        || p.contains("todo")
        || p.contains("contains")
        || p.contains("line")
        || p.contains("pattern")
        || p.contains("regex")
        || p.contains("case-insensitively")
        || p.contains("import ")
    {
        push("grep");
        push("rg");
    }
    if p.contains("sort") || p.contains("alphabet") {
        push("sort");
    }
    if p.contains("head") || p.contains("first ") {
        push("head");
    }
    if p.contains("tail") || p.contains("last ") {
        push("tail");
    }
    if p.contains("count") || p.contains("how many") {
        push("wc");
    }
    if p.contains("make directory")
        || p.contains("make a directory")
        || p.contains("create directory")
        || p.contains("create directories")
        || p.contains("create a directory")
        || p.contains("create folder")
        || p.contains("create folders")
        || p.contains("create nested")
        || p.contains("mkdir")
        || p.contains("nested directories")
        || (p.contains("nested") && p.contains("director"))
    {
        push("mkdir");
    }
    if p.contains("move ") || p.contains("rename ") {
        push("mv");
    }
    if p.contains("copy ") || p.contains("backup") {
        push("cp");
    }
    if p.contains("archive") || p.contains("extract") || p.contains("tar") {
        push("tar");
    }
    if p.contains("combine") || p.contains("concatenate") {
        push("cat");
    }
    if p.contains("chmod") || p.contains("executable") {
        push("chmod");
    }
    if p.contains("touch") || p.contains("empty file") {
        push("touch");
    }
    if p.contains("symlink") || p.contains("symbolic link") {
        push("ln");
    }
    if p.contains("replace ") || p.contains("substitute ") {
        push("sed");
    }
    if p.contains("empty line") || p.contains("blank line") {
        push("sed");
    }
    if p.contains("column") || p.contains("csv") {
        push("cut");
        push("awk");
    }
    if p.contains("uppercase") || p.contains("lowercase") {
        push("tr");
    }
    if p.contains("listening") || p.contains("port ") {
        push("lsof");
        push("netstat");
        push("ss");
    }
    if p.contains("process") || p.contains("running") {
        push("ps");
    }
    if p.contains("cpu") || p.contains("hardware") {
        push("sysctl");
        push("system_profiler");
    }
    if p.contains("memory") || p.contains("ram") {
        push("vm_stat");
        push("top");
        push("sysctl");
    }
    if p.contains("uptime") || p.contains("logged in") {
        push("uptime");
        push("w");
    }
    if p.contains("delete")
        || p.contains("remove")
        || p.contains("cleanup")
        || p.contains("clean up")
    {
        push("rm");
    }
    if p.contains("brew") {
        push("brew");
    }
    if p.contains("json") {
        push("jq");
        push("python3");
    }
    if p.contains("web") || p.contains("url") || p.contains("http") {
        push("curl");
        push("open");
    }
    if p.contains("compress") || p.contains("zstd") {
        push("zstd");
        push("gzip");
    }
    if p.contains("hash") || p.contains("checksum") || p.contains("duplicate") {
        push("md5");
        push("shasum");
    }
    if p.contains("today") || p.contains("date") || p.contains("last week") {
        push("date");
    }
    if p.contains("longest line") || p.contains("maximum line length") {
        push("awk");
        push("sort");
        push("tail");
    }
    if p.contains("size descending") || p.contains("sort by size") {
        push("ls");
    }
    for command in [
        "ls", "find", "grep", "rg", "sed", "awk", "mkdir", "git", "tar", "du", "head", "tail",
        "cat", "vm_stat", "top", "sysctl",
    ] {
        if mentions_command_token(&p, command) {
            push(command);
        }
    }
    hints
}

fn render_retrieval_hint_fallback(command: &str) -> Option<RetrievedChunk> {
    let summary = match command {
        "ls" => "List directory contents (supports long/all/time sorting variants).",
        "find" => "Search files and directories by predicates like name, size, and mtime.",
        "grep" => "Search file content with regular expressions.",
        "rg" => "Fast recursive grep with smart defaults and globs.",
        "du" => "Estimate disk usage for files and directories.",
        "df" => "Report filesystem disk space usage and capacity.",
        "sort" => "Sort input lines or records.",
        "head" => "Show first N lines of input.",
        "tail" => "Show last N lines of input (or follow).",
        "wc" => "Count lines, words, and bytes.",
        "mkdir" => "Create directories (use -p for nested paths).",
        "mv" => "Move or rename files.",
        "cp" => "Copy files or directories.",
        "tar" => "Create/extract tar archives; supports gzip/zstd options.",
        "cat" => "Concatenate files to standard output.",
        "ln" => "Create hard or symbolic links.",
        "chmod" => "Change file mode bits/permissions.",
        "touch" => "Create files or update timestamps.",
        "sed" => "Stream editor for substitutions and text transforms.",
        "awk" => "Pattern scanning and processing language.",
        "cut" => "Extract columns/fields from lines.",
        "tr" => "Translate/delete characters.",
        "git" => "Distributed version control for commits, branches, and diffs.",
        "lsof" => "List open files and network sockets.",
        "netstat" => "Inspect network connections and routing tables.",
        "ss" => "Socket statistics and listening port inspection.",
        "ps" => "Report running processes.",
        "sysctl" => "Inspect kernel and hardware parameters.",
        "system_profiler" => "Detailed macOS hardware/software reports.",
        "vm_stat" => "macOS virtual memory statistics.",
        "top" => "Live process and resource usage summary.",
        "uptime" => "System uptime and load averages.",
        "w" => "Who is logged in and what they are doing.",
        "rm" => "Remove files or directories (dangerous with recursive flags).",
        "jq" => "Query and transform JSON documents.",
        "python3" => "Run Python scripts and one-liners.",
        "curl" => "Transfer data from URLs.",
        "open" => "Open files/URLs with default macOS handlers.",
        "zstd" => "High-performance compression utility.",
        "gzip" => "GNU gzip compression utility.",
        "md5" => "Compute MD5 checksums (macOS tool).",
        "shasum" => "Compute SHA checksums.",
        "date" => "Print or parse date/time values.",
        "brew" => "Homebrew package manager command.",
        _ => return None,
    };
    let text = summary.to_string();
    Some(RetrievedChunk {
        command_name: command.to_string(),
        section_name: "heuristic".to_string(),
        path: format!("heuristic://command-hint/{command}"),
        extraction_method: "heuristic".to_string(),
        chunk_index: 0,
        total_chunks: 1,
        doc_hash: format!("{:x}", sha2::Sha256::digest(text.as_bytes())),
        extracted_at: chrono::Utc::now(),
        text,
    })
}

fn mentions_command_token(prompt: &str, command: &str) -> bool {
    let Ok(re) = regex::Regex::new(&format!(r"\b{}\b", regex::escape(command))) else {
        return false;
    };
    re.is_match(prompt)
}

async fn prewarm_common_docs(state: &Arc<DaemonState>) -> Result<()> {
    let cfg = state.config_snapshot();
    let warm_targets = [
        "git", "ls", "find", "grep", "sed", "awk", "rg", "cat", "docker", "kubectl", "brew",
        "python3", "node", "cargo", "npm",
    ];
    let known = {
        let runtime = state.index_runtime.lock().await;
        runtime
            .chunks
            .iter()
            .map(|c| c.command_name.clone())
            .collect::<BTreeSet<_>>()
    };

    for command in warm_targets {
        if known.contains(command) {
            let _ = lookup_docs_excerpt(state, command).await;
        }
    }
    {
        let runtime = state.index_runtime.lock().await;
        let _ = termlm_indexer::cheatsheet::build_cheatsheet(
            &runtime.chunks,
            cfg.indexer.cheatsheet_static_count,
            &[],
            &[],
            cfg.context_budget.cheat_sheet_tokens,
        );
    }
    Ok(())
}

async fn bootstrap_index_runtime(state: Arc<DaemonState>) -> Result<()> {
    let cfg = state.config_snapshot();
    if !cfg.indexer.enabled {
        let mut progress = state.index_progress.lock().await;
        progress.phase = "idle".to_string();
        progress.total = 0;
        progress.scanned = 0;
        progress.percent = 100.0;
        return Ok(());
    }

    let expected_model_hash = hash_index_embedding_model(cfg.as_ref());
    let mut needs_full = true;
    let mut loaded_chunks = Vec::new();
    let mut loaded_entries = Vec::new();
    let mut loaded_embedding_mode = "disabled".to_string();

    if let Some(manifest) = state.index_store.load_manifest()? {
        loaded_embedding_mode = manifest.embedding_mode.clone();
        needs_full = manifest.index_version != current_index_version()
            || manifest.embedding_model_hash != expected_model_hash
            || manifest.embed_dim != cfg.indexer.embed_dim
            || manifest.vector_storage != cfg.indexer.vector_storage
            || manifest.query_prefix != cfg.indexer.embed_query_prefix
            || manifest.doc_prefix != cfg.indexer.embed_doc_prefix;
        if !needs_full {
            loaded_chunks = state.index_store.load_chunks().unwrap_or_default();
            loaded_entries = state.index_store.load_entries().unwrap_or_default();
            let (pruned_entries, pruned_chunks, stale_removed) =
                prune_stale_loaded_index(loaded_entries, loaded_chunks);
            loaded_entries = pruned_entries;
            loaded_chunks = pruned_chunks;
            if stale_removed > 0 {
                info!(
                    "pruned {stale_removed} stale indexed binaries from persisted index before serving tasks"
                );
            }
        }
    }

    if !loaded_chunks.is_empty() {
        let binaries = if !loaded_entries.is_empty() {
            loaded_entries
                .iter()
                .map(|entry| IndexedBinary {
                    name: entry.name.clone(),
                    path: entry.path.display().to_string(),
                    mtime_secs: entry.mtime_secs,
                    size: entry.size,
                    inode: entry.inode,
                })
                .collect::<Vec<_>>()
        } else {
            loaded_chunks
                .iter()
                .map(|c| IndexedBinary {
                    name: c.command_name.clone(),
                    path: c.path.clone(),
                    mtime_secs: 0,
                    size: 0,
                    inode: 0,
                })
                .collect::<Vec<_>>()
        };
        let mut runtime = state.index_runtime.lock().await;
        runtime.retriever = build_retriever_for_chunks(
            &state.index_store,
            cfg.as_ref(),
            loaded_chunks.clone(),
            &loaded_embedding_mode,
        );
        runtime.binaries = binaries;
        runtime.chunks = loaded_chunks;
        runtime.uses_external_embeddings = loaded_embedding_mode == "provider";
        runtime.revision = runtime.revision.saturating_add(1);
        let mut progress = state.index_progress.lock().await;
        progress.phase = "complete".to_string();
        progress.total = runtime.binaries.len() as u64;
        progress.scanned = runtime.binaries.len() as u64;
        progress.percent = 100.0;
    }

    run_delta_indexing(&state, needs_full).await
}

fn prune_stale_loaded_index(
    entries: Vec<termlm_indexer::BinaryEntry>,
    chunks: Vec<Chunk>,
) -> (Vec<termlm_indexer::BinaryEntry>, Vec<Chunk>, usize) {
    if entries.is_empty() && chunks.is_empty() {
        return (entries, chunks, 0);
    }

    let mut live_paths = BTreeSet::<String>::new();
    let mut stale_removed = 0usize;

    let kept_entries = if entries.is_empty() {
        Vec::new()
    } else {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.path.exists() {
                let path = entry.path.to_string_lossy().to_string();
                live_paths.insert(path);
                out.push(entry);
            } else {
                stale_removed = stale_removed.saturating_add(1);
            }
        }
        out
    };

    if kept_entries.is_empty() {
        for path in chunks
            .iter()
            .map(|c| c.path.clone())
            .collect::<BTreeSet<_>>()
        {
            if Path::new(&path).exists() {
                live_paths.insert(path);
            } else {
                stale_removed = stale_removed.saturating_add(1);
            }
        }
    }

    let kept_chunks = chunks
        .into_iter()
        .filter(|chunk| live_paths.contains(&chunk.path))
        .collect::<Vec<_>>();

    (kept_entries, kept_chunks, stale_removed)
}

async fn run_index_watch_loop(state: Arc<DaemonState>) -> Result<()> {
    if !state.config_snapshot().indexer.enabled {
        return Ok(());
    }

    let debounce_ms = state.config_snapshot().indexer.fsevents_debounce_ms.max(50);
    let coalesce_secs = state
        .config_snapshot()
        .indexer
        .disk_write_coalesce_secs
        .max(1);
    let initial_watch_paths = collect_index_path_dirs(&state).await;
    let mut watched_path_union = initial_watch_paths
        .into_iter()
        .map(|p| (normalize_path_key(p.to_string_lossy().as_ref()), p))
        .collect::<BTreeMap<_, _>>();

    if watched_path_union.is_empty() {
        info!("index watcher waiting: no watchable PATH directories yet");
    }

    let (tx, mut rx) = mpsc::channel::<Vec<PathBuf>>(32);
    let (watch_cfg_tx, watch_cfg_rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    std::thread::spawn(move || {
        let mut watcher = match termlm_indexer::watch::PathWatcher::new(
            std::time::Duration::from_millis(debounce_ms),
        ) {
            Ok(w) => w,
            Err(e) => {
                warn!("failed to initialize index path watcher: {e:#}");
                return;
            }
        };
        let mut configured = false;

        loop {
            let mut latest_paths = None;
            loop {
                match watch_cfg_rx.try_recv() {
                    Ok(paths) => latest_paths = Some(paths),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                }
            }
            if let Some(paths) = latest_paths {
                configured = !paths.is_empty();
                if let Err(e) = watcher.sync_paths(&paths) {
                    warn!("failed to sync watched index paths: {e:#}");
                }
            }

            if !configured {
                std::thread::sleep(std::time::Duration::from_millis(250));
                continue;
            }

            let Some(changed) = watcher.recv_changed_paths(std::time::Duration::from_secs(1))
            else {
                continue;
            };
            if tx.blocking_send(changed).is_err() {
                break;
            }
        }
    });

    let _ = watch_cfg_tx.send(watched_path_union.values().cloned().collect());

    let mut periodic = tokio::time::interval(std::time::Duration::from_secs(coalesce_secs.max(30)));
    periodic.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    periodic.tick().await;

    loop {
        tokio::select! {
            maybe = rx.recv() => {
                let Some(first_batch) = maybe else {
                    break;
                };
                let mut changed_paths = first_batch;
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(coalesce_secs);
                loop {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        break;
                    }
                    let remaining = deadline.saturating_duration_since(now);
                    match tokio::time::timeout(remaining, rx.recv()).await {
                        Ok(Some(extra)) => changed_paths.extend(extra),
                        Ok(None) | Err(_) => break,
                    }
                }

                let unique_changed = changed_paths
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect::<BTreeSet<_>>();
                info!(
                    "index watcher triggering delta refresh from {} changed paths",
                    unique_changed.len()
                );
                if let Err(e) = run_delta_indexing(&state, false).await {
                    warn!("watch-triggered delta indexing failed: {e:#}");
                }
            }
            _ = periodic.tick() => {
                let refreshed_paths = collect_index_path_dirs(&state).await;
                let mut added = 0usize;
                for path in refreshed_paths {
                    let key = normalize_path_key(path.to_string_lossy().as_ref());
                    if watched_path_union.insert(key, path).is_none() {
                        added = added.saturating_add(1);
                    }
                }
                if added > 0 {
                    info!(
                        "index watcher path set updated (+{}; {} total directories)",
                        added,
                        watched_path_union.len()
                    );
                    let watch_paths = watched_path_union.values().cloned().collect::<Vec<_>>();
                    if watch_cfg_tx.send(watch_paths).is_err() {
                        warn!("index watcher config channel closed");
                        break;
                    }
                }
                if let Err(e) = run_delta_indexing(&state, false).await {
                    warn!("periodic delta indexing failed: {e:#}");
                }
            }
        }
    }
    Ok(())
}

async fn prioritize_index_targets(
    state: &Arc<DaemonState>,
    cfg: &AppConfig,
    binaries: &mut [termlm_indexer::BinaryEntry],
) {
    if binaries.len() <= 1 {
        return;
    }
    if cfg.performance.indexer_priority_mode == "path_order" {
        binaries.sort_by(|a, b| a.path.cmp(&b.path).then(a.name.cmp(&b.name)));
        return;
    }
    if !cfg.indexer.priority_indexing {
        return;
    }

    let mut priority = BTreeMap::<String, i32>::new();
    for (idx, command) in termlm_indexer::cheatsheet::STATIC_PRIORITY_COMMANDS
        .iter()
        .enumerate()
    {
        let score = 10_000i32.saturating_sub(idx as i32 * 10);
        priority.insert((*command).to_string(), score);
    }
    let shell_priority_commands = collect_shell_priority_commands(state).await;
    for (idx, command) in shell_priority_commands.into_iter().enumerate() {
        let score = 15_000i32.saturating_sub(idx as i32 * 3).max(2_500);
        let slot = priority.entry(command).or_insert(0);
        *slot = (*slot).max(score);
    }

    if cfg.indexer.priority_recent_commands {
        let recent_commands = {
            let observed = state.observed.lock().await;
            observed
                .iter()
                .rev()
                .take(200)
                .filter_map(|entry| parse_command(&entry.command).first_token)
                .collect::<Vec<_>>()
        };
        for (idx, command) in recent_commands.iter().enumerate() {
            let bonus = 5_000i32.saturating_sub(idx as i32 * 8).max(200);
            let slot = priority.entry(command.to_string()).or_insert(0);
            *slot = slot.saturating_add(bonus);
        }
    }
    if cfg.indexer.priority_prompt_commands {
        let prompt_commands = {
            let tasks = state.tasks.lock().await;
            let mut out = Vec::<String>::new();
            for task in tasks.values() {
                out.extend(command_hints_from_prompt(&task.original_prompt));
                out.extend(command_tokens_from_prompt(&task.original_prompt));
            }
            out
        };
        for (idx, command) in prompt_commands.iter().enumerate() {
            let bonus = 7_500i32.saturating_sub(idx as i32 * 9).max(250);
            let slot = priority.entry(command.to_string()).or_insert(0);
            *slot = slot.saturating_add(bonus);
        }
    }

    binaries.sort_by(|a, b| {
        let left = priority.get(&a.name).copied().unwrap_or(0);
        let right = priority.get(&b.name).copied().unwrap_or(0);
        right
            .cmp(&left)
            .then(a.path.cmp(&b.path))
            .then(a.name.cmp(&b.name))
    });
}

fn command_tokens_from_prompt(prompt: &str) -> Vec<String> {
    let Ok(re) = regex::Regex::new(r"\b[a-z][a-z0-9._-]{1,31}\b") else {
        return Vec::new();
    };
    let mut out = Vec::<String>::new();
    for m in re.find_iter(&prompt.to_ascii_lowercase()) {
        let tok = m.as_str();
        if matches!(
            tok,
            "the"
                | "and"
                | "for"
                | "with"
                | "that"
                | "this"
                | "show"
                | "list"
                | "find"
                | "search"
                | "from"
                | "into"
                | "your"
                | "what"
                | "when"
                | "where"
        ) {
            continue;
        }
        if !out.iter().any(|v| v == tok) {
            out.push(tok.to_string());
        }
    }
    out
}

async fn run_delta_indexing(state: &Arc<DaemonState>, force_full: bool) -> Result<()> {
    if !state.config_snapshot().indexer.enabled {
        return Ok(());
    }
    let _index_write_guard = state.index_write_lock.lock().await;

    let cfg = state.config_snapshot();
    let path_dirs = collect_index_path_dirs(state).await;
    let path_union = path_dirs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(":");

    let (virtual_entries, synthetic_docs_map) = collect_shell_virtual_entries(state).await;
    let total_virtual_entries = virtual_entries.len();
    let reserved_slots = virtual_entries.len().min(cfg.indexer.max_binaries);
    let discover_limit = cfg
        .indexer
        .max_binaries
        .saturating_sub(reserved_slots)
        .max(1);
    let discovered = discover_binaries_with_stats(&path_union, discover_limit);
    let mut binaries = discovered.entries;
    let mut binaries_cap_hit = discovered.capped;
    let mut seen_virtual = BTreeSet::<String>::new();
    let mut consumed_virtual_entries = 0usize;
    for entry in virtual_entries {
        consumed_virtual_entries += 1;
        if binaries.len() >= cfg.indexer.max_binaries {
            binaries_cap_hit = true;
            break;
        }
        let path_key = entry.path.to_string_lossy().to_string();
        if seen_virtual.insert(path_key) {
            binaries.push(entry);
        }
    }
    if consumed_virtual_entries < total_virtual_entries {
        binaries_cap_hit = true;
    }
    prioritize_index_targets(state, cfg.as_ref(), &mut binaries).await;
    {
        let mut progress = state.index_progress.lock().await;
        progress.total = binaries.len() as u64;
        progress.scanned = 0;
        progress.percent = 0.0;
        progress.phase = "scan".to_string();
    }

    let chunker = Chunker::new(cfg.indexer.chunk_max_tokens.saturating_mul(4));
    let synthetic_docs_map = Arc::new(synthetic_docs_map);
    let previous_entries = if force_full {
        Vec::new()
    } else {
        state.index_store.load_entries().unwrap_or_default()
    };
    let previous_chunks = if force_full {
        Vec::new()
    } else {
        state.index_store.load_chunks().unwrap_or_default()
    };

    let mut previous_by_path = BTreeMap::new();
    for entry in previous_entries {
        previous_by_path.insert(entry.path.to_string_lossy().to_string(), entry);
    }
    let mut previous_chunks_by_path = BTreeMap::<String, Vec<Chunk>>::new();
    for chunk in previous_chunks {
        previous_chunks_by_path
            .entry(chunk.path.clone())
            .or_default()
            .push(chunk);
    }
    for value in previous_chunks_by_path.values_mut() {
        value.sort_by_key(|chunk| chunk.chunk_index);
    }

    let current_path_set = binaries
        .iter()
        .map(|b| b.path.to_string_lossy().to_string())
        .collect::<BTreeSet<_>>();
    let removed_paths = previous_by_path
        .keys()
        .filter(|k| !current_path_set.contains(*k))
        .cloned()
        .collect::<Vec<_>>();
    let removed_count = removed_paths.len();

    let mut reused_chunks_by_path = BTreeMap::<String, Vec<Chunk>>::new();
    let mut extract_targets = Vec::new();
    let mut added_paths = BTreeSet::new();
    let mut updated_paths = BTreeSet::new();
    for bin in &binaries {
        let path = bin.path.to_string_lossy().to_string();
        if force_full {
            extract_targets.push(bin.clone());
            continue;
        }

        if let Some(prev_entry) = previous_by_path.get(&path) {
            if same_binary_signature(prev_entry, bin) {
                if let Some(reused) = previous_chunks_by_path.remove(&path) {
                    reused_chunks_by_path.insert(path, reused);
                } else {
                    updated_paths.insert(path.clone());
                    extract_targets.push(bin.clone());
                }
            } else {
                updated_paths.insert(path.clone());
                extract_targets.push(bin.clone());
            }
        } else {
            added_paths.insert(path.clone());
            extract_targets.push(bin.clone());
        }
    }
    let tombstoned_chunks = previous_chunks_by_path
        .values()
        .flat_map(|chunks| chunks.iter().cloned())
        .collect::<Vec<_>>();
    let tombstoned_count = tombstoned_chunks.len();

    {
        let mut progress = state.index_progress.lock().await;
        progress.phase = "extract".to_string();
    }

    let mut extracted_chunks_by_path = BTreeMap::<String, Vec<Chunk>>::new();
    if !extract_targets.is_empty() {
        let max_concurrency = effective_indexer_concurrency(cfg.as_ref());
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrency));
        let mut tasks = Vec::with_capacity(extract_targets.len());
        for (i, bin) in extract_targets.into_iter().enumerate() {
            if i > 0
                && i % 100 == 0
                && let Some(loadavg) = one_minute_loadavg()
                && loadavg > f64::from(cfg.indexer.max_loadavg)
            {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }

            let permit = semaphore.clone().acquire_owned().await?;
            let chunker = chunker.clone();
            let max_doc_bytes = cfg.indexer.max_doc_bytes;
            let synthetic_docs_map = Arc::clone(&synthetic_docs_map);
            tasks.push(tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let path = bin.path.to_string_lossy().to_string();
                let chunks = extract_chunks_for_binary(
                    &chunker,
                    &bin,
                    max_doc_bytes,
                    synthetic_docs_map.get(&path),
                );
                (path, chunks)
            }));
        }
        for task in tasks {
            let (path, chunks) = task.await?;
            extracted_chunks_by_path.insert(path, chunks);
        }
    }

    {
        let mut progress = state.index_progress.lock().await;
        progress.phase = "embed".to_string();
    }

    let mut chunks = Vec::new();
    let mut indexed = Vec::new();
    let mut stored_entries = Vec::new();
    let limit = cfg.indexer.max_chunks;
    let max_total_doc_bytes = 200usize * 1024 * 1024;
    let mut total_doc_bytes = 0usize;
    let mut doc_bytes_capped = false;
    let mut chunk_count_capped = false;
    let mut reused_count = 0usize;
    let mut extracted_count = 0usize;
    let added_count = added_paths.len();
    let updated_count = updated_paths.len();
    for (idx, bin) in binaries.iter().enumerate() {
        if chunks.len() >= limit {
            chunk_count_capped = true;
            break;
        }
        if total_doc_bytes >= max_total_doc_bytes {
            doc_bytes_capped = true;
            break;
        }

        let path = bin.path.to_string_lossy().to_string();
        let mut command_chunks = if let Some(extracted) = extracted_chunks_by_path.remove(&path) {
            extracted_count += 1;
            extracted
        } else if let Some(reused) = reused_chunks_by_path.remove(&path) {
            reused_count += 1;
            reused
        } else {
            extracted_count += 1;
            extract_chunks_for_binary(
                &chunker,
                bin,
                cfg.indexer.max_doc_bytes,
                synthetic_docs_map.get(&path),
            )
        };

        for chunk in &mut command_chunks {
            chunk.command_name = bin.name.clone();
            chunk.path = path.clone();
        }
        retag_chunks_for_command(&mut command_chunks);

        if chunks.len() + command_chunks.len() > limit {
            chunk_count_capped = true;
            command_chunks.truncate(limit.saturating_sub(chunks.len()));
            retag_chunks_for_command(&mut command_chunks);
        }
        if !command_chunks.is_empty() {
            let available_bytes = max_total_doc_bytes.saturating_sub(total_doc_bytes);
            let mut consumed = 0usize;
            let mut keep = 0usize;
            for chunk in &command_chunks {
                let next = consumed.saturating_add(chunk.text.len());
                if next > available_bytes {
                    break;
                }
                consumed = next;
                keep += 1;
            }
            if keep < command_chunks.len() {
                command_chunks.truncate(keep);
                retag_chunks_for_command(&mut command_chunks);
                doc_bytes_capped = true;
            }
            total_doc_bytes = total_doc_bytes.saturating_add(consumed);
        }
        if command_chunks.is_empty() {
            continue;
        }
        chunks.extend(command_chunks);
        indexed.push(IndexedBinary {
            name: bin.name.clone(),
            path: path.clone(),
            mtime_secs: bin.mtime_secs,
            size: bin.size,
            inode: bin.inode,
        });
        stored_entries.push(bin.clone());
        let mut progress = state.index_progress.lock().await;
        progress.scanned = (idx + 1) as u64;
        progress.percent = if progress.total == 0 {
            100.0
        } else {
            (progress.scanned as f32 / progress.total as f32) * 100.0
        };
    }

    let doc_inputs = build_doc_embedding_inputs(&chunks, cfg.as_ref());
    let doc_embeddings = embed_texts_with_provider(state, cfg.as_ref(), &doc_inputs).await?;
    let embedding_mode = if doc_embeddings.is_some() {
        "provider".to_string()
    } else {
        "disabled".to_string()
    };

    let manifest = IndexManifest {
        index_version: current_index_version(),
        embedding_model_hash: hash_index_embedding_model(cfg.as_ref()),
        embedding_mode: embedding_mode.clone(),
        embed_dim: cfg.indexer.embed_dim,
        vector_storage: cfg.indexer.vector_storage.clone(),
        chunk_count: chunks.len(),
        generated_at: chrono::Utc::now(),
        query_prefix: cfg.indexer.embed_query_prefix.clone(),
        doc_prefix: cfg.indexer.embed_doc_prefix.clone(),
    };

    if force_full && state.index_store.root.exists() {
        let _ = std::fs::remove_dir_all(&state.index_store.root);
    }
    state.index_store.ensure_layout()?;
    state.index_store.write_manifest_atomic(&manifest)?;
    state
        .index_store
        .write_layout_artifacts(LayoutWriteArtifacts {
            entries: &stored_entries,
            chunks: &chunks,
            tombstoned_chunks: &tombstoned_chunks,
            lexical_index_enabled: cfg.indexer.lexical_index_enabled,
            embed_dim: cfg.indexer.embed_dim,
            vector_storage: &cfg.indexer.vector_storage,
            doc_prefix: &cfg.indexer.embed_doc_prefix,
            embeddings_f32: doc_embeddings.as_deref(),
        })?;

    {
        let mut runtime = state.index_runtime.lock().await;
        runtime.retriever = build_retriever_for_chunks(
            &state.index_store,
            cfg.as_ref(),
            chunks.clone(),
            &manifest.embedding_mode,
        );
        runtime.chunks = chunks;
        runtime.binaries = indexed;
        runtime.uses_external_embeddings = manifest.embedding_mode == "provider";
        runtime.revision = runtime.revision.saturating_add(1);
    }

    let mut progress = state.index_progress.lock().await;
    progress.phase = "complete".to_string();
    progress.scanned = progress.total;
    progress.percent = 100.0;

    let index_update = IndexUpdateSummary {
        added: added_paths.iter().cloned().collect(),
        updated: updated_paths.iter().cloned().collect(),
        removed: removed_paths,
    };
    info!(
        "IndexUpdate {{ added: {:?}, updated: {:?}, removed: {:?} }}",
        index_update.added, index_update.updated, index_update.removed
    );
    {
        let mut guard = state.last_index_update.lock().await;
        *guard = index_update.clone();
    }
    if !index_update.is_empty() {
        let _ = state.index_update_tx.send(index_update);
    }

    info!(
        "delta indexing complete: extracted={extracted_count} reused={reused_count} added={added_count} updated={updated_count} removed={removed_count} tombstoned={tombstoned_count} chunks={} embedding_mode={}",
        manifest.chunk_count, manifest.embedding_mode
    );
    if force_full {
        info!("full index rebuild completed");
    }
    if binaries_cap_hit {
        warn!(
            "index binary cap reached at {}; remaining binaries were skipped",
            cfg.indexer.max_binaries
        );
    }
    if chunk_count_capped {
        warn!(
            "index chunk cap reached at {}; remaining chunks were skipped",
            cfg.indexer.max_chunks
        );
    }
    if doc_bytes_capped {
        warn!(
            "index docs cap reached at {} bytes; remaining command docs were skipped",
            max_total_doc_bytes
        );
    }
    Ok(())
}

fn retag_chunks_for_command(chunks: &mut [Chunk]) {
    let total = chunks.len();
    for (idx, chunk) in chunks.iter_mut().enumerate() {
        chunk.chunk_index = idx;
        chunk.total_chunks = total;
    }
}

fn extract_chunks_for_binary(
    chunker: &Chunker,
    bin: &termlm_indexer::BinaryEntry,
    max_doc_bytes: usize,
    synthetic: Option<&SyntheticDocSpec>,
) -> Vec<Chunk> {
    if let Some(spec) = synthetic {
        let normalized = normalize_doc_text(&spec.doc_text);
        return chunker.chunk_document(
            &bin.name,
            &bin.path.to_string_lossy(),
            &spec.extraction_method,
            &normalized,
        );
    }
    let extracted = termlm_indexer::extract::extract_docs_with_method(
        &bin.name,
        &bin.path.to_string_lossy(),
        max_doc_bytes,
    )
    .unwrap_or_else(|_| termlm_indexer::extract::ExtractedDocs {
        text: "no documentation available".to_string(),
        method: "stub".to_string(),
    });
    let normalized = normalize_doc_text(&extracted.text);
    chunker.chunk_document(
        &bin.name,
        &bin.path.to_string_lossy(),
        &extracted.method,
        &normalized,
    )
}

async fn collect_shell_priority_commands(state: &Arc<DaemonState>) -> BTreeSet<String> {
    let mut builtins = BTreeSet::<String>::new();
    let mut aliases = BTreeSet::<String>::new();
    let mut functions = BTreeSet::<String>::new();

    for builtin in &state.default_zsh_builtins {
        if let Some(name) = normalize_shell_symbol_name(builtin) {
            builtins.insert(name);
        }
    }

    {
        let reg = state.registry.lock().await;
        for (_, session) in reg.iter() {
            if !matches!(session.shell_kind, ShellKind::Zsh) {
                continue;
            }
            if let Some(ctx) = session.context.as_ref() {
                ingest_shell_context_names(ctx, &mut builtins, &mut aliases, &mut functions);
            }
        }
    }
    {
        let detached = state.detached_contexts.lock().await;
        for (_, ctx) in detached.iter() {
            if !matches!(ctx.shell_kind, ShellKind::Zsh) {
                continue;
            }
            ingest_shell_context_names(ctx, &mut builtins, &mut aliases, &mut functions);
        }
    }

    let mut out = BTreeSet::<String>::new();
    out.extend(builtins);
    out.extend(aliases);
    out.extend(functions);
    out.extend(zsh_reserved_words().iter().map(|w| (*w).to_string()));
    out
}

async fn collect_shell_virtual_entries(
    state: &Arc<DaemonState>,
) -> (
    Vec<termlm_indexer::BinaryEntry>,
    BTreeMap<String, SyntheticDocSpec>,
) {
    let mut builtins = BTreeSet::<String>::new();
    let mut aliases = BTreeMap::<String, String>::new();
    let mut functions = BTreeMap::<String, String>::new();

    for builtin in &state.default_zsh_builtins {
        if let Some(name) = normalize_shell_symbol_name(builtin) {
            builtins.insert(name);
        }
    }

    {
        let reg = state.registry.lock().await;
        for (_, session) in reg.iter() {
            if !matches!(session.shell_kind, ShellKind::Zsh) {
                continue;
            }
            if let Some(ctx) = session.context.as_ref() {
                ingest_shell_context_names_and_docs(
                    ctx,
                    &mut builtins,
                    &mut aliases,
                    &mut functions,
                );
            }
        }
    }
    {
        let detached = state.detached_contexts.lock().await;
        for (_, ctx) in detached.iter() {
            if !matches!(ctx.shell_kind, ShellKind::Zsh) {
                continue;
            }
            ingest_shell_context_names_and_docs(ctx, &mut builtins, &mut aliases, &mut functions);
        }
    }

    let mut entries = Vec::<termlm_indexer::BinaryEntry>::new();
    let mut docs = BTreeMap::<String, SyntheticDocSpec>::new();

    for name in builtins {
        let doc = format!(
            "NAME\n{name} - zsh builtin command\n\nDESCRIPTION\n\
This command is provided by zsh as a shell builtin in the current environment.\n\
Use `man zshbuiltins` for full reference details."
        );
        let (entry, spec) = build_virtual_index_entry("builtin", &name, doc, "builtin");
        docs.insert(entry.path.to_string_lossy().to_string(), spec);
        entries.push(entry);
    }
    for (name, expansion) in aliases {
        let doc = format!(
            "NAME\n{name} - zsh alias\n\nEXPANSION\n{expansion}\n\nDESCRIPTION\n\
Alias captured from active shell context. Expansion may contain shell syntax."
        );
        let (entry, spec) = build_virtual_index_entry("alias", &name, doc, "alias");
        docs.insert(entry.path.to_string_lossy().to_string(), spec);
        entries.push(entry);
    }
    for (name, body_prefix) in functions {
        let doc = format!(
            "NAME\n{name} - zsh shell function\n\nBODY_PREFIX\n{body_prefix}\n\nDESCRIPTION\n\
Function captured from active shell context. BODY_PREFIX is a bounded preview."
        );
        let (entry, spec) = build_virtual_index_entry("function", &name, doc, "function");
        docs.insert(entry.path.to_string_lossy().to_string(), spec);
        entries.push(entry);
    }

    (entries, docs)
}

fn build_virtual_index_entry(
    kind: &str,
    name: &str,
    doc_text: String,
    extraction_method: &str,
) -> (termlm_indexer::BinaryEntry, SyntheticDocSpec) {
    let slug = sanitize_virtual_symbol(name);
    let path = format!("{kind}://zsh/{}-{}", slug, hash_prefix(name));
    let signature = synthetic_signature_seed(kind, name, &doc_text);
    let mtime_secs = i64::from_le_bytes(signature.to_le_bytes());
    let entry = termlm_indexer::BinaryEntry {
        name: name.to_string(),
        path: PathBuf::from(path),
        mtime_secs,
        size: doc_text.len() as u64,
        inode: signature.rotate_left(17),
    };
    let spec = SyntheticDocSpec {
        extraction_method: extraction_method.to_string(),
        doc_text,
    };
    (entry, spec)
}

fn synthetic_signature_seed(kind: &str, name: &str, doc_text: &str) -> u64 {
    let digest = sha2::Sha256::digest(format!("{kind}\n{name}\n{doc_text}").as_bytes());
    u64::from_le_bytes(digest[0..8].try_into().unwrap_or([0u8; 8]))
}

fn sanitize_virtual_symbol(input: &str) -> String {
    let out = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if out.is_empty() {
        "symbol".to_string()
    } else {
        out
    }
}

fn normalize_shell_symbol_name(input: &str) -> Option<String> {
    let name = input.trim();
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

fn ingest_shell_context_names(
    ctx: &termlm_protocol::ShellContext,
    builtins: &mut BTreeSet<String>,
    aliases: &mut BTreeSet<String>,
    functions: &mut BTreeSet<String>,
) {
    for builtin in &ctx.builtins {
        if let Some(name) = normalize_shell_symbol_name(builtin) {
            builtins.insert(name);
        }
    }
    for alias in &ctx.aliases {
        if let Some(name) = normalize_shell_symbol_name(&alias.name) {
            aliases.insert(name);
        }
    }
    for function in &ctx.functions {
        if let Some(name) = normalize_shell_symbol_name(&function.name) {
            functions.insert(name);
        }
    }
}

fn ingest_shell_context_names_and_docs(
    ctx: &termlm_protocol::ShellContext,
    builtins: &mut BTreeSet<String>,
    aliases: &mut BTreeMap<String, String>,
    functions: &mut BTreeMap<String, String>,
) {
    for builtin in &ctx.builtins {
        if let Some(name) = normalize_shell_symbol_name(builtin) {
            builtins.insert(name);
        }
    }
    for alias in &ctx.aliases {
        if let Some(name) = normalize_shell_symbol_name(&alias.name) {
            aliases
                .entry(name)
                .or_insert_with(|| alias.expansion.clone());
        }
    }
    for function in &ctx.functions {
        if let Some(name) = normalize_shell_symbol_name(&function.name) {
            functions
                .entry(name)
                .or_insert_with(|| function.body_prefix.clone());
        }
    }
}

fn same_binary_signature(a: &termlm_indexer::BinaryEntry, b: &termlm_indexer::BinaryEntry) -> bool {
    a.mtime_secs == b.mtime_secs && a.size == b.size && a.inode == b.inode
}

fn one_minute_loadavg() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut loads = [0f64; 3];
        // SAFETY: getloadavg writes up to `n` contiguous f64 values to provided pointer.
        let rc = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
        if rc >= 1 { Some(loads[0]) } else { None }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

async fn collect_index_path_dirs(state: &Arc<DaemonState>) -> Vec<PathBuf> {
    let cfg = state.config_snapshot();
    let mut raw_paths = Vec::new();
    {
        let reg = state.registry.lock().await;
        for (_, session) in reg.iter() {
            if let Some(path) = session.env_subset.get("PATH") {
                raw_paths.push(path.clone());
            }
        }
    }
    if raw_paths.is_empty() {
        raw_paths.push(std::env::var("PATH").unwrap_or_default());
    }

    let ignored = cfg
        .indexer
        .ignore_paths
        .iter()
        .map(|p| normalize_path_key(&expand_home_path(p)))
        .collect::<Vec<_>>();

    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for path_var in raw_paths {
        for part in path_var.split(':') {
            if part.trim().is_empty() {
                continue;
            }
            let expanded = expand_home_path(part);
            let path = PathBuf::from(expanded);
            let key = normalize_path_key(path.to_string_lossy().as_ref());
            if ignored.iter().any(|prefix| key.starts_with(prefix)) {
                continue;
            }
            if !path.exists() || !path.is_dir() {
                continue;
            }
            if seen.insert(key) {
                out.push(path);
            }
        }
    }
    for extra in &cfg.indexer.extra_paths {
        if extra.trim().is_empty() {
            continue;
        }
        let expanded = expand_home_path(extra);
        let path = PathBuf::from(expanded);
        let key = normalize_path_key(path.to_string_lossy().as_ref());
        if ignored.iter().any(|prefix| key.starts_with(prefix)) {
            continue;
        }
        if !path.exists() || !path.is_dir() {
            continue;
        }
        if seen.insert(key) {
            out.push(path);
        }
    }
    out
}

fn expand_home_path(input: &str) -> String {
    if input == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| input.to_string());
    }
    if let Some(rest) = input.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return format!("{home}/{rest}");
    }
    input.to_string()
}

fn normalize_path_key(path: &str) -> String {
    path.trim_end_matches('/').to_string()
}

async fn compact_index(state: &Arc<DaemonState>) -> Result<()> {
    let _index_write_guard = state.index_write_lock.lock().await;
    let (chunks, binaries) = {
        let runtime = state.index_runtime.lock().await;
        let bins = runtime
            .binaries
            .iter()
            .map(|b| termlm_indexer::BinaryEntry {
                name: b.name.clone(),
                path: PathBuf::from(&b.path),
                mtime_secs: b.mtime_secs,
                size: b.size,
                inode: b.inode,
            })
            .collect::<Vec<_>>();
        (runtime.chunks.clone(), bins)
    };

    let cfg = state.config_snapshot();
    let doc_inputs = build_doc_embedding_inputs(&chunks, cfg.as_ref());
    let doc_embeddings = embed_texts_with_provider(state, cfg.as_ref(), &doc_inputs).await?;
    let embedding_mode = if doc_embeddings.is_some() {
        "provider".to_string()
    } else {
        "disabled".to_string()
    };

    let manifest = IndexManifest {
        index_version: current_index_version(),
        embedding_model_hash: hash_index_embedding_model(cfg.as_ref()),
        embedding_mode: embedding_mode.clone(),
        embed_dim: cfg.indexer.embed_dim,
        vector_storage: cfg.indexer.vector_storage.clone(),
        chunk_count: chunks.len(),
        generated_at: chrono::Utc::now(),
        query_prefix: cfg.indexer.embed_query_prefix.clone(),
        doc_prefix: cfg.indexer.embed_doc_prefix.clone(),
    };

    state.index_store.ensure_layout()?;
    state.index_store.write_manifest_atomic(&manifest)?;
    state
        .index_store
        .write_layout_artifacts(LayoutWriteArtifacts {
            entries: &binaries,
            chunks: &chunks,
            tombstoned_chunks: &[],
            lexical_index_enabled: cfg.indexer.lexical_index_enabled,
            embed_dim: cfg.indexer.embed_dim,
            vector_storage: &cfg.indexer.vector_storage,
            doc_prefix: &cfg.indexer.embed_doc_prefix,
            embeddings_f32: doc_embeddings.as_deref(),
        })?;

    let mut runtime = state.index_runtime.lock().await;
    runtime.retriever = build_retriever_for_chunks(
        &state.index_store,
        cfg.as_ref(),
        chunks,
        &manifest.embedding_mode,
    );
    runtime.uses_external_embeddings = manifest.embedding_mode == "provider";
    runtime.revision = runtime.revision.saturating_add(1);
    *state.last_index_update.lock().await = IndexUpdateSummary::default();
    Ok(())
}

async fn lookup_task_session(state: &Arc<DaemonState>, task: &InFlightTask) -> ShellSession {
    if let Some(found) = state.registry.lock().await.get(&task.shell_id).cloned() {
        return found;
    }
    let detached = state
        .detached_contexts
        .lock()
        .await
        .get(&task.shell_id)
        .cloned();
    ShellSession {
        shell_pid: 0,
        tty: "unknown".to_string(),
        shell_kind: ShellKind::Zsh,
        shell_version: "unknown".to_string(),
        env_subset: BTreeMap::new(),
        context: detached,
    }
}

fn hash_index_embedding_model(cfg: &AppConfig) -> String {
    if cfg.indexer.embedding_provider == "ollama" {
        return format!(
            "ollama:{}:{}",
            cfg.ollama.endpoint, cfg.indexer.ollama_embed_model
        );
    }

    let embed_path = resolve_models_dir(&cfg.model.models_dir).join(&cfg.indexer.embed_filename);
    match blake3_file_hash(&embed_path) {
        Ok(hash) => hash,
        Err(e) => {
            warn!(
                "embedding model hash unavailable for {}: {e:#}",
                embed_path.display()
            );
            "missing".to_string()
        }
    }
}

fn blake3_file_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn current_index_version() -> u32 {
    3
}

fn decode_b64_maybe(input: &Option<String>) -> String {
    let Some(encoded) = input else {
        return String::new();
    };
    match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(_) => String::new(),
    }
}

fn redact_capture_env_values(input: &str, keys: &[String]) -> (String, Vec<String>) {
    if input.is_empty() || keys.is_empty() {
        return (input.to_string(), Vec::new());
    }

    let mut text = input.to_string();
    let mut applied = Vec::new();
    for key in keys {
        let Ok(secret) = std::env::var(key) else {
            continue;
        };
        if secret.is_empty() {
            continue;
        }
        if text.contains(&secret) {
            text = text.replace(&secret, &format!("[REDACTED:{key}]"));
            applied.push(key.clone());
        }
    }
    (text, applied)
}

fn normalize_doc_text(input: &str) -> String {
    // Many man outputs include overstrike sequences like "N\bNA\bAM\bME\bE".
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '\u{0008}' {
            out.push(chars[i + 2]);
            i += 3;
            continue;
        }
        if chars[i] != '\u{0008}' {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

fn builtins_cache_path(index_root: &Path) -> PathBuf {
    index_root.join("builtins.zsh.json")
}

fn legacy_builtins_cache_path(index_root: &Path) -> PathBuf {
    index_root.join("builtins.json")
}

fn load_or_extract_zsh_builtins(index_root: &Path) -> Result<BTreeSet<String>> {
    let cache_path = builtins_cache_path(index_root);
    if cache_path.exists() {
        let raw = std::fs::read_to_string(&cache_path)
            .with_context(|| format!("read {}", cache_path.display()))?;
        let parsed = serde_json::from_str::<Vec<String>>(&raw)
            .with_context(|| format!("parse {}", cache_path.display()))?;
        return Ok(parsed.into_iter().collect::<BTreeSet<_>>());
    }
    let legacy_path = legacy_builtins_cache_path(index_root);
    if legacy_path.exists() {
        let raw = std::fs::read_to_string(&legacy_path)
            .with_context(|| format!("read {}", legacy_path.display()))?;
        let parsed = serde_json::from_str::<Vec<String>>(&raw)
            .with_context(|| format!("parse {}", legacy_path.display()))?
            .into_iter()
            .collect::<BTreeSet<_>>();
        if !parsed.is_empty() {
            let _ = write_zsh_builtins_cache(&cache_path, &parsed);
            return Ok(parsed);
        }
    }

    let man_text = termlm_indexer::extract::extract_docs("zshbuiltins", "zshbuiltins", 512 * 1024)
        .unwrap_or_default();
    let parsed = parse_zsh_builtins_from_man(&man_text);
    if !parsed.is_empty() {
        write_zsh_builtins_cache(&cache_path, &parsed)?;
    }
    Ok(parsed)
}

fn write_zsh_builtins_cache(path: &Path, parsed: &BTreeSet<String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let payload = serde_json::to_vec_pretty(&parsed.iter().cloned().collect::<Vec<_>>())?;
    {
        let mut file =
            std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(&payload)
            .with_context(|| format!("write {}", tmp.display()))?;
        file.flush()
            .with_context(|| format!("flush {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

fn parse_zsh_builtins_from_man(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(re) = regex::Regex::new(r"^\s{2,}([a-z][a-z0-9_-]+)\b") else {
        return out;
    };
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "from", "into", "when", "then", "else",
        "done", "function", "where", "which", "each",
    ];
    for line in text.lines() {
        if let Some(caps) = re.captures(line) {
            let token = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            if token.len() < 2 || STOPWORDS.contains(&token) {
                continue;
            }
            out.insert(token.to_string());
        }
    }
    out
}

fn extract_execute_shell_command_from_partial_tagged_call(
    text: &str,
) -> Option<termlm_inference::ToolCall> {
    let start = text.find("call:execute_shell_command")?;
    let tail = &text[start..];
    let cmd_pos = tail.find("cmd:")?;
    let after_cmd = &tail[cmd_pos + 4..];

    for marker in ["<|\"|>", "<|\\\"|>"] {
        if let Some(open) = after_cmd.find(marker) {
            let rest = &after_cmd[open + marker.len()..];
            let candidate = if let Some(close) = rest.find(marker) {
                rest[..close].trim()
            } else {
                let mut cutoff = rest.len();
                for delim in [
                    ",commands_used",
                    ",expected_effect",
                    ",intent",
                    "<tool_call",
                    "<|/tool_call|>",
                    "\n",
                    "}",
                    ",",
                ] {
                    if let Some(pos) = rest.find(delim) {
                        cutoff = cutoff.min(pos);
                    }
                }
                rest[..cutoff].trim()
            };
            if !candidate.is_empty() {
                return Some(termlm_inference::ToolCall {
                    name: "execute_shell_command".to_string(),
                    arguments: serde_json::json!({ "cmd": candidate }),
                });
            }
        }
    }

    None
}

fn resolve_index_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/termlm/index")
}

fn levenshtein(a: &str, b: &str) -> usize {
    let mut costs: Vec<usize> = (0..=b.chars().count()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev_diag = i;
        costs[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let temp = costs[j + 1];
            let replace_cost = if ca == cb { prev_diag } else { prev_diag + 1 };
            let insert_cost = costs[j + 1] + 1;
            let delete_cost = costs[j] + 1;
            costs[j + 1] = replace_cost.min(insert_cost).min(delete_cost);
            prev_diag = temp;
        }
    }
    costs[b.chars().count()]
}

async fn cached_command_exists(
    state: &Arc<DaemonState>,
    session: &ShellSession,
    token: &str,
) -> bool {
    let token = token.trim();
    if token.is_empty() {
        return false;
    }
    let cfg = state.config_snapshot();
    if !cfg.cache.enabled {
        return command_exists(token, session);
    }

    let index_revision = { state.index_runtime.lock().await.revision };
    let path_env = session
        .env_subset
        .get("PATH")
        .map(String::as_str)
        .unwrap_or_default();
    let context_hash = session
        .context
        .as_ref()
        .map(|c| c.context_hash.as_str())
        .unwrap_or_default();
    let key = cache_key(
        "command_exists",
        &[
            token.to_string(),
            format!("{:?}", session.shell_kind),
            path_env.to_string(),
            context_hash.to_string(),
            format!("idx:{index_revision}"),
        ],
    );
    if let Some(hit) = state.command_validation_cache.lock().await.get(&key) {
        return hit;
    }
    let mut exists = command_exists(token, session);
    if !exists
        && matches!(session.shell_kind, ShellKind::Zsh)
        && state.default_zsh_builtins.contains(token)
    {
        exists = true;
    }
    state
        .command_validation_cache
        .lock()
        .await
        .insert(key, exists);
    exists
}

fn command_exists(token: &str, session: &ShellSession) -> bool {
    if token.is_empty() {
        return false;
    }

    if token.contains('/') {
        let path = Path::new(token);
        if path.exists() {
            return true;
        }
    }

    let reserved = shell_reserved_words(&session.shell_kind);
    if reserved.contains(&token) {
        return true;
    }

    if let Some(ctx) = &session.context {
        if ctx.aliases.iter().any(|a| a.name == token) {
            return true;
        }
        if ctx.functions.iter().any(|f| f.name == token) {
            return true;
        }
        if ctx.builtins.iter().any(|b| b == token) {
            return true;
        }
    }

    let path_env = session
        .env_subset
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();

    for dir in path_env.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(token);
        if let Ok(meta) = std::fs::metadata(&candidate)
            && meta.is_file()
            && (meta.permissions().mode() & 0o111 != 0)
        {
            return true;
        }
    }

    false
}

fn shell_reserved_words(shell_kind: &ShellKind) -> &'static [&'static str] {
    match shell_kind {
        ShellKind::Zsh => zsh_reserved_words(),
        ShellKind::Bash | ShellKind::Fish | ShellKind::Other(_) => &[],
    }
}

fn zsh_reserved_words() -> &'static [&'static str] {
    &[
        "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
        "function", "select", "time", "coproc", "repeat", "in", "return", "break", "continue",
        "exit", "set", "unset", "export", "typeset", "local", "readonly", "true", "false", "cd",
        "pwd", "echo", "alias", "unalias", "history", "jobs", "fg", "bg", "wait", "exec",
    ]
}

fn required_capability_names() -> &'static [&'static str] {
    &[
        "prompt_mode",
        "session_mode",
        "single_key_approval",
        "edit_approval",
        "execute_in_real_shell",
        "command_completion_ack",
        "stdout_stderr_capture",
        "all_interactive_command_observation",
        "terminal_context_capture",
        "alias_capture",
        "function_capture",
        "builtin_inventory",
        "shell_native_history",
    ]
}

fn missing_required_capabilities(cap: &ShellCapabilities) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if !cap.prompt_mode {
        missing.push("prompt_mode");
    }
    if !cap.session_mode {
        missing.push("session_mode");
    }
    if !cap.single_key_approval {
        missing.push("single_key_approval");
    }
    if !cap.edit_approval {
        missing.push("edit_approval");
    }
    if !cap.execute_in_real_shell {
        missing.push("execute_in_real_shell");
    }
    if !cap.command_completion_ack {
        missing.push("command_completion_ack");
    }
    if !cap.stdout_stderr_capture {
        missing.push("stdout_stderr_capture");
    }
    if !cap.all_interactive_command_observation {
        missing.push("all_interactive_command_observation");
    }
    if !cap.terminal_context_capture {
        missing.push("terminal_context_capture");
    }
    if !cap.alias_capture {
        missing.push("alias_capture");
    }
    if !cap.function_capture {
        missing.push("function_capture");
    }
    if !cap.builtin_inventory {
        missing.push("builtin_inventory");
    }
    if !cap.shell_native_history {
        missing.push("shell_native_history");
    }
    missing
}

fn truncate_string(input: &str, max_chars: usize) -> String {
    let mut out = input.chars().take(max_chars).collect::<String>();
    if input.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn active_model_name(cfg: &AppConfig) -> String {
    if cfg.inference.provider == "ollama" {
        cfg.ollama.model.clone()
    } else {
        format!("gemma-4-{}", cfg.model.variant)
    }
}

fn provider_request_options(cfg: &AppConfig) -> BTreeMap<String, serde_json::Value> {
    if cfg.inference.provider != "ollama" {
        return BTreeMap::new();
    }

    cfg.ollama
        .options
        .iter()
        .filter_map(|(key, value)| match serde_json::to_value(value) {
            Ok(v) => Some((key.clone(), v)),
            Err(e) => {
                warn!("failed to serialize [ollama].options.{key}: {e}");
                None
            }
        })
        .collect()
}

fn tool_output_budget_tokens(cfg: &AppConfig, tool_name: &str) -> usize {
    match tool_name {
        "project_metadata" | "git_context" => cfg.context_budget.project_git_metadata_tokens,
        "web_search" | "web_read" => cfg.context_budget.web_result_tokens,
        "lookup_command_docs" => cfg.context_budget.docs_rag_tokens,
        _ => cfg.context_budget.local_tool_result_tokens,
    }
}

fn build_provider(cfg: &AppConfig) -> Result<ProviderRuntime> {
    match cfg.inference.provider.as_str() {
        "local" => {
            let model_file = if cfg.model.variant.eq_ignore_ascii_case("E2B") {
                cfg.model.e2b_filename.clone()
            } else {
                cfg.model.e4b_filename.clone()
            };
            let model_path = resolve_models_dir(&cfg.model.models_dir).join(model_file);
            Ok(ProviderRuntime::Local(LocalLlamaProvider::new(
                model_path.display().to_string(),
                cfg.model.context_tokens,
                cfg.model.gpu_layers,
                cfg.model.threads,
            )))
        }
        "ollama" => Ok(ProviderRuntime::Ollama(OllamaProvider::new(
            cfg.ollama.endpoint.clone(),
            cfg.ollama.model.clone(),
            cfg.ollama.allow_remote,
            cfg.ollama.allow_plain_http_remote,
            cfg.ollama.connect_timeout_secs,
            cfg.ollama.request_timeout_secs,
            cfg.ollama.keep_alive.clone(),
        )?)),
        other => bail!("unsupported inference provider: {other}"),
    }
}

fn peer_uid_matches(stream: &UnixStream) -> Result<bool> {
    let fd = stream.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: getsockopt(SO_PEERCRED) reads peer credentials for a connected Unix socket.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error()).context("getsockopt(SO_PEERCRED) failed");
        }
        // SAFETY: geteuid reads current effective uid.
        let self_uid = unsafe { libc::geteuid() };
        Ok(cred.uid == self_uid)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let mut euid: libc::uid_t = 0;
        let mut egid: libc::gid_t = 0;
        // SAFETY: getpeereid reads peer credentials for a connected Unix socket.
        let rc = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error()).context("getpeereid failed");
        }
        // SAFETY: geteuid reads current effective uid.
        let self_uid = unsafe { libc::geteuid() };
        Ok(euid == self_uid)
    }
}

fn init_logging(cfg: &AppConfig) {
    let fallback_filter = match cfg.daemon.log_level.to_ascii_lowercase().as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => {
            format!("termlm_core={}", cfg.daemon.log_level.to_ascii_lowercase())
        }
        _ => "termlm_core=info".to_string(),
    };
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| fallback_filter.into());

    let log_path = resolve_log_path(&cfg.daemon.log_file);
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Some(parent) = log_path.parent()
        && let Some(name) = log_path.file_name().and_then(|n| n.to_str())
    {
        let appender = tracing_appender::rolling::never(parent, name);
        let (non_blocking, guard) = tracing_appender::non_blocking(appender);
        if let Ok(mut slot) = LOG_GUARD.lock() {
            *slot = Some(guard);
        }
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(non_blocking)
            .try_init();
        return;
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}

fn resolve_log_path(raw: &str) -> PathBuf {
    let mut path = raw.to_string();
    if path.contains("$XDG_STATE_HOME") {
        let xdg_state = std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{home}/.local/state")
        });
        path = path.replace("$XDG_STATE_HOME", &xdg_state);
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

fn maybe_detach(detach: bool) -> Result<()> {
    if !detach {
        return Ok(());
    }
    #[cfg(unix)]
    {
        // SAFETY: standard double-fork daemonization pattern.
        unsafe {
            let pid = libc::fork();
            if pid < 0 {
                return Err(std::io::Error::last_os_error()).context("first fork failed");
            }
            if pid > 0 {
                libc::_exit(0);
            }
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error()).context("setsid failed");
            }
            let pid2 = libc::fork();
            if pid2 < 0 {
                return Err(std::io::Error::last_os_error()).context("second fork failed");
            }
            if pid2 > 0 {
                libc::_exit(0);
            }
        }
    }
    Ok(())
}

fn resolve_socket_path(config_path: &str) -> PathBuf {
    let xdg = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: geteuid reads current effective uid.
        let uid = unsafe { libc::geteuid() };
        format!("/tmp/termlm-{uid}")
    });

    if config_path.contains("$XDG_RUNTIME_DIR") {
        PathBuf::from(config_path.replace("$XDG_RUNTIME_DIR", &xdg))
    } else {
        PathBuf::from(config_path)
    }
}

fn resolve_runtime_path(config_path: &str) -> PathBuf {
    let xdg = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: geteuid reads current effective uid.
        let uid = unsafe { libc::geteuid() };
        format!("/tmp/termlm-{uid}")
    });
    if config_path.contains("$XDG_RUNTIME_DIR") {
        PathBuf::from(config_path.replace("$XDG_RUNTIME_DIR", &xdg))
    } else {
        PathBuf::from(config_path)
    }
}

fn prepare_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn cleanup_stale_socket(socket_path: &Path) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }

    match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(_) => anyhow::bail!("another termlm-core is already running"),
        Err(_) => {
            let _ = std::fs::remove_file(socket_path);
        }
    }
    Ok(())
}

fn write_pid_file(pid_path: &Path) -> Result<()> {
    std::fs::write(pid_path, format!("{}\n", std::process::id()))
        .with_context(|| format!("write {}", pid_path.display()))
}

fn ensure_single_daemon(pid_path: &Path, socket_path: &Path) -> Result<()> {
    if !pid_path.exists() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(pid_path).unwrap_or_default();
    let pid = raw.trim().parse::<i32>().unwrap_or(0);
    if pid <= 0 {
        let _ = std::fs::remove_file(pid_path);
        return Ok(());
    }

    // SAFETY: kill with signal 0 only probes process existence.
    let alive = unsafe { libc::kill(pid, 0) == 0 };
    if alive && socket_path.exists() {
        match std::os::unix::net::UnixStream::connect(socket_path) {
            Ok(_) => anyhow::bail!("another termlm-core is already running"),
            Err(_) => {
                let _ = std::fs::remove_file(socket_path);
            }
        }
    }

    let _ = std::fs::remove_file(pid_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("acquire env lock")
    }

    fn set_env_var(key: &str, value: Option<&str>) {
        if let Some(value) = value {
            // SAFETY: tests serialize process env edits behind `env_lock`.
            unsafe { std::env::set_var(key, value) };
        } else {
            // SAFETY: tests serialize process env edits behind `env_lock`.
            unsafe { std::env::remove_var(key) };
        }
    }

    struct EnvSnapshot {
        values: Vec<(String, Option<String>)>,
    }

    impl EnvSnapshot {
        fn capture(keys: &[&str]) -> Self {
            let values = keys
                .iter()
                .map(|key| (key.to_string(), std::env::var(key).ok()))
                .collect();
            Self { values }
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (key, value) in self.values.iter().rev() {
                if let Some(value) = value {
                    // SAFETY: tests serialize process env edits behind `env_lock`.
                    unsafe { std::env::set_var(key, value) };
                } else {
                    // SAFETY: tests serialize process env edits behind `env_lock`.
                    unsafe { std::env::remove_var(key) };
                }
            }
        }
    }

    #[test]
    fn changed_config_keys_reports_nested_paths() {
        let old = AppConfig::default();
        let mut new = old.clone();
        new.approval.mode = "auto".to_string();
        new.model.variant = "E2B".to_string();
        new.local_tools.max_search_results += 5;

        let mut changed = changed_config_keys(&old, &new).expect("changed keys");
        changed.sort();

        assert!(changed.iter().any(|k| k == "approval.mode"));
        assert!(changed.iter().any(|k| k == "model.variant"));
        assert!(
            changed
                .iter()
                .any(|k| k == "local_tools.max_search_results")
        );
    }

    #[test]
    fn cleanup_stale_socket_refuses_live_socket() {
        let root = std::env::temp_dir().join(format!(
            "termlm-live-sock-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let socket_path = root.join("termlm.sock");
        let _listener = StdUnixListener::bind(&socket_path).expect("bind socket");

        let err = cleanup_stale_socket(&socket_path).expect_err("must refuse live socket");
        assert!(err.to_string().contains("already running"));
        assert!(socket_path.exists());

        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ensure_single_daemon_refuses_alive_pid_with_live_socket() {
        let root = std::env::temp_dir().join(format!(
            "termlm-pid-guard-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let socket_path = root.join("termlm.sock");
        let pid_path = root.join("termlm.pid");
        let _listener = StdUnixListener::bind(&socket_path).expect("bind socket");
        std::fs::write(&pid_path, format!("{}\n", std::process::id())).expect("write pid");

        let err =
            ensure_single_daemon(&pid_path, &socket_path).expect_err("must refuse second daemon");
        assert!(err.to_string().contains("already running"));

        let _ = std::fs::remove_file(&pid_path);
        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preserve_restart_required_fields_defers_restart_keys() {
        let old = AppConfig::default();
        let mut next = old.clone();

        next.inference.provider = "ollama".to_string();
        next.model.variant = "E2B".to_string();
        next.ollama.endpoint = "http://10.0.0.2:11434".to_string();
        next.web.provider = "custom_json".to_string();
        next.approval.mode = "auto".to_string();

        preserve_restart_required_fields(&old, &mut next);

        assert_eq!(next.inference.provider, old.inference.provider);
        assert_eq!(next.model.variant, old.model.variant);
        assert_eq!(next.ollama.endpoint, old.ollama.endpoint);
        assert_eq!(next.web.provider, old.web.provider);
        assert_eq!(next.approval.mode, "auto");
    }

    #[tokio::test]
    async fn apply_runtime_config_updates_ollama_keep_alive() {
        let mut runtime = ProviderRuntime::Ollama(
            OllamaProvider::new(
                "http://127.0.0.1:11434",
                "gemma4:e4b",
                false,
                false,
                3,
                30,
                "5m",
            )
            .expect("construct ollama provider"),
        );
        let mut cfg = AppConfig::default();
        cfg.inference.provider = "ollama".to_string();
        cfg.ollama.keep_alive = "30m".to_string();
        runtime
            .apply_runtime_config(&cfg)
            .await
            .expect("apply runtime config");

        match &runtime {
            ProviderRuntime::Ollama(p) => {
                assert_eq!(p.keep_alive.as_deref(), Some("30m"));
            }
            ProviderRuntime::Local(_) => panic!("expected ollama runtime"),
        }

        let mut cfg_clear = cfg.clone();
        cfg_clear.ollama.keep_alive = String::new();
        runtime
            .apply_runtime_config(&cfg_clear)
            .await
            .expect("apply runtime config with empty keep_alive");
        match &runtime {
            ProviderRuntime::Ollama(p) => {
                assert!(p.keep_alive.is_none());
            }
            ProviderRuntime::Local(_) => panic!("expected ollama runtime"),
        }
    }

    #[test]
    fn startup_health_enforcement_respects_provider_and_ollama_switch() {
        let mut cfg = AppConfig::default();
        cfg.inference.provider = "local".to_string();
        assert!(should_enforce_startup_health(&cfg));

        cfg.inference.provider = "ollama".to_string();
        cfg.ollama.healthcheck_on_start = true;
        assert!(should_enforce_startup_health(&cfg));

        cfg.ollama.healthcheck_on_start = false;
        assert!(!should_enforce_startup_health(&cfg));
    }

    #[test]
    fn partial_tagged_execute_shell_command_is_extractable() {
        let raw = r#"prefix <|tool_call>call:execute_shell_command{cmd:<|"|>ls -1 | head -n 5<|"|>,intent:<|"|>List files<|"|>"#;
        let parsed = extract_execute_shell_command_from_partial_tagged_call(raw)
            .expect("extract partial execute_shell_command");
        assert_eq!(parsed.name, "execute_shell_command");
        assert_eq!(parsed.arguments["cmd"], "ls -1 | head -n 5");
    }

    #[test]
    fn partial_tagged_execute_shell_command_without_cmd_closer_is_extractable() {
        let raw = r#"prefix <|tool_call>call:execute_shell_command{cmd:<|"|>ls -1 | head -n 5"#;
        let parsed = extract_execute_shell_command_from_partial_tagged_call(raw)
            .expect("extract partial execute_shell_command without cmd closer");
        assert_eq!(parsed.name, "execute_shell_command");
        assert_eq!(parsed.arguments["cmd"], "ls -1 | head -n 5");
    }

    #[test]
    fn extraction_helpers_find_errors_paths_and_commands() {
        let errors = extract_error_lines(
            "ok\nfatal: could not read\npermission denied for /tmp/x\n",
            4,
        );
        assert!(errors.iter().any(|l| l.contains("fatal")));
        assert!(errors.iter().any(|l| l.contains("permission denied")));

        let paths = extract_paths("open ./src/main.rs and /tmp/log.txt", 4);
        assert!(paths.iter().any(|p| p == "./src/main.rs"));
        assert!(paths.iter().any(|p| p == "/tmp/log.txt"));

        let urls = extract_urls(
            "open https://example.com/docs?token=abc and http://x.test/",
            4,
        );
        assert!(
            urls.iter()
                .any(|u| u.starts_with("https://example.com/docs"))
        );
        assert!(urls.iter().any(|u| u == "http://x.test/"));

        let commands = extract_command_names("env A=1 ls -la | rg foo && git status", 8);
        assert!(commands.iter().any(|c| c == "ls"));
        assert!(commands.iter().any(|c| c == "rg"));
        assert!(commands.iter().any(|c| c == "git"));
    }

    #[test]
    fn normalize_observed_capture_status_defaults_to_skipped_interactive() {
        assert_eq!(
            normalize_observed_capture_status("captured", false),
            "captured"
        );
        assert_eq!(
            normalize_observed_capture_status("none", false),
            "skipped_interactive_tty"
        );
        assert_eq!(
            normalize_observed_capture_status("skipped_not_captured", false),
            "skipped_interactive_tty"
        );
        assert_eq!(
            normalize_observed_capture_status("anything_else", false),
            "skipped_interactive_tty"
        );
        assert_eq!(
            normalize_observed_capture_status("captured", true),
            "excluded_interactive"
        );
    }

    #[test]
    fn observed_from_ack_maps_capture_enabled_to_captured() {
        let cfg = AppConfig::default();
        let shell_id = Uuid::now_v7();
        let task_id = Uuid::now_v7();
        let ack = termlm_protocol::Ack {
            task_id,
            command_seq: 7,
            executed_command: "ls -la".to_string(),
            cwd_before: "/tmp".to_string(),
            cwd_after: "/tmp".to_string(),
            started_at: chrono::Utc::now(),
            exit_status: 0,
            stdout_b64: Some("aGVsbG8=".to_string()),
            stdout_truncated: false,
            stderr_b64: None,
            stderr_truncated: false,
            redactions_applied: Vec::new(),
            elapsed_ms: 25,
        };

        let observed = observed_from_ack(shell_id, &ack, &cfg);
        assert_eq!(observed.shell_id, shell_id);
        assert_eq!(observed.command_seq, 7);
        assert_eq!(observed.raw_command, "ls -la");
        assert_eq!(observed.expanded_command, "ls -la");
        assert_eq!(observed.output_capture_status, "captured");
        assert_eq!(observed.stdout_b64.as_deref(), Some("aGVsbG8="));
    }

    #[test]
    fn observed_from_ack_maps_capture_disabled_to_skipped_not_captured() {
        let mut cfg = AppConfig::default();
        cfg.capture.enabled = false;
        let shell_id = Uuid::now_v7();
        let task_id = Uuid::now_v7();
        let ack = termlm_protocol::Ack {
            task_id,
            command_seq: 1,
            executed_command: "pwd".to_string(),
            cwd_before: "/tmp".to_string(),
            cwd_after: "/tmp".to_string(),
            started_at: chrono::Utc::now(),
            exit_status: 0,
            stdout_b64: None,
            stdout_truncated: false,
            stderr_b64: None,
            stderr_truncated: false,
            redactions_applied: Vec::new(),
            elapsed_ms: 1,
        };

        let observed = observed_from_ack(shell_id, &ack, &cfg);
        assert_eq!(observed.output_capture_status, "skipped_not_captured");
    }

    #[test]
    fn head_tail_snippets_preserve_end_of_output() {
        let text = "line1\nline2\nline3\nline4";
        let (head, tail) = head_tail_snippets(text, 8);
        assert!(head.starts_with("line1"));
        assert!(tail.contains("line4"));
        assert!(tail.starts_with('…'));
    }

    #[test]
    fn parse_openai_embeddings_response() {
        let value = serde_json::json!({
            "data": [
                { "embedding": [1.0, 0.0, 0.0] },
                { "embedding": [0.0, 1.0, 0.0] }
            ]
        });
        let rows = parse_embeddings_response(value, 3).expect("parse embeddings");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 3);
        assert!((rows[0][0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parse_ollama_embeddings_response() {
        let value = serde_json::json!({
            "embeddings": [[2.0, 0.0], [0.0, 2.0]]
        });
        let rows = parse_embeddings_response(value, 2).expect("parse embeddings");
        assert_eq!(rows.len(), 2);
        assert!((rows[0][0] - 1.0).abs() < 1e-6);
        assert!((rows[1][1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_sha256_accepts_valid_hex_only() {
        assert!(normalize_sha256("a").is_none());
        assert!(normalize_sha256("zzzz").is_none());
        let valid = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(normalize_sha256(valid).as_deref(), Some(valid));
        assert_eq!(
            normalize_sha256(&valid.to_uppercase()).as_deref(),
            Some(valid)
        );
    }

    #[test]
    fn embedding_download_url_uses_builtin_default_mapping() {
        let _env_lock = env_lock();
        let _snapshot =
            EnvSnapshot::capture(&["TERMLM_EMBED_MODEL_URL", "TERMLM_EMBED_MODEL_BASE_URL"]);
        set_env_var("TERMLM_EMBED_MODEL_URL", None);
        set_env_var("TERMLM_EMBED_MODEL_BASE_URL", None);

        assert_eq!(
            resolve_embedding_download_url(DEFAULT_EMBED_FILENAME_BGE_Q4KM).as_deref(),
            Some(DEFAULT_EMBED_URL_BGE_Q4KM)
        );
        assert!(resolve_embedding_download_url("unknown.gguf").is_none());
    }

    #[test]
    fn embedding_sha256_uses_builtin_default_mapping() {
        let _env_lock = env_lock();
        let filename_key = "TERMLM_EMBED_MODEL_BGE_SMALL_EN_V1_5_Q4_K_M_GGUF_SHA256";
        let _snapshot = EnvSnapshot::capture(&["TERMLM_EMBED_MODEL_SHA256", filename_key]);
        set_env_var("TERMLM_EMBED_MODEL_SHA256", None);
        set_env_var(filename_key, None);

        assert_eq!(
            expected_embedding_sha256(DEFAULT_EMBED_FILENAME_BGE_Q4KM).as_deref(),
            Some(DEFAULT_EMBED_SHA256_BGE_Q4KM)
        );
        assert!(expected_embedding_sha256("unknown.gguf").is_none());
    }

    #[test]
    fn embedding_env_overrides_builtin_defaults() {
        let _env_lock = env_lock();
        let filename_key = "TERMLM_EMBED_MODEL_BGE_SMALL_EN_V1_5_Q4_K_M_GGUF_SHA256";
        let _snapshot = EnvSnapshot::capture(&[
            "TERMLM_EMBED_MODEL_URL",
            "TERMLM_EMBED_MODEL_BASE_URL",
            "TERMLM_EMBED_MODEL_SHA256",
            filename_key,
        ]);
        set_env_var("TERMLM_EMBED_MODEL_BASE_URL", None);
        set_env_var(
            "TERMLM_EMBED_MODEL_URL",
            Some("https://example.invalid/custom.gguf"),
        );
        set_env_var(
            "TERMLM_EMBED_MODEL_SHA256",
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        );
        set_env_var(filename_key, None);

        assert_eq!(
            resolve_embedding_download_url(DEFAULT_EMBED_FILENAME_BGE_Q4KM).as_deref(),
            Some("https://example.invalid/custom.gguf")
        );
        assert_eq!(
            expected_embedding_sha256(DEFAULT_EMBED_FILENAME_BGE_Q4KM).as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn model_download_url_uses_requested_filename() {
        let _env_lock = env_lock();
        let _snapshot = EnvSnapshot::capture(&[
            "TERMLM_MODEL_BASE_URL",
            "TERMLM_MODEL_E2B_URL",
            "TERMLM_MODEL_E4B_URL",
        ]);
        set_env_var("TERMLM_MODEL_BASE_URL", None);
        set_env_var("TERMLM_MODEL_E2B_URL", None);
        set_env_var("TERMLM_MODEL_E4B_URL", None);

        let url = resolve_model_download_url("E2B", "gemma-4-E2B-it-Q4_K_M.gguf")
            .expect("resolve e2b url");
        assert!(url.ends_with("/gemma-4-E2B-it-Q4_K_M.gguf"));
    }

    #[test]
    fn builtins_cache_uses_shell_specific_filename() {
        let path = builtins_cache_path(Path::new("/tmp/termlm-index-test"));
        assert!(path.ends_with("builtins.zsh.json"));
    }

    #[test]
    fn legacy_builtins_cache_is_read_and_migrated() {
        let root = std::env::temp_dir().join(format!(
            "termlm-builtins-legacy-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let legacy = legacy_builtins_cache_path(&root);
        std::fs::write(&legacy, "[\"alias\",\"typeset\"]").expect("write legacy cache");

        let loaded = load_or_extract_zsh_builtins(&root).expect("load builtins");
        assert!(loaded.contains("alias"));
        assert!(loaded.contains("typeset"));
        assert!(builtins_cache_path(&root).exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn model_asset_manifest_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "termlm-model-manifest-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let path = model_asset_manifest_path(&root);

        let mut manifest = ModelAssetManifest::default();
        let changed = record_model_asset(
            &mut manifest,
            "embedding_model",
            "bge.gguf",
            Some("https://example.invalid/bge.gguf"),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            42,
        );
        assert!(changed);
        write_model_asset_manifest_atomic(&path, &manifest).expect("write");
        let loaded = load_model_asset_manifest(&path).expect("load");
        assert_eq!(loaded.schema_version, 1);
        assert!(loaded.assets.contains_key("embedding_model:bge.gguf"));

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ensure_model_asset_records_existing_file() {
        let root = std::env::temp_dir().join(format!(
            "termlm-model-asset-existing-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let path = root.join("file.gguf");
        std::fs::write(&path, b"abc").expect("write asset");
        let (sha, _) = compute_file_sha256(&path).expect("sha");

        let mut manifest = ModelAssetManifest::default();
        let changed = ensure_model_asset(
            &mut manifest,
            EnsureModelAssetArgs {
                kind: "inference_model",
                filename: "file.gguf",
                path: &path,
                download_url: None,
                allow_download: false,
                required: true,
                expected_sha256: Some(sha.clone()),
                timeout: std::time::Duration::from_secs(1),
                user_agent: "test-agent",
            },
        )
        .await
        .expect("ensure model");
        assert!(changed);

        let changed_again = ensure_model_asset(
            &mut manifest,
            EnsureModelAssetArgs {
                kind: "inference_model",
                filename: "file.gguf",
                path: &path,
                download_url: None,
                allow_download: false,
                required: true,
                expected_sha256: Some(sha),
                timeout: std::time::Duration::from_secs(1),
                user_agent: "test-agent",
            },
        )
        .await
        .expect("ensure model second pass");
        assert!(!changed_again);

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ensure_model_asset_rejects_bad_checksum_when_required() {
        let root = std::env::temp_dir().join(format!(
            "termlm-model-asset-mismatch-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let path = root.join("file.gguf");
        std::fs::write(&path, b"abc").expect("write asset");

        let mut manifest = ModelAssetManifest::default();
        let err = ensure_model_asset(
            &mut manifest,
            EnsureModelAssetArgs {
                kind: "inference_model",
                filename: "file.gguf",
                path: &path,
                download_url: None,
                allow_download: false,
                required: true,
                expected_sha256: Some(
                    "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                ),
                timeout: std::time::Duration::from_secs(1),
                user_agent: "test-agent",
            },
        )
        .await
        .expect_err("must fail on checksum mismatch");
        assert!(err.to_string().contains("checksum mismatch"));

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn classifier_disable_forces_fresh_command_path() {
        let mut cfg = AppConfig::default();
        cfg.behavior.context_classifier_enabled = false;
        let classification = classify_prompt_for_task(&cfg, "latest npm release?");
        assert!(matches!(
            classification.classification,
            tasks::TaskClassification::FreshCommandRequest
        ));
        assert_eq!(classification.confidence, 1.0);
    }

    #[test]
    fn classifier_uses_configured_freshness_terms() {
        let mut cfg = AppConfig::default();
        cfg.web.freshness_required_terms = vec!["breaking changes".to_string()];
        let classification =
            classify_prompt_for_task(&cfg, "what are the breaking changes in rust stable?");
        assert!(matches!(
            classification.classification,
            tasks::TaskClassification::WebCurrentInfoQuestion
        ));
    }

    #[test]
    fn infer_search_freshness_maps_common_terms() {
        assert_eq!(
            infer_search_freshness("what changed today in rust?", &[]),
            Some("day".to_string())
        );
        assert_eq!(
            infer_search_freshness(
                "show breaking changes in the latest release",
                &[String::from("breaking changes"), String::from("latest")]
            ),
            Some("breaking changes".to_string())
        );
    }

    #[test]
    fn citation_helpers_extract_and_format_web_sources() {
        let now = chrono::Utc::now();
        let mut ledger = source_ledger::SourceLedger::default();
        ledger.push(source_ledger::SourceRef {
            source_type: "web_search_result".to_string(),
            source_id: "https://example.com/a".to_string(),
            hash: "h1".to_string(),
            redacted: false,
            truncated: false,
            observed_at: now,
            detail: None,
            section: None,
            offset_start: None,
            offset_end: None,
            extraction_method: None,
            extracted_at: None,
            index_version: None,
        });
        ledger.push(source_ledger::SourceRef {
            source_type: "web_read_page".to_string(),
            source_id: "https://example.com/b".to_string(),
            hash: "h2".to_string(),
            redacted: false,
            truncated: false,
            observed_at: now,
            detail: None,
            section: None,
            offset_start: None,
            offset_end: None,
            extraction_method: None,
            extracted_at: None,
            index_version: None,
        });
        ledger.push(source_ledger::SourceRef {
            source_type: "local_file_read".to_string(),
            source_id: "/tmp/x".to_string(),
            hash: "h3".to_string(),
            redacted: true,
            truncated: false,
            observed_at: now,
            detail: None,
            section: None,
            offset_start: None,
            offset_end: None,
            extraction_method: None,
            extracted_at: None,
            index_version: None,
        });
        let urls = citation_urls_from_ledger(&ledger);
        assert_eq!(urls.len(), 2);
        assert!(urls.iter().any(|u| u == "https://example.com/a"));
        assert!(urls.iter().any(|u| u == "https://example.com/b"));

        let block = build_citation_block(&urls);
        assert!(block.contains("## Citations"));
        assert!(block.contains("[1] https://"));
        assert!(has_citation_block(&block));
    }

    #[test]
    fn indexing_progress_banner_includes_lookup_hint() {
        let progress = IndexProgress {
            scanned: 12,
            total: 100,
            percent: 12.0,
            phase: "scan".to_string(),
        };
        let msg = indexing_progress_banner(&progress).expect("banner");
        assert!(msg.contains("lookup_command_docs(name)"));
        assert!(msg.contains("12 commands available so far"));
    }

    #[test]
    fn prune_stale_loaded_index_removes_missing_paths_before_runtime_publish() {
        let root = std::env::temp_dir().join(format!(
            "termlm-prune-stale-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let keep_path = root.join("keep-cmd");
        std::fs::write(&keep_path, b"#!/bin/sh\necho ok\n").expect("write keep");

        let missing_path = root.join("missing-cmd");
        let entries = vec![
            termlm_indexer::BinaryEntry {
                name: "keep".to_string(),
                path: keep_path.clone(),
                mtime_secs: 1,
                size: 1,
                inode: 1,
            },
            termlm_indexer::BinaryEntry {
                name: "missing".to_string(),
                path: missing_path.clone(),
                mtime_secs: 1,
                size: 1,
                inode: 1,
            },
        ];
        let now = chrono::Utc::now();
        let chunks = vec![
            Chunk {
                command_name: "keep".to_string(),
                path: keep_path.to_string_lossy().to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                doc_hash: "a".to_string(),
                extracted_at: now,
                text: "keep".to_string(),
            },
            Chunk {
                command_name: "missing".to_string(),
                path: missing_path.to_string_lossy().to_string(),
                extraction_method: "man".to_string(),
                section_name: "NAME".to_string(),
                chunk_index: 0,
                total_chunks: 1,
                doc_hash: "b".to_string(),
                extracted_at: now,
                text: "missing".to_string(),
            },
        ];

        let (kept_entries, kept_chunks, stale_removed) = prune_stale_loaded_index(entries, chunks);
        assert_eq!(stale_removed, 1);
        assert_eq!(kept_entries.len(), 1);
        assert_eq!(kept_chunks.len(), 1);
        assert_eq!(kept_entries[0].name, "keep");
        assert_eq!(kept_chunks[0].command_name, "keep");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn effective_web_cache_settings_honor_global_cache_switches() {
        let mut cfg = AppConfig::default();
        cfg.web.cache_max_bytes = 80 * 1024 * 1024;
        cfg.cache.max_total_cache_bytes = 40 * 1024 * 1024;
        let capped = effective_web_cache_bytes(&cfg);
        assert_eq!(capped, 40 * 1024 * 1024);

        cfg.cache.enabled = false;
        let disabled = effective_web_cache_bytes(&cfg);
        assert_eq!(disabled, 32 * 1024);
        let ttls = effective_web_cache_ttls(&cfg);
        assert_eq!(ttls, (0, 0));
    }

    #[test]
    fn read_file_cache_key_changes_when_file_changes() {
        let root = std::env::temp_dir().join(format!(
            "termlm-read-cache-key-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let path = root.join("notes.txt");
        std::fs::write(&path, "one\n").expect("write");

        let key1 = file_read_cache_key(
            &path,
            1,
            0,
            8192,
            &termlm_local_tools::TextDetectionOptions::default(),
        )
        .expect("key1");
        std::fs::write(&path, "two-two\n").expect("rewrite");
        let key2 = file_read_cache_key(
            &path,
            1,
            0,
            8192,
            &termlm_local_tools::TextDetectionOptions::default(),
        )
        .expect("key2");
        assert_ne!(key1, key2);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn project_metadata_cache_key_changes_when_manifest_changes() {
        let root = std::env::temp_dir().join(format!(
            "termlm-meta-cache-key-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let package_json = root.join("package.json");
        std::fs::write(&package_json, r#"{"scripts":{"test":"vitest"}}"#).expect("write");

        let key1 = project_metadata_cache_key(&root, true, true, 50, 65_536, true);
        std::fs::write(
            &package_json,
            r#"{"scripts":{"test":"vitest","build":"vite build"}}"#,
        )
        .expect("rewrite");
        let key2 = project_metadata_cache_key(&root, true, true, 50, 65_536, true);
        assert_ne!(key1, key2);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn git_context_cache_key_changes_when_status_changes() {
        let root = std::env::temp_dir().join(format!(
            "termlm-git-cache-key-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .status()
                .expect("git command");
            assert!(status.success(), "git command failed: {:?}", args);
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "termlm@test.local"]);
        run(&["config", "user.name", "termlm-test"]);
        std::fs::write(root.join("a.txt"), "one\n").expect("write");
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);

        let key1 = git_context_cache_key(&root, true, 200, 10, 12_000);
        std::fs::write(root.join("a.txt"), "changed\n").expect("rewrite");
        let key2 = git_context_cache_key(&root, true, 200, 10, 12_000);
        assert_ne!(key1, key2);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn docs_retrieval_cache_key_changes_with_index_revision_and_semantics() {
        let base = DocsRetrievalCacheSemantics {
            rag_top_k: 8,
            rag_min_similarity: 0.35,
            hybrid_retrieval_enabled: true,
            lexical_index_enabled: true,
            lexical_top_k: 50,
            command_aware_retrieval: true,
            command_aware_top_k: 8,
            index_revision: 42,
        };
        let k1 = docs_retrieval_cache_key("query text", "git", base);
        let k2 = docs_retrieval_cache_key(
            "query text",
            "git",
            DocsRetrievalCacheSemantics {
                index_revision: 43,
                ..base
            },
        );
        let k3 = docs_retrieval_cache_key(
            "query text",
            "git",
            DocsRetrievalCacheSemantics {
                rag_top_k: 16,
                ..base
            },
        );
        let k4 = docs_retrieval_cache_key(
            "query text",
            "git",
            DocsRetrievalCacheSemantics {
                rag_min_similarity: 0.42,
                ..base
            },
        );
        let k5 = docs_retrieval_cache_key(
            "query text",
            "git",
            DocsRetrievalCacheSemantics {
                hybrid_retrieval_enabled: false,
                ..base
            },
        );
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_ne!(k1, k4);
        assert_ne!(k1, k5);
    }

    #[test]
    fn web_search_cache_key_changes_with_provider_and_freshness() {
        let req = SearchRequest {
            query: "rust release notes".to_string(),
            freshness: Some("latest".to_string()),
            max_results: 6,
        };
        let key1 = web_search_cache_key("duckduckgo_html:", &req);
        let key2 =
            web_search_cache_key("brave:https://api.search.brave.com/res/v1/web/search", &req);
        let key3 = web_search_cache_key(
            "duckduckgo_html:",
            &SearchRequest {
                freshness: Some("current".to_string()),
                ..req.clone()
            },
        );
        assert_ne!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn web_read_cache_key_changes_with_extraction_and_bounds() {
        let req = WebReadRequest {
            url: "https://example.com/docs?a=1".to_string(),
            max_bytes: 2 * 1024 * 1024,
            allow_plain_http: false,
            allow_local_addresses: false,
            user_agent: "termlm-test".to_string(),
            obey_robots_txt: true,
            min_delay_between_requests_ms: 1200,
            robots_cache_ttl_secs: 900,
            extract_strategy: "auto".to_string(),
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
        };
        let key1 = web_read_cache_key(&req);
        let mut changed = req.clone();
        changed.max_markdown_bytes = 32_768;
        let key2 = web_read_cache_key(&changed);
        changed = req.clone();
        changed.extract_strategy = "readability".to_string();
        let key3 = web_read_cache_key(&changed);
        changed = req.clone();
        changed.strip_tracking_params = false;
        let key4 = web_read_cache_key(&changed);
        assert_ne!(key1, key2);
        assert_ne!(key1, key3);
        assert_ne!(key1, key4);
    }

    #[test]
    fn web_search_provider_cache_id_changes_with_endpoint() {
        let mut cfg = WebRuntimeConfig {
            provider: "custom_json".to_string(),
            search_endpoint: "https://one.example/search".to_string(),
            ..WebRuntimeConfig::default()
        };
        let id1 = web_search_provider_cache_id(&cfg);
        cfg.search_endpoint = "https://two.example/search".to_string();
        let id2 = web_search_provider_cache_id(&cfg);
        assert_ne!(id1, id2);
    }

    #[test]
    fn performance_profile_caps_indexer_concurrency() {
        let mut cfg = AppConfig::default();
        cfg.indexer.concurrency = 32;
        cfg.performance.max_background_cpu_pct = 100;

        cfg.performance.profile = "eco".to_string();
        let eco = effective_indexer_concurrency(&cfg);
        assert!(eco <= 2);

        cfg.performance.profile = "balanced".to_string();
        let balanced = effective_indexer_concurrency(&cfg);
        assert!(balanced <= 4);
    }

    #[test]
    fn parse_zsh_builtins_from_man_extracts_candidates() {
        let sample = r#"
       alias [ -gmrL ] [ name[=value] ... ]
       unalias [ -ams ] name ...
       typeset [ +AHhilmnrtux ] [ name[=value] ... ]

       This paragraph should not become a builtin entry.
        "#;
        let parsed = parse_zsh_builtins_from_man(sample);
        assert!(parsed.contains("alias"));
        assert!(parsed.contains("unalias"));
        assert!(parsed.contains("typeset"));
        assert!(!parsed.contains("this"));
    }

    #[test]
    fn local_tool_schemas_for_profile_respect_file_and_terminal_flags() {
        let cfg = AppConfig::default();
        let profile = context::ToolExposureProfile {
            execute_shell_command: true,
            lookup_command_docs: true,
            local_file_tools: true,
            terminal_context_tool: false,
            web_tools: false,
        };
        let file_only = local_tool_schemas_for_profile(&cfg, &profile);
        assert!(file_only.iter().any(|tool| tool.name == "read_file"));
        assert!(
            !file_only
                .iter()
                .any(|tool| tool.name == "search_terminal_context")
        );

        let profile_terminal = context::ToolExposureProfile {
            execute_shell_command: true,
            lookup_command_docs: true,
            local_file_tools: false,
            terminal_context_tool: true,
            web_tools: false,
        };
        let terminal_only = local_tool_schemas_for_profile(&cfg, &profile_terminal);
        assert!(
            terminal_only
                .iter()
                .any(|tool| tool.name == "search_terminal_context")
        );
        assert!(!terminal_only.iter().any(|tool| tool.name == "read_file"));
    }

    #[test]
    fn trim_session_conversation_drops_oldest_turns_and_keeps_system_prompt() {
        let mut conv = SessionConversation {
            system_prompt: "S".repeat(20),
            turns: VecDeque::from(vec![
                SessionTurn {
                    user: "A".repeat(20),
                    assistant: "a".repeat(20),
                },
                SessionTurn {
                    user: "B".repeat(20),
                    assistant: "b".repeat(20),
                },
                SessionTurn {
                    user: "C".repeat(20),
                    assistant: "c".repeat(20),
                },
            ]),
        };

        trim_session_conversation(&mut conv, 24);

        assert_eq!(conv.system_prompt, "S".repeat(20));
        assert_eq!(conv.turns.len(), 1);
        assert_eq!(conv.turns[0].user, "C".repeat(20));
        assert_eq!(conv.turns[0].assistant, "c".repeat(20));
    }

    #[test]
    fn session_turn_lines_include_user_and_assistant_labels() {
        let conv = SessionConversation {
            system_prompt: String::new(),
            turns: VecDeque::from(vec![SessionTurn {
                user: "show git status".to_string(),
                assistant: "Run `git status`.".to_string(),
            }]),
        };
        let lines = session_turn_lines(&conv);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("User: show git status"));
        assert!(lines[0].contains("Assistant: Run `git status`."));
    }

    #[test]
    fn session_turn_messages_preserve_turn_order() {
        let conv = SessionConversation {
            system_prompt: String::new(),
            turns: VecDeque::from(vec![
                SessionTurn {
                    user: "u1".to_string(),
                    assistant: "a1".to_string(),
                },
                SessionTurn {
                    user: "u2".to_string(),
                    assistant: "a2".to_string(),
                },
            ]),
        };
        let msgs = session_turn_messages(&conv);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "u1");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content, "a1");
        assert_eq!(msgs[2].role, "user");
        assert_eq!(msgs[2].content, "u2");
        assert_eq!(msgs[3].role, "assistant");
        assert_eq!(msgs[3].content, "a2");
    }

    #[test]
    fn summarize_validation_findings_joins_kind_and_detail() {
        let findings = vec![
            planning::ValidationFinding {
                kind: "unknown_command".to_string(),
                detail: "foo not found".to_string(),
            },
            planning::ValidationFinding {
                kind: "unsupported_flag".to_string(),
                detail: "--bad not in docs".to_string(),
            },
        ];
        let summary = summarize_validation_findings(&findings);
        assert!(summary.contains("unknown_command: foo not found"));
        assert!(summary.contains("unsupported_flag: --bad not in docs"));
    }

    #[test]
    fn validation_findings_tool_response_uses_unknown_command_shape() {
        let findings = vec![planning::ValidationFinding {
            kind: "unknown_command".to_string(),
            detail: "first token missing".to_string(),
        }];
        let payload = validation_findings_tool_response(
            "fakecmd --help",
            "fakecmd",
            &findings,
            &["find".to_string(), "fd".to_string()],
            2,
            3,
        );
        let value: serde_json::Value = serde_json::from_str(&payload).expect("json");
        assert_eq!(
            value.get("error").and_then(|v| v.as_str()),
            Some("unknown_command")
        );
        assert_eq!(
            value.get("command").and_then(|v| v.as_str()),
            Some("fakecmd")
        );
        let suggestions = value
            .get("suggestions")
            .and_then(|v| v.as_array())
            .expect("suggestions array");
        assert_eq!(suggestions.len(), 2);
    }

    #[test]
    fn extract_command_aware_features_collects_flags_paths_and_risk_markers() {
        let features = extract_command_aware_features(
            "find",
            "find ./logs -name '*.log' -mtime +7 -delete | sort",
        );
        assert!(features.flags.iter().any(|f| f == "-name"));
        assert!(features.flags.iter().any(|f| f == "-mtime"));
        assert!(features.flags.iter().any(|f| f == "-delete"));
        assert!(features.paths.iter().any(|p| p == "./logs"));
        assert!(features.risk_markers.iter().any(|m| m == "pipeline"));
    }

    #[test]
    fn command_aware_query_includes_structured_sections() {
        let query = build_command_aware_retrieval_query(
            "remove week-old logs",
            "find",
            "find . -name '*.log' -mtime +7 -delete",
        );
        assert!(query.contains("command: find"));
        assert!(query.contains("draft_command: find . -name"));
        assert!(query.contains("flags:"));
    }
}
