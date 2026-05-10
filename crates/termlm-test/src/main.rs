use anyhow::{Context, Result, bail};
use base64::Engine;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use regex::Regex;
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child as StdChild, Command as StdCommand, Stdio};
use std::time::Instant;
use termlm_config::AppConfig;
use termlm_indexer::{Chunk, HybridRetriever, RetrievalQuery};
use termlm_protocol::{
    Ack, AliasDef, ClientMessage, ErrorKind, FunctionDef, MAX_FRAME_BYTES, RegisterShell,
    ReindexMode, RetrieveRequest, ServerMessage, ShellCapabilities, ShellContext, ShellKind,
    StartTask, UserDecision, UserResponse,
};
use termlm_safety::matches_safety_floor;
use termlm_test::{SuiteConfig, TestCase, load_suite};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio_serde::formats::Json;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use uuid::Uuid;

type ClientTransport = tokio_serde::Framed<
    Framed<UnixStream, LengthDelimitedCodec>,
    ServerMessage,
    ClientMessage,
    Json<ServerMessage, ClientMessage>,
>;

#[derive(Debug, Clone, ValueEnum)]
enum HarnessMode {
    Retrieval,
    E2e,
    Safety,
    All,
    LocalIntegration,
    OllamaIntegration,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum HarnessProvider {
    Local,
    Ollama,
}

#[derive(Debug, Parser)]
#[command(name = "termlm-test")]
#[command(about = "termlm validation harness")]
struct Cli {
    #[arg(long, default_value = "tests/fixtures/termlm-test-suite.toml")]
    suite: PathBuf,
    #[arg(long, value_enum, default_value = "all")]
    mode: HarnessMode,
    #[arg(long, default_value_t = 5)]
    top_k: u32,
    #[arg(long)]
    timeout_secs: Option<u64>,
    #[arg(long, default_value_t = false)]
    keep_sandbox: bool,
    #[arg(long)]
    perf_gates: Option<PathBuf>,
    #[arg(long)]
    results_out: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "local")]
    provider: HarnessProvider,
}

#[derive(Debug, serde::Serialize)]
struct RetrievalScore {
    top_k: u32,
    hit: bool,
    best_rank: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
struct TestReport {
    id: String,
    mode: String,
    category: String,
    passed: bool,
    duration_ms: u64,
    retrieval_score: Option<RetrievalScore>,
    retrieval_latency_ms: Option<u64>,
    retrieval_50k_latency_ms: Option<u64>,
    retrieval_50k_lexical_ms: Option<u64>,
    task_latency_ms: Option<u64>,
    ttft_ms: Option<u64>,
    throughput_toks_per_sec: Option<f64>,
    throughput_source: Option<String>,
    model_load_ms: Option<u64>,
    model_resident_mb: Option<u64>,
    indexer_resident_mb: Option<u64>,
    orchestration_resident_mb: Option<u64>,
    last_task_prompt_tokens: Option<u64>,
    last_task_completion_tokens: Option<u64>,
    last_task_usage_reported: Option<bool>,
    embedding_chunks_per_sec: Option<f64>,
    full_reindex_ms: Option<u64>,
    delta_reindex_ms: Option<u64>,
    index_disk_mb: Option<u64>,
    ollama_orchestration_overhead_ms: Option<f64>,
    observed_command_overhead_ms: Option<f64>,
    observed_command_capture_overhead_ms: Option<f64>,
    idle_cpu_pct: Option<f64>,
    source_ledger_ref_count: Option<u64>,
    source_ledger_overhead_ms: Option<u64>,
    tool_routing_overhead_ms: Option<u64>,
    pre_provider_overhead_ms: Option<u64>,
    planning_loop_overhead_ms: Option<u64>,
    web_extract_latency_ms: Option<u64>,
    web_extract_latency_p95_ms: Option<u64>,
    web_extract_rss_delta_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    stage_timings_ms: BTreeMap<String, u64>,
    rss_mb: Option<u64>,
    kv_cache_mb: Option<u64>,
    proposed_command: Option<String>,
    exit_status: Option<i32>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct PerfGateThreshold {
    #[serde(default)]
    p50_ms: Option<u64>,
    #[serde(default)]
    p95_ms: Option<u64>,
    #[serde(default)]
    max_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct PerfGateFloatThreshold {
    #[serde(default)]
    p50_min: Option<f64>,
    #[serde(default)]
    p95_min: Option<f64>,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    p50_max: Option<f64>,
    #[serde(default)]
    p95_max: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, Default)]
struct PerfGateHardwareProfile {
    #[serde(default)]
    ttft_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    model_load_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    throughput_toks_per_sec: Option<PerfGateFloatThreshold>,
    #[serde(default)]
    embedding_chunks_per_sec: Option<PerfGateFloatThreshold>,
    #[serde(default)]
    observed_command_overhead_ms: Option<PerfGateFloatThreshold>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, Default)]
struct PerfGateHardwareProfiles {
    #[serde(default)]
    apple_m2_pro_max_local: Option<PerfGateHardwareProfile>,
    #[serde(default)]
    apple_m3_pro_local: Option<PerfGateHardwareProfile>,
    #[serde(default)]
    apple_m3_max_local: Option<PerfGateHardwareProfile>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct PerfGateConfig {
    #[serde(default)]
    retrieval_latency_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    retrieval_50k_latency_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    retrieval_50k_lexical_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    task_latency_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    ttft_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    model_load_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    model_resident_mb: Option<PerfGateThreshold>,
    #[serde(default)]
    rss_mb: Option<PerfGateThreshold>,
    #[serde(default)]
    indexer_resident_mb: Option<PerfGateThreshold>,
    #[serde(default)]
    orchestration_resident_mb: Option<PerfGateThreshold>,
    #[serde(default)]
    kv_cache_mb: Option<PerfGateThreshold>,
    #[serde(default)]
    full_reindex_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    delta_reindex_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    index_disk_mb: Option<PerfGateThreshold>,
    #[serde(default)]
    source_ledger_ref_count: Option<PerfGateThreshold>,
    #[serde(default)]
    source_ledger_overhead_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    tool_routing_overhead_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    pre_provider_overhead_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    planning_loop_overhead_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    web_extract_latency_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    web_extract_latency_p95_ms: Option<PerfGateThreshold>,
    #[serde(default)]
    web_extract_rss_delta_mb: Option<PerfGateThreshold>,
    #[serde(default)]
    throughput_toks_per_sec: Option<PerfGateFloatThreshold>,
    #[serde(default)]
    embedding_chunks_per_sec: Option<PerfGateFloatThreshold>,
    #[serde(default)]
    ollama_orchestration_overhead_ms: Option<PerfGateFloatThreshold>,
    #[serde(default)]
    observed_command_overhead_ms: Option<PerfGateFloatThreshold>,
    #[serde(default)]
    observed_command_capture_overhead_ms: Option<PerfGateFloatThreshold>,
    #[serde(default)]
    idle_cpu_pct: Option<PerfGateFloatThreshold>,
    #[serde(default)]
    stage_timings_ms: BTreeMap<String, PerfGateThreshold>,
    #[serde(default)]
    hardware_profiles: PerfGateHardwareProfiles,
}

#[derive(Debug, Clone, Copy)]
struct LatencyStats {
    p50_ms: u64,
    p95_ms: u64,
    max_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct FloatStats {
    p50: f64,
    p95: f64,
    min: f64,
    max: f64,
}

#[derive(Debug, serde::Serialize, Default)]
struct CategorySummary {
    total: usize,
    passed: usize,
}

#[derive(Debug, serde::Serialize)]
struct Summary {
    total: usize,
    passed: usize,
    failed: usize,
    by_category: BTreeMap<String, CategorySummary>,
    retrieval_hit_rate_top1: f64,
    retrieval_hit_rate_top5: f64,
}

#[derive(Debug, serde::Serialize)]
struct HarnessResults {
    suite_version: String,
    embedding_model: String,
    benchmark_environment: BenchmarkEnvironment,
    started_at: chrono::DateTime<chrono::Utc>,
    duration_secs: u64,
    tests: Vec<TestReport>,
    summary: Summary,
}

#[derive(Debug, serde::Serialize)]
struct BenchmarkEnvironment {
    os: String,
    arch: String,
    cpu: String,
    hardware_class: String,
    logical_cpus: usize,
    total_memory_mb: Option<u64>,
    provider: String,
    model: String,
    performance_profile: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HardwareClass {
    AppleM2ProMaxLocal,
    AppleM3ProLocal,
    AppleM3MaxLocal,
    Other,
}

impl HardwareClass {
    fn as_str(self) -> &'static str {
        match self {
            HardwareClass::AppleM2ProMaxLocal => "apple_m2_pro_max_local",
            HardwareClass::AppleM3ProLocal => "apple_m3_pro_local",
            HardwareClass::AppleM3MaxLocal => "apple_m3_max_local",
            HardwareClass::Other => "other",
        }
    }
}

#[derive(Debug, Default)]
struct TaskRunOutput {
    proposed_command: Option<String>,
    stdout: String,
    stderr: String,
    exit_status: Option<i32>,
    saw_needs_clarification: bool,
    saw_safety_floor: bool,
    saw_unknown_command: bool,
    saw_validation_incomplete: bool,
    ttft_ms: Option<u64>,
    stream_window_secs: Option<f64>,
    throughput_toks_per_sec_heuristic: Option<f64>,
    trace: String,
}

#[derive(Debug, Clone)]
struct HarnessDaemonPaths {
    config_path: PathBuf,
    socket_path: PathBuf,
    runtime_dir: PathBuf,
    home_dir: PathBuf,
    index_root: PathBuf,
}

#[derive(Debug, Default, serde::Deserialize)]
struct TerminalObserverBenchmarkOutput {
    observed_command_overhead_ms: f64,
    observed_command_capture_overhead_ms: f64,
}

#[derive(Debug, Default)]
struct IndexBenchmarkOutput {
    embedding_chunks_per_sec: Option<f64>,
    full_reindex_ms: Option<u64>,
    delta_reindex_ms: Option<u64>,
    index_disk_mb: Option<u64>,
}

#[derive(Debug, Default)]
struct Retrieval50kBenchmarkOutput {
    hybrid_latency_ms: Option<u64>,
    lexical_latency_ms: Option<u64>,
}

#[derive(Debug, Default)]
struct WebExtractBenchmarkOutput {
    latency_p50_ms: Option<u64>,
    latency_p95_ms: Option<u64>,
    rss_delta_mb: Option<u64>,
}

#[derive(Debug)]
struct OllamaIntegrationRuntime {
    root_dir: PathBuf,
    endpoint: String,
    model: String,
    child: Option<StdChild>,
}

impl Drop for OllamaIntegrationRuntime {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root_dir);
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let local_integration_mode = matches!(cli.mode, HarnessMode::LocalIntegration);
    let ollama_integration_mode = matches!(cli.mode, HarnessMode::OllamaIntegration);
    if ollama_integration_mode
        && std::env::var("TERMLM_TEST_OLLAMA")
            .map(|v| v != "1")
            .unwrap_or(true)
    {
        println!(
            "termlm-test: skipping ollama-integration mode (set TERMLM_TEST_OLLAMA=1 to enable)"
        );
        return Ok(());
    }
    let ollama_runtime = if ollama_integration_mode {
        prepare_ollama_integration_runtime()?
    } else {
        None
    };
    if ollama_integration_mode && ollama_runtime.is_none() {
        return Ok(());
    }

    let started_at = chrono::Utc::now();
    let run_started = Instant::now();
    let suite = load_suite(&cli.suite).with_context(|| format!("load {}", cli.suite.display()))?;

    let sandbox_root = resolve_sandbox_root(&suite.suite.sandbox_root_template)?;
    validate_sandbox_root(&sandbox_root)?;
    validate_suite(&suite, &sandbox_root)?;
    tokio::fs::create_dir_all(&sandbox_root)
        .await
        .with_context(|| format!("create {}", sandbox_root.display()))?;

    let harness_provider = if ollama_integration_mode {
        HarnessProvider::Ollama
    } else {
        cli.provider
    };
    let runtime_real = std::env::var("TERMLM_E2E_REAL")
        .map(|v| v == "1")
        .unwrap_or(false)
        || local_integration_mode;
    let daemon_paths = write_harness_daemon_config(
        harness_provider,
        ollama_integration_mode,
        ollama_runtime.as_ref(),
    )?;
    let timeout_secs = cli
        .timeout_secs
        .unwrap_or(suite.suite.default_timeout_secs)
        .max(1);
    let timeout_secs = if local_integration_mode {
        timeout_secs.max(120)
    } else {
        timeout_secs
    };
    let mut daemon_child =
        spawn_daemon(&daemon_paths, &sandbox_root, harness_provider, runtime_real)?;
    let daemon_boot_timeout_secs =
        daemon_boot_timeout_secs(runtime_real, ollama_integration_mode).max(1);
    wait_for_socket_with_child(
        &daemon_paths.socket_path,
        &mut daemon_child,
        std::time::Duration::from_secs(daemon_boot_timeout_secs),
    )
    .await?;
    let mut transport = connect_transport(&daemon_paths.socket_path).await?;

    let shell_id = register_shell(&mut transport).await?;
    send_shell_context(&mut transport, shell_id, &suite).await?;
    kick_delta_reindex_best_effort(&mut transport).await;

    let selected = if ollama_integration_mode {
        ollama_integration_cases()
    } else if local_integration_mode {
        local_integration_cases()
    } else {
        suite
            .test
            .iter()
            .filter(|t| test_selected(t, &cli.mode))
            .cloned()
            .collect::<Vec<_>>()
    };

    let mut reports = Vec::<TestReport>::new();
    for test in selected {
        let started = Instant::now();
        let test_dir = sandbox_root.join(&test.id);
        tokio::fs::create_dir_all(&test_dir)
            .await
            .with_context(|| format!("create {}", test_dir.display()))?;

        let report = run_one_test(
            &mut transport,
            shell_id,
            &test,
            &test_dir,
            &cli.mode,
            cli.top_k,
            timeout_secs,
        )
        .await
        .unwrap_or_else(|e| TestReport {
            id: test.id.clone(),
            mode: test.mode.clone(),
            category: test.category.clone(),
            passed: false,
            duration_ms: started.elapsed().as_millis() as u64,
            retrieval_score: None,
            retrieval_latency_ms: None,
            retrieval_50k_latency_ms: None,
            retrieval_50k_lexical_ms: None,
            task_latency_ms: None,
            ttft_ms: None,
            throughput_toks_per_sec: None,
            throughput_source: None,
            model_load_ms: None,
            model_resident_mb: None,
            indexer_resident_mb: None,
            orchestration_resident_mb: None,
            last_task_prompt_tokens: None,
            last_task_completion_tokens: None,
            last_task_usage_reported: None,
            embedding_chunks_per_sec: None,
            full_reindex_ms: None,
            delta_reindex_ms: None,
            index_disk_mb: None,
            ollama_orchestration_overhead_ms: None,
            observed_command_overhead_ms: None,
            observed_command_capture_overhead_ms: None,
            idle_cpu_pct: None,
            source_ledger_ref_count: None,
            source_ledger_overhead_ms: None,
            tool_routing_overhead_ms: None,
            pre_provider_overhead_ms: None,
            planning_loop_overhead_ms: None,
            web_extract_latency_ms: None,
            web_extract_latency_p95_ms: None,
            web_extract_rss_delta_mb: None,
            stage_timings_ms: BTreeMap::new(),
            rss_mb: None,
            kv_cache_mb: None,
            proposed_command: None,
            exit_status: None,
            error: Some(e.to_string()),
        });

        reports.push(report);
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    let integration_mode = matches!(
        cli.mode,
        HarnessMode::LocalIntegration | HarnessMode::OllamaIntegration
    );
    if !integration_mode {
        let index_benchmarks =
            benchmark_index_metrics(&mut transport, &daemon_paths.index_root).await?;
        if let Some(chunks_per_sec) = index_benchmarks.embedding_chunks_per_sec {
            for report in &mut reports {
                report.embedding_chunks_per_sec = Some(chunks_per_sec);
            }
        }
        if let Some(full_reindex_ms) = index_benchmarks.full_reindex_ms {
            for report in &mut reports {
                report.full_reindex_ms = Some(full_reindex_ms);
            }
        }
        if let Some(delta_reindex_ms) = index_benchmarks.delta_reindex_ms {
            for report in &mut reports {
                report.delta_reindex_ms = Some(delta_reindex_ms);
            }
        }
        if let Some(index_disk_mb) = index_benchmarks.index_disk_mb {
            for report in &mut reports {
                report.index_disk_mb = Some(index_disk_mb);
            }
        }

        let retrieval_50k = benchmark_retrieval_50k_metrics();
        for report in &mut reports {
            report.retrieval_50k_latency_ms = retrieval_50k.hybrid_latency_ms;
            report.retrieval_50k_lexical_ms = retrieval_50k.lexical_latency_ms;
        }

        if let Ok(observer) = benchmark_terminal_observer_overhead() {
            for report in &mut reports {
                report.observed_command_overhead_ms = Some(observer.observed_command_overhead_ms);
                report.observed_command_capture_overhead_ms =
                    Some(observer.observed_command_capture_overhead_ms);
            }
        }

        if let Ok(web_extract) = benchmark_web_extract_metrics() {
            for report in &mut reports {
                report.web_extract_latency_ms = web_extract.latency_p50_ms;
                report.web_extract_latency_p95_ms = web_extract.latency_p95_ms;
                report.web_extract_rss_delta_mb = web_extract.rss_delta_mb;
            }
        }

        if let Ok(status) =
            wait_for_daemon_idle(&mut transport, std::time::Duration::from_secs(30)).await
            && let Some(pid) = status.pid
            && let Some(idle) = sample_stable_idle_cpu_pct(&mut transport, pid).await
        {
            for report in &mut reports {
                report.idle_cpu_pct = Some(idle);
            }
        }
    }

    let _ = transport
        .send(ClientMessage::UnregisterShell { shell_id })
        .await;

    let summary = summarize(&reports);
    let benchmark_environment =
        gather_benchmark_environment(&daemon_paths.config_path, harness_provider)?;
    let results = HarnessResults {
        suite_version: suite.suite.version.clone(),
        embedding_model: "bge-small-en-v1.5".to_string(),
        benchmark_environment,
        started_at,
        duration_secs: run_started.elapsed().as_secs(),
        tests: reports,
        summary,
    };

    let results_path = cli
        .results_out
        .clone()
        .unwrap_or_else(|| sandbox_root.join("results.json"));
    if let Some(parent) = results_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let encoded = serde_json::to_vec_pretty(&results)?;
    tokio::fs::write(&results_path, &encoded)
        .await
        .with_context(|| format!("write {}", results_path.display()))?;
    print_human_summary(&results, &results_path);
    let perf_gate_violation = if let Some(path) = &cli.perf_gates {
        let gates = load_perf_gates(path)?;
        let effective_gates = apply_hardware_gate_profile(&gates, &results.benchmark_environment);
        if effective_gates != gates {
            println!(
                "perf gate profile override: {}",
                results.benchmark_environment.hardware_class
            );
        }
        print_perf_summary(&results.tests);
        check_perf_gates(&results.tests, &effective_gates)
    } else {
        None
    };

    if !cli.keep_sandbox {
        let _ = tokio::fs::remove_dir_all(&sandbox_root).await;
    }

    let _ = daemon_child.kill();
    let _ = daemon_child.wait();
    let _ = std::fs::remove_dir_all(&daemon_paths.runtime_dir);

    if results.summary.failed > 0 {
        std::process::exit(1);
    }
    if let Some(err) = perf_gate_violation {
        eprintln!("perf gate failed:\n{err}");
        std::process::exit(2);
    }
    Ok(())
}

fn prepare_ollama_integration_runtime() -> Result<Option<OllamaIntegrationRuntime>> {
    let has_ollama = StdCommand::new("ollama")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_ollama {
        println!("termlm-test: skipping ollama-integration (ollama binary not available)");
        return Ok(None);
    }

    let short_id = Uuid::now_v7().simple().to_string();
    let root_dir = PathBuf::from(format!("/tmp/termlm-ollama-int-{}", &short_id[..12]));
    let home_dir = root_dir.join("home");
    let models_dir = root_dir.join("models");
    std::fs::create_dir_all(&home_dir).with_context(|| format!("create {}", home_dir.display()))?;
    std::fs::create_dir_all(&models_dir)
        .with_context(|| format!("create {}", models_dir.display()))?;

    let listener = TcpListener::bind("127.0.0.1:0").context("bind free tcp port for ollama")?;
    let port = listener
        .local_addr()
        .context("resolve local addr for ollama")?
        .port();
    drop(listener);

    let host = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{host}");
    let model = std::env::var("TERMLM_TEST_OLLAMA_MODEL").unwrap_or_else(|_| "gemma3:1b".into());
    let mut child = StdCommand::new("ollama")
        .arg("serve")
        .env("HOME", &home_dir)
        .env("OLLAMA_MODELS", &models_dir)
        .env("OLLAMA_HOST", &host)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn isolated ollama serve")?;

    let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut ready = false;
    while std::time::Instant::now() < ready_deadline {
        if let Some(status) = child.try_wait().context("check ollama serve status")? {
            println!(
                "termlm-test: skipping ollama-integration (isolated ollama exited early: {status})"
            );
            let _ = std::fs::remove_dir_all(&root_dir);
            return Ok(None);
        }

        let list_status = StdCommand::new("ollama")
            .arg("list")
            .env("HOME", &home_dir)
            .env("OLLAMA_MODELS", &models_dir)
            .env("OLLAMA_HOST", &host)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if list_status.map(|s| s.success()).unwrap_or(false) {
            ready = true;
            break;
        }

        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if !ready {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root_dir);
        println!("termlm-test: skipping ollama-integration (timed out starting isolated ollama)");
        return Ok(None);
    }

    let pull_status = StdCommand::new("ollama")
        .arg("pull")
        .arg(&model)
        .env("HOME", &home_dir)
        .env("OLLAMA_MODELS", &models_dir)
        .env("OLLAMA_HOST", &host)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !pull_status.map(|s| s.success()).unwrap_or(false) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root_dir);
        println!(
            "termlm-test: skipping ollama-integration (failed to pull model '{}' in isolated runtime)",
            model
        );
        return Ok(None);
    }

    println!("termlm-test: using isolated ollama runtime at {endpoint} with model {model}");
    Ok(Some(OllamaIntegrationRuntime {
        root_dir,
        endpoint,
        model,
        child: Some(child),
    }))
}

fn gather_benchmark_environment(
    config_path: &Path,
    provider: HarnessProvider,
) -> Result<BenchmarkEnvironment> {
    let cfg = termlm_config::load_or_create(Some(config_path))?.config;
    let provider_name = match provider {
        HarnessProvider::Local => "local",
        HarnessProvider::Ollama => "ollama",
    }
    .to_string();
    let model = if provider_name == "ollama" {
        cfg.ollama.model.clone()
    } else {
        cfg.model.e4b_filename.clone()
    };
    let cpu = detect_cpu_brand().unwrap_or_else(|| "unknown".to_string());
    let total_memory_mb = detect_total_memory_mb();
    let mut env = BenchmarkEnvironment {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu,
        hardware_class: HardwareClass::Other.as_str().to_string(),
        logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
        total_memory_mb,
        provider: provider_name,
        model,
        performance_profile: cfg.performance.profile.clone(),
    };
    env.hardware_class = classify_hardware_class(&env).as_str().to_string();
    Ok(env)
}

fn classify_hardware_class(env: &BenchmarkEnvironment) -> HardwareClass {
    if !(env.os == "macos" && env.arch == "aarch64" && env.provider == "local") {
        return HardwareClass::Other;
    }
    let cpu = env.cpu.to_ascii_lowercase();
    if cpu.contains("apple m3 max") {
        return HardwareClass::AppleM3MaxLocal;
    }
    if cpu.contains("apple m3 pro") {
        return HardwareClass::AppleM3ProLocal;
    }
    if cpu.contains("apple m2 pro") || cpu.contains("apple m2 max") {
        return HardwareClass::AppleM2ProMaxLocal;
    }
    HardwareClass::Other
}

fn detect_cpu_brand() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = StdCommand::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let contents = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        contents
            .lines()
            .find_map(|line| line.strip_prefix("model name\t: "))
            .map(|s| s.trim().to_string())
    }
}

fn detect_total_memory_mb() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let out = StdCommand::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let bytes = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .ok()?;
        Some(bytes.div_ceil(1024 * 1024))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb = contents.lines().find_map(|line| {
            line.strip_prefix("MemTotal:")
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
        })?;
        Some(kb.div_ceil(1024))
    }
}

fn validate_suite(suite: &SuiteConfig, sandbox_root: &Path) -> Result<()> {
    if suite.suite.total_tests != suite.test.len() {
        bail!(
            "suite total_tests={} but found {} [[test]] entries",
            suite.suite.total_tests,
            suite.test.len()
        );
    }
    for test in &suite.test {
        if !matches!(
            test.mode.as_str(),
            "execute" | "verify_proposal" | "verify_event"
        ) {
            bail!("test {} has unsupported mode '{}'", test.id, test.mode);
        }
        for command in &test.setup {
            validate_setup_command(command, sandbox_root, &test.id)?;
        }
    }
    Ok(())
}

fn validate_sandbox_root(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        bail!("sandbox root must be an absolute path: {}", root.display());
    }

    let root_s = root.to_string_lossy();
    let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let tmpdir = tmpdir.trim_end_matches('/');
    let in_tmpdir = root_s == tmpdir || root_s.starts_with(&format!("{tmpdir}/"));
    let in_tmp = root_s == "/tmp" || root_s.starts_with("/tmp/");
    if !(in_tmpdir || in_tmp) {
        bail!(
            "sandbox root must start with TMPDIR or /tmp; got {}",
            root.display()
        );
    }
    Ok(())
}

fn validate_setup_command(command: &str, sandbox_root: &Path, test_id: &str) -> Result<()> {
    if matches_safety_floor(command).is_some() {
        bail!("test {test_id} setup command rejected by safety floor preflight: {command}");
    }

    let parent_re = Regex::new(r"(^|[\s;|&()])\.\.(/|[\s;|&()])").expect("valid parent regex");
    if parent_re.is_match(command) {
        bail!(
            "test {test_id} setup command rejected by preflight parent traversal rule: {command}"
        );
    }

    let abs_path_re = Regex::new(r#"(^|[\s"'=:(])(?P<path>/[A-Za-z0-9._/@:+,\-]+)"#)
        .expect("valid absolute path regex");
    let root = sandbox_root.to_string_lossy();
    let root_prefix = format!("{}/", root.trim_end_matches('/'));
    const SAFE_ABSOLUTE_SETUP_PATHS: &[&str] = &[
        "/dev/null",
        "/dev/zero",
        "/dev/stdin",
        "/dev/stdout",
        "/dev/stderr",
        "/bin/bash",
        "/bin/sh",
        "/usr/bin/env",
    ];
    for cap in abs_path_re.captures_iter(command) {
        let path = cap.name("path").map(|m| m.as_str()).unwrap_or_default();
        if SAFE_ABSOLUTE_SETUP_PATHS.contains(&path) {
            continue;
        }
        let allowed = path == root || path.starts_with(&root_prefix);
        if !allowed {
            bail!("test {test_id} setup command references absolute path outside sandbox: {path}");
        }
    }

    Ok(())
}

fn resolve_sandbox_root(template: &str) -> Result<PathBuf> {
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let with_tmp = template.replace("${TMPDIR:-/tmp}", &tmp);
    let resolved = with_tmp.replace("{uuid}", &Uuid::now_v7().to_string());
    Ok(PathBuf::from(resolved))
}

fn write_harness_daemon_config(
    provider: HarnessProvider,
    ollama_integration: bool,
    ollama_runtime: Option<&OllamaIntegrationRuntime>,
) -> Result<HarnessDaemonPaths> {
    let short_id = Uuid::now_v7().simple().to_string();
    let runtime_dir = PathBuf::from(format!("/tmp/termlm-testd-{}", &short_id[..12]));
    std::fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("create {}", runtime_dir.display()))?;
    let home_dir = runtime_dir.join("home");
    std::fs::create_dir_all(&home_dir).with_context(|| format!("create {}", home_dir.display()))?;

    let mut cfg = AppConfig::default();
    if let Ok(host_home) = std::env::var("HOME") {
        cfg.model.models_dir = format!("{host_home}/.local/share/termlm/models");
    }
    let socket_path = runtime_dir.join("termlm.sock");
    cfg.daemon.socket_path = socket_path.display().to_string();
    cfg.daemon.pid_file = runtime_dir.join("termlm.pid").display().to_string();
    cfg.daemon.log_file = runtime_dir.join("termlm.log").display().to_string();
    cfg.daemon.shutdown_grace_secs = 1;
    cfg.model.auto_download = false;
    match provider {
        HarnessProvider::Local => {
            cfg.inference.provider = "local".to_string();
        }
        HarnessProvider::Ollama => {
            cfg.inference.provider = "ollama".to_string();
            if ollama_integration {
                if let Some(runtime) = ollama_runtime {
                    cfg.ollama.endpoint = runtime.endpoint.clone();
                    cfg.ollama.model = runtime.model.clone();
                } else {
                    cfg.ollama.endpoint = std::env::var("TERMLM_TEST_OLLAMA_ENDPOINT")
                        .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
                    cfg.ollama.model = std::env::var("TERMLM_TEST_OLLAMA_MODEL")
                        .unwrap_or_else(|_| "gemma3:1b".to_string());
                }
                cfg.ollama.connect_timeout_secs = 3;
                cfg.ollama.request_timeout_secs = 60;
                cfg.ollama.healthcheck_on_start = true;
            } else {
                cfg.ollama.endpoint = "http://127.0.0.1:9".to_string();
                cfg.ollama.connect_timeout_secs = 1;
                cfg.ollama.request_timeout_secs = 2;
                cfg.ollama.healthcheck_on_start = false;
            }
        }
    }
    cfg.web.enabled = false;
    cfg.web.expose_tools = false;

    let config_path = runtime_dir.join("config.toml");
    let encoded = toml::to_string_pretty(&cfg)?;
    std::fs::write(&config_path, encoded)
        .with_context(|| format!("write {}", config_path.display()))?;
    Ok(HarnessDaemonPaths {
        config_path,
        socket_path,
        index_root: home_dir.join(".local/share/termlm/index"),
        runtime_dir,
        home_dir,
    })
}

fn spawn_daemon(
    daemon_paths: &HarnessDaemonPaths,
    sandbox_root: &Path,
    provider: HarnessProvider,
    runtime_real: bool,
) -> Result<std::process::Child> {
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["run", "-p", "termlm-core"]);
    cmd.args(daemon_runtime_feature_args(provider, runtime_real));

    cmd.arg("--")
        .arg("--config")
        .arg(&daemon_paths.config_path)
        .arg("--sandbox-cwd")
        .arg(sandbox_root)
        .env("HOME", &daemon_paths.home_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());

    cmd.spawn().context("failed to spawn termlm-core")
}

fn daemon_runtime_feature_args(provider: HarnessProvider, runtime_real: bool) -> Vec<&'static str> {
    if matches!(provider, HarnessProvider::Local) && !runtime_real {
        vec!["--no-default-features", "--features", "runtime-stub"]
    } else {
        Vec::new()
    }
}

async fn wait_for_socket_with_child(
    socket: &Path,
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if tokio::fs::metadata(socket).await.is_ok() {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .context("failed to inspect spawned daemon process")?
        {
            bail!("termlm-core exited before socket was ready (status: {status})");
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    bail!(
        "socket did not appear at {} within {:?}",
        socket.display(),
        timeout
    );
}

async fn connect_transport(socket: &Path) -> Result<ClientTransport> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_BYTES)
        .new_codec();
    let framed = Framed::new(stream, codec);
    Ok(tokio_serde::Framed::new(
        framed,
        Json::<ServerMessage, ClientMessage>::default(),
    ))
}

async fn register_shell(transport: &mut ClientTransport) -> Result<Uuid> {
    transport
        .send(ClientMessage::RegisterShell {
            payload: RegisterShell {
                shell_pid: std::process::id(),
                tty: "harness".to_string(),
                client_version: "termlm-test".to_string(),
                shell_kind: ShellKind::Zsh,
                shell_version: "test".to_string(),
                adapter_version: "test".to_string(),
                capabilities: default_capabilities(),
                env_subset: env_subset(),
            },
        })
        .await?;

    while let Some(msg) = transport.next().await {
        let msg = msg?;
        if let ServerMessage::ShellRegistered { shell_id, .. } = msg {
            return Ok(shell_id);
        }
    }
    bail!("did not receive ShellRegistered");
}

async fn send_shell_context(
    transport: &mut ClientTransport,
    shell_id: Uuid,
    suite: &SuiteConfig,
) -> Result<()> {
    let aliases = suite
        .shell_context
        .aliases
        .iter()
        .map(|(name, expansion)| AliasDef {
            name: name.clone(),
            expansion: expansion.clone(),
        })
        .collect::<Vec<_>>();
    let functions = suite
        .shell_context
        .functions
        .iter()
        .map(|(name, body)| FunctionDef {
            name: name.clone(),
            body_prefix: body.clone(),
        })
        .collect::<Vec<_>>();

    transport
        .send(ClientMessage::ShellContext {
            payload: ShellContext {
                shell_id,
                shell_kind: ShellKind::Zsh,
                context_hash: Uuid::now_v7().to_string(),
                aliases,
                functions,
                builtins: vec!["cd".to_string(), "echo".to_string()],
            },
        })
        .await?;
    Ok(())
}

async fn kick_delta_reindex_best_effort(transport: &mut ClientTransport) {
    let _ = transport
        .send(ClientMessage::Reindex {
            mode: ReindexMode::Delta,
        })
        .await;
    let wait = tokio::time::Duration::from_secs(2);
    if let Ok(Some(Ok(ServerMessage::IndexProgress(_)))) =
        tokio::time::timeout(wait, transport.next()).await
    {}
}

fn test_selected(test: &TestCase, mode: &HarnessMode) -> bool {
    match mode {
        HarnessMode::All => true,
        HarnessMode::LocalIntegration => false,
        HarnessMode::OllamaIntegration => true,
        HarnessMode::Retrieval => true,
        HarnessMode::E2e => matches!(test.mode.as_str(), "execute" | "verify_proposal"),
        HarnessMode::Safety => {
            test.mode == "verify_event"
                || test.category == "safety_floor"
                || test.category == "critical_approval"
        }
    }
}

fn daemon_boot_timeout_secs(runtime_real: bool, ollama_integration_mode: bool) -> u64 {
    if let Ok(raw) = std::env::var("TERMLM_TEST_DAEMON_BOOT_TIMEOUT_SECS")
        && let Ok(parsed) = raw.parse::<u64>()
        && parsed > 0
    {
        return parsed;
    }
    if runtime_real {
        return 600;
    }
    if ollama_integration_mode {
        return 60;
    }
    180
}

fn parse_positive_u64(raw: Option<String>) -> Option<u64> {
    let parsed = raw?.trim().parse::<u64>().ok()?;
    (parsed > 0).then_some(parsed)
}

fn select_timeout_secs(
    specific_raw: Option<String>,
    global_raw: Option<String>,
    default_secs: u64,
) -> u64 {
    parse_positive_u64(specific_raw)
        .or_else(|| parse_positive_u64(global_raw))
        .unwrap_or(default_secs)
}

fn resolved_timeout_secs(specific_env: &str, default_secs: u64) -> u64 {
    select_timeout_secs(
        std::env::var(specific_env).ok(),
        std::env::var("TERMLM_TEST_REINDEX_TIMEOUT_SECS").ok(),
        default_secs,
    )
}

fn reindex_full_timeout_secs() -> u64 {
    resolved_timeout_secs("TERMLM_TEST_REINDEX_FULL_TIMEOUT_SECS", 180)
}

fn reindex_delta_timeout_secs() -> u64 {
    resolved_timeout_secs("TERMLM_TEST_REINDEX_DELTA_TIMEOUT_SECS", 60)
}

fn ollama_integration_cases() -> Vec<TestCase> {
    let expected = termlm_test::Expected::default();
    vec![
        TestCase {
            id: "OLI-001".to_string(),
            category: "ollama_integration".to_string(),
            prompt: "List files in the current directory.".to_string(),
            setup: Vec::new(),
            mode: "verify_proposal".to_string(),
            expected: expected.clone(),
            relevant_commands: Vec::new(),
            approval_mode: None,
        },
        TestCase {
            id: "OLI-002".to_string(),
            category: "ollama_integration".to_string(),
            prompt: "Find files containing TODO recursively.".to_string(),
            setup: Vec::new(),
            mode: "verify_proposal".to_string(),
            expected: expected.clone(),
            relevant_commands: Vec::new(),
            approval_mode: None,
        },
        TestCase {
            id: "OLI-003".to_string(),
            category: "ollama_integration".to_string(),
            prompt: "What command shows current git branch status?".to_string(),
            setup: Vec::new(),
            mode: "verify_proposal".to_string(),
            expected,
            relevant_commands: Vec::new(),
            approval_mode: None,
        },
    ]
}

fn local_integration_cases() -> Vec<TestCase> {
    let expected = termlm_test::Expected::default();
    vec![
        TestCase {
            id: "LRI-001".to_string(),
            category: "local_integration".to_string(),
            prompt: "List files in the current directory.".to_string(),
            setup: Vec::new(),
            mode: "verify_proposal".to_string(),
            expected: expected.clone(),
            relevant_commands: Vec::new(),
            approval_mode: None,
        },
        TestCase {
            id: "LRI-002".to_string(),
            category: "local_integration".to_string(),
            prompt: "Show me the current working directory.".to_string(),
            setup: Vec::new(),
            mode: "verify_proposal".to_string(),
            expected: expected.clone(),
            relevant_commands: Vec::new(),
            approval_mode: None,
        },
        TestCase {
            id: "LRI-003".to_string(),
            category: "local_integration".to_string(),
            prompt: "What command shows current git branch status?".to_string(),
            setup: Vec::new(),
            mode: "verify_proposal".to_string(),
            expected,
            relevant_commands: Vec::new(),
            approval_mode: None,
        },
    ]
}

async fn run_one_test(
    transport: &mut ClientTransport,
    shell_id: Uuid,
    test: &TestCase,
    test_dir: &Path,
    harness_mode: &HarnessMode,
    top_k: u32,
    timeout_secs: u64,
) -> Result<TestReport> {
    for cmd in &test.setup {
        run_setup_command(test_dir, cmd).await?;
    }

    let started = Instant::now();
    let mut retrieval_score = None;
    let mut retrieval_latency_ms = None;
    let mut task_latency_ms = None;
    let mut stage_timings_ms = BTreeMap::new();
    let mut model_load_ms = None;
    let mut model_resident_mb = None;
    let mut indexer_resident_mb = None;
    let mut orchestration_resident_mb = None;
    let mut rss_mb = None;
    let mut kv_cache_mb = None;
    let mut last_task_prompt_tokens = None;
    let mut last_task_completion_tokens = None;
    let mut last_task_usage_reported = None;
    let mut source_ledger_ref_count = None;
    let mut source_ledger_overhead_ms = None;
    let mut tool_routing_overhead_ms = None;
    let mut pre_provider_overhead_ms = None;
    let mut planning_loop_overhead_ms = None;
    let mut ttft_ms = None;
    let mut throughput_toks_per_sec = None;
    let mut throughput_source = None;
    let mut ollama_orchestration_overhead_ms = None;
    let mut proposed_command = None;
    let mut exit_status = None;
    let mut failure = None;

    if matches!(harness_mode, HarnessMode::Retrieval | HarnessMode::All) {
        let retrieval_started = Instant::now();
        let score = run_retrieval_check(transport, test, top_k).await?;
        retrieval_latency_ms = Some(retrieval_started.elapsed().as_millis() as u64);
        if !test.relevant_commands.is_empty() && !score.hit {
            failure = Some(format!(
                "retrieval miss: relevant commands {:?} not found in top {}",
                test.relevant_commands, top_k
            ));
        }
        retrieval_score = Some(score);
    }

    if !matches!(harness_mode, HarnessMode::Retrieval) {
        let task_started = Instant::now();
        let output = run_task_check(transport, shell_id, test, test_dir, timeout_secs).await?;
        task_latency_ms = Some(task_started.elapsed().as_millis() as u64);
        ttft_ms = output.ttft_ms;
        proposed_command = output.proposed_command.clone();
        exit_status = output.exit_status;
        let status = fetch_status_metrics(transport).await?;
        stage_timings_ms = status.stage_timings_ms;
        ollama_orchestration_overhead_ms = stage_timings_ms
            .get("provider_orchestration_ms")
            .copied()
            .map(|v| v as f64);
        model_load_ms = status.model_load_ms;
        model_resident_mb = status.model_resident_mb;
        indexer_resident_mb = status.indexer_resident_mb;
        orchestration_resident_mb = status.orchestration_resident_mb;
        rss_mb = status.rss_mb;
        kv_cache_mb = status.kv_cache_mb;
        last_task_prompt_tokens = status.last_task_prompt_tokens;
        last_task_completion_tokens = status.last_task_completion_tokens;
        last_task_usage_reported = status.last_task_usage_reported;
        throughput_toks_per_sec = output.throughput_toks_per_sec_heuristic;
        throughput_source = throughput_toks_per_sec.map(|_| "heuristic_char_estimate".to_string());
        if let (Some(window_secs), Some(completion_tokens)) = (
            output.stream_window_secs,
            status.last_task_completion_tokens,
        ) && window_secs > 0.0
            && completion_tokens > 0
        {
            throughput_toks_per_sec = Some(completion_tokens as f64 / window_secs);
            throughput_source = Some("provider_reported_tokens".to_string());
        }
        source_ledger_ref_count = status.last_task_source_refs.map(|v| v as u64);
        source_ledger_overhead_ms = stage_timings_ms.get("source_ledger_ms").copied();
        tool_routing_overhead_ms = stage_timings_ms.get("classify_ms").copied();
        planning_loop_overhead_ms = stage_timings_ms
            .get("provider_orchestration_ms")
            .copied()
            .or_else(|| stage_timings_ms.get("runtime_stub_provider_ms").copied());
        pre_provider_overhead_ms = Some(
            stage_timings_ms
                .get("source_ledger_ms")
                .copied()
                .unwrap_or(0)
                + stage_timings_ms.get("classify_ms").copied().unwrap_or(0)
                + stage_timings_ms
                    .get("assemble_context_ms")
                    .copied()
                    .unwrap_or(0)
                + stage_timings_ms
                    .get("progress_banner_ms")
                    .copied()
                    .unwrap_or(0),
        );
        if let Err(e) = evaluate_expected(test, test_dir, &output) {
            failure = Some(e.to_string());
        }
    }

    Ok(TestReport {
        id: test.id.clone(),
        mode: test.mode.clone(),
        category: test.category.clone(),
        passed: failure.is_none(),
        duration_ms: started.elapsed().as_millis() as u64,
        retrieval_score,
        retrieval_latency_ms,
        retrieval_50k_latency_ms: None,
        retrieval_50k_lexical_ms: None,
        task_latency_ms,
        ttft_ms,
        throughput_toks_per_sec,
        throughput_source,
        model_load_ms,
        model_resident_mb,
        indexer_resident_mb,
        orchestration_resident_mb,
        last_task_prompt_tokens,
        last_task_completion_tokens,
        last_task_usage_reported,
        embedding_chunks_per_sec: None,
        full_reindex_ms: None,
        delta_reindex_ms: None,
        index_disk_mb: None,
        ollama_orchestration_overhead_ms,
        observed_command_overhead_ms: None,
        observed_command_capture_overhead_ms: None,
        idle_cpu_pct: None,
        source_ledger_ref_count,
        source_ledger_overhead_ms,
        tool_routing_overhead_ms,
        pre_provider_overhead_ms,
        planning_loop_overhead_ms,
        web_extract_latency_ms: None,
        web_extract_latency_p95_ms: None,
        web_extract_rss_delta_mb: None,
        stage_timings_ms,
        rss_mb,
        kv_cache_mb,
        proposed_command,
        exit_status,
        error: failure,
    })
}

async fn run_setup_command(cwd: &Path, command: &str) -> Result<()> {
    let out = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("setup command failed to spawn: {command}"))?;
    if !out.status.success() {
        bail!(
            "setup command failed (status={}): {}\nstderr: {}",
            out.status.code().unwrap_or(-1),
            command,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

async fn run_retrieval_check(
    transport: &mut ClientTransport,
    test: &TestCase,
    top_k: u32,
) -> Result<RetrievalScore> {
    transport
        .send(ClientMessage::Retrieve {
            payload: RetrieveRequest {
                prompt: test.prompt.clone(),
                top_k: Some(top_k),
            },
        })
        .await?;

    let mut chunks = Vec::<termlm_protocol::RetrievedChunk>::new();
    while let Some(msg) = transport.next().await {
        match msg? {
            ServerMessage::RetrievalResult { chunks: found } => {
                chunks = found;
                break;
            }
            ServerMessage::Error { message, .. } => bail!("retrieve failed: {message}"),
            _ => continue,
        }
    }

    let mut best_rank = None;
    for (idx, chunk) in chunks.iter().enumerate() {
        if test
            .relevant_commands
            .iter()
            .any(|c| c == &chunk.command_name)
        {
            best_rank = Some(idx + 1);
            break;
        }
    }
    Ok(RetrievalScore {
        top_k,
        hit: best_rank.is_some(),
        best_rank,
    })
}

#[derive(Debug, Default)]
struct StatusMetrics {
    pid: Option<u32>,
    stage_timings_ms: BTreeMap<String, u64>,
    model_load_ms: Option<u64>,
    model_resident_mb: Option<u64>,
    indexer_resident_mb: Option<u64>,
    orchestration_resident_mb: Option<u64>,
    rss_mb: Option<u64>,
    kv_cache_mb: Option<u64>,
    last_task_prompt_tokens: Option<u64>,
    last_task_completion_tokens: Option<u64>,
    last_task_usage_reported: Option<bool>,
    last_task_source_refs: Option<usize>,
    active_tasks: Option<usize>,
    index_phase: Option<String>,
}

async fn fetch_status_metrics(transport: &mut ClientTransport) -> Result<StatusMetrics> {
    transport.send(ClientMessage::Status).await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(remaining, transport.next()).await;
        let Ok(Some(Ok(server_msg))) = msg else {
            continue;
        };
        if let ServerMessage::StatusReport {
            pid,
            rss_mb,
            model_resident_mb,
            indexer_resident_mb,
            orchestration_resident_mb,
            kv_cache_mb,
            stage_timings_ms,
            model_load_ms,
            last_task_prompt_tokens,
            last_task_completion_tokens,
            last_task_usage_reported,
            last_task_source_refs,
            active_tasks,
            index_progress,
            ..
        } = server_msg
        {
            return Ok(StatusMetrics {
                pid: Some(pid),
                stage_timings_ms,
                model_load_ms: Some(model_load_ms),
                model_resident_mb: Some(model_resident_mb),
                indexer_resident_mb: Some(indexer_resident_mb),
                orchestration_resident_mb: Some(orchestration_resident_mb),
                rss_mb: Some(rss_mb),
                kv_cache_mb: Some(kv_cache_mb),
                last_task_prompt_tokens,
                last_task_completion_tokens,
                last_task_usage_reported: Some(last_task_usage_reported),
                last_task_source_refs: Some(last_task_source_refs),
                active_tasks: Some(active_tasks),
                index_phase: Some(index_progress.phase),
            });
        }
    }
    Ok(StatusMetrics::default())
}

async fn wait_for_daemon_idle(
    transport: &mut ClientTransport,
    timeout: std::time::Duration,
) -> Result<StatusMetrics> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = StatusMetrics::default();
    while tokio::time::Instant::now() < deadline {
        let status = fetch_status_metrics(transport).await?;
        let no_active_tasks = status.active_tasks.unwrap_or(1) == 0;
        let index_idle = matches!(status.index_phase.as_deref(), Some("idle" | "complete"));
        if no_active_tasks && index_idle && status.pid.is_some() {
            return Ok(status);
        }
        last = status;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(last)
}

fn sample_idle_cpu_pct(pid: u32) -> Option<f64> {
    std::thread::sleep(std::time::Duration::from_millis(150));
    #[cfg(target_os = "macos")]
    {
        let out = StdCommand::new("top")
            .args([
                "-l",
                "2",
                "-stats",
                "pid,cpu",
                "-pid",
                &pid.to_string(),
                "-n",
                "2",
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let pid_text = pid.to_string();
        let mut latest = None::<f64>;
        for line in text.lines() {
            let trimmed = line.trim_start();
            let mut fields = trimmed.split_whitespace();
            let Some(row_pid) = fields.next() else {
                continue;
            };
            if row_pid != pid_text {
                continue;
            }
            for field in fields {
                let normalized = field.trim_end_matches('%');
                if let Ok(cpu) = normalized.parse::<f64>() {
                    latest = Some(cpu);
                    break;
                }
            }
        }
        if latest.is_some() {
            return latest;
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        let out = StdCommand::new("ps")
            .args(["-p", &pid.to_string(), "-o", "%cpu="])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        value.parse::<f64>().ok()
    }
}

fn median_f64(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    } else {
        Some(sorted[mid])
    }
}

async fn sample_stable_idle_cpu_pct(transport: &mut ClientTransport, pid: u32) -> Option<f64> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut samples = Vec::new();

    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let status = fetch_status_metrics(transport).await.ok()?;
        let no_active_tasks = status.active_tasks.unwrap_or(1) == 0;
        let index_idle = matches!(status.index_phase.as_deref(), Some("idle" | "complete"));
        if !no_active_tasks || !index_idle {
            continue;
        }

        if let Some(sample) = sample_idle_cpu_pct(pid) {
            samples.push(sample);
            if samples.len() >= 3
                && let Some(median) = median_f64(&samples)
                && median <= 1.5
            {
                return Some(median);
            }
        }
    }

    median_f64(&samples)
}

fn sample_process_rss_mb(pid: u32) -> Option<u64> {
    let out = StdCommand::new("ps")
        .args(["-p", &pid.to_string(), "-o", "rss="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let rss_kb = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .ok()?;
    Some((rss_kb.saturating_add(1023)) / 1024)
}

fn sample_web_extract_html(target_bytes: usize) -> String {
    let mut html = String::from("<html><head><title>Docs</title></head><body><main>");
    let mut section = 0usize;
    while html.len() < target_bytes {
        html.push_str(&format!(
            "<section><h2>Section {section}</h2><p>Paragraph {section} with <a href='https://example.com/page?utm_source=test&id={section}'>link</a> and <code>cmd --flag {section}</code>.</p><pre><code class='language-bash'>echo section-{section}\nls -la</code></pre></section>"
        ));
        section += 1;
    }
    html.push_str("</main></body></html>");
    html
}

fn benchmark_web_extract_metrics() -> Result<WebExtractBenchmarkOutput> {
    let html = sample_web_extract_html(900 * 1024);
    let opts = termlm_web::extract::ExtractOptions::default();
    let pid = std::process::id();
    let baseline_rss_mb = sample_process_rss_mb(pid).unwrap_or(0);
    let mut peak_rss_mb = baseline_rss_mb;
    let mut durations = Vec::<u64>::new();
    let iterations = 21usize;

    for _ in 0..iterations {
        let started = Instant::now();
        let out = termlm_web::extract::extract_markdown_with_options(&html, &opts);
        std::hint::black_box(&out.markdown);
        durations.push(started.elapsed().as_millis() as u64);
        if let Some(rss_mb) = sample_process_rss_mb(pid) {
            peak_rss_mb = peak_rss_mb.max(rss_mb);
        }
    }

    durations.sort_unstable();
    if durations.is_empty() {
        return Ok(WebExtractBenchmarkOutput::default());
    }
    let p50_ms = percentile(&durations, 0.50);
    let p95_ms = percentile(&durations, 0.95);
    let rss_delta_mb = peak_rss_mb.saturating_sub(baseline_rss_mb);

    Ok(WebExtractBenchmarkOutput {
        latency_p50_ms: Some(p50_ms),
        latency_p95_ms: Some(p95_ms),
        rss_delta_mb: Some(rss_delta_mb),
    })
}

fn build_retrieval_bench_chunks(count: usize) -> Vec<Chunk> {
    let sections = ["NAME", "SYNOPSIS", "OPTIONS", "EXAMPLES"];
    let mut chunks = Vec::with_capacity(count);
    let now = chrono::Utc::now();
    for i in 0..count {
        let command = format!("cmd{}", i % 3000);
        let section = sections[i % sections.len()].to_string();
        let text = format!(
            "{command} supports --flag{} and pattern search over workspace file {}",
            i % 7,
            i % 101
        );
        chunks.push(Chunk {
            command_name: command.clone(),
            path: format!("/usr/local/share/man/man1/{command}.1"),
            extraction_method: "man".to_string(),
            section_name: section,
            chunk_index: i % 4,
            total_chunks: 4,
            doc_hash: format!("h{i:08x}"),
            extracted_at: now,
            text,
        });
    }
    chunks
}

fn run_retrieval_query_bench_ms(
    retriever: &HybridRetriever,
    query: &RetrievalQuery,
) -> Option<u64> {
    let warmup_iters = 6usize;
    let samples = 31usize;
    for _ in 0..warmup_iters {
        std::hint::black_box(retriever.search(query));
    }
    let mut elapsed_us = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        std::hint::black_box(retriever.search(query));
        elapsed_us.push(started.elapsed().as_micros() as u64);
    }
    if elapsed_us.is_empty() {
        return None;
    }
    elapsed_us.sort_unstable();
    let p50_us = percentile(&elapsed_us, 0.50);
    Some(p50_us.div_ceil(1000))
}

fn benchmark_retrieval_50k_metrics() -> Retrieval50kBenchmarkOutput {
    let chunks = build_retrieval_bench_chunks(50_000);
    let hybrid_retriever = HybridRetriever::with_dim(chunks.clone(), 384);
    let lexical_retriever = HybridRetriever::lexical_only(chunks);

    let hybrid_query = RetrievalQuery::new("cmd42 --flag2 options", 8, 0.0);
    let mut lexical_query = RetrievalQuery::new("cmd42 --flag2 options", 8, 0.0);
    lexical_query.hybrid_enabled = false;
    lexical_query.lexical_enabled = true;

    Retrieval50kBenchmarkOutput {
        hybrid_latency_ms: run_retrieval_query_bench_ms(&hybrid_retriever, &hybrid_query),
        lexical_latency_ms: run_retrieval_query_bench_ms(&lexical_retriever, &lexical_query),
    }
}

async fn benchmark_index_metrics(
    transport: &mut ClientTransport,
    index_root: &Path,
) -> Result<IndexBenchmarkOutput> {
    let mut out = IndexBenchmarkOutput::default();
    let full_timeout_secs = reindex_full_timeout_secs();
    let delta_timeout_secs = reindex_delta_timeout_secs();

    let full_started = Instant::now();
    transport
        .send(ClientMessage::Reindex {
            mode: ReindexMode::Full,
        })
        .await?;
    wait_for_reindex_completion(transport, std::time::Duration::from_secs(full_timeout_secs))
        .await?;
    let full_reindex_ms = full_started.elapsed().as_millis() as u64;
    out.full_reindex_ms = Some(full_reindex_ms);

    let manifest_path = index_root.join("manifest.json");
    let chunk_count = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("chunk_count").and_then(|n| n.as_u64()))
        .unwrap_or(0);
    if chunk_count > 0 {
        let elapsed = full_started.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            out.embedding_chunks_per_sec = Some(chunk_count as f64 / elapsed);
        }
    }

    let delta_started = Instant::now();
    transport
        .send(ClientMessage::Reindex {
            mode: ReindexMode::Delta,
        })
        .await?;
    wait_for_reindex_completion(
        transport,
        std::time::Duration::from_secs(delta_timeout_secs),
    )
    .await?;
    out.delta_reindex_ms = Some(delta_started.elapsed().as_millis() as u64);

    let bytes = dir_size_bytes(index_root)?;
    out.index_disk_mb = Some(bytes.div_ceil(1024 * 1024));

    Ok(out)
}

async fn wait_for_reindex_completion(
    transport: &mut ClientTransport,
    timeout: std::time::Duration,
) -> Result<()> {
    let started = tokio::time::Instant::now();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut saw_active_phase = false;
    while tokio::time::Instant::now() < deadline {
        let progress = fetch_index_phase(transport).await?;
        if let Some(phase) = progress.as_deref() {
            if !matches!(phase, "complete" | "idle") {
                saw_active_phase = true;
            } else if saw_active_phase || started.elapsed() >= std::time::Duration::from_secs(1) {
                return Ok(());
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    bail!("reindex did not reach completion within {:?}", timeout);
}

fn dir_size_bytes(root: &Path) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut stack = vec![root.to_path_buf()];
    let mut total = 0u64;
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).with_context(|| format!("read {}", path.display()))? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}

async fn fetch_index_phase(transport: &mut ClientTransport) -> Result<Option<String>> {
    transport.send(ClientMessage::Status).await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(remaining, transport.next()).await;
        let Ok(Some(Ok(server_msg))) = msg else {
            continue;
        };
        if let ServerMessage::StatusReport { index_progress, .. } = server_msg {
            return Ok(Some(index_progress.phase));
        }
    }
    Ok(None)
}

fn benchmark_terminal_observer_overhead() -> Result<TerminalObserverBenchmarkOutput> {
    let script_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/perf/terminal_observer_overhead.zsh");
    let output = StdCommand::new("zsh")
        .arg(script_path)
        .output()
        .context("run terminal observer overhead benchmark")?;
    if !output.status.success() {
        bail!(
            "terminal observer benchmark failed (status={}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        bail!("terminal observer benchmark emitted empty output");
    }
    let parsed = serde_json::from_str::<TerminalObserverBenchmarkOutput>(&raw)
        .context("parse terminal observer benchmark json output")?;
    Ok(parsed)
}

async fn run_task_check(
    transport: &mut ClientTransport,
    shell_id: Uuid,
    test: &TestCase,
    test_dir: &Path,
    timeout_secs: u64,
) -> Result<TaskRunOutput> {
    let task_id = Uuid::now_v7();
    transport
        .send(ClientMessage::StartTask {
            payload: StartTask {
                task_id,
                shell_id,
                shell_kind: ShellKind::Zsh,
                shell_version: "test".to_string(),
                mode: "?".to_string(),
                prompt: test.prompt.clone(),
                cwd: test_dir.display().to_string(),
                env_subset: env_subset_with_pwd(test_dir),
            },
        })
        .await?;

    let mut output = TaskRunOutput::default();
    let task_started = Instant::now();
    let mut first_response_at = None::<Instant>;
    let mut model_text_chars = 0usize;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut proposal_count = 0usize;
    let mut sent_abort_for_verify_proposal = false;

    let completion_at = loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            bail!("task timed out after {timeout_secs}s");
        }
        let remaining = deadline - now;
        let msg = tokio::time::timeout(remaining, transport.next())
            .await
            .context("timeout while waiting for daemon message")?;
        let Some(msg) = msg else {
            bail!("daemon disconnected");
        };
        match msg? {
            ServerMessage::ModelText { chunk, .. } => {
                if first_response_at.is_none() {
                    first_response_at = Some(Instant::now());
                }
                model_text_chars = model_text_chars.saturating_add(chunk.chars().count());
                output.trace.push_str(&chunk);
                output.trace.push('\n');
                if test.mode == "verify_proposal"
                    && output.proposed_command.is_none()
                    && chunk.contains("lookup_command_docs")
                {
                    output.proposed_command = Some("lookup_command_docs".to_string());
                }
            }
            ServerMessage::NeedsClarification { question, .. } => {
                if first_response_at.is_none() {
                    first_response_at = Some(Instant::now());
                }
                output.saw_needs_clarification = true;
                if question
                    .to_ascii_lowercase()
                    .contains("validation_incomplete")
                {
                    output.saw_validation_incomplete = true;
                }
                output
                    .trace
                    .push_str(&format!("NeedsClarification: {question}\n"));
                transport
                    .send(ClientMessage::UserResponse {
                        payload: UserResponse {
                            task_id,
                            decision: UserDecision::Abort,
                            edited_command: None,
                            text: None,
                        },
                    })
                    .await?;
            }
            ServerMessage::Error { kind, message, .. } => {
                if first_response_at.is_none() {
                    first_response_at = Some(Instant::now());
                }
                output
                    .trace
                    .push_str(&format!("Error({kind:?}): {message}\n"));
                if kind == ErrorKind::SafetyFloor {
                    output.saw_safety_floor = true;
                }
                if kind == ErrorKind::UnknownCommand {
                    output.saw_unknown_command = true;
                }
            }
            ServerMessage::ProposedCommand { payload } => {
                if first_response_at.is_none() {
                    first_response_at = Some(Instant::now());
                }
                if output.proposed_command.is_none() {
                    output.proposed_command = Some(payload.cmd.clone());
                }
                if payload.validation.status == "validation_incomplete" {
                    output.saw_validation_incomplete = true;
                }
                output
                    .trace
                    .push_str(&format!("Proposed: {}\n", payload.cmd));
                proposal_count = proposal_count.saturating_add(1);
                if proposal_count > 12 {
                    bail!("received too many proposed commands without task completion");
                }
                match test.mode.as_str() {
                    "execute" => {
                        transport
                            .send(ClientMessage::UserResponse {
                                payload: UserResponse {
                                    task_id,
                                    decision: UserDecision::Approved,
                                    edited_command: None,
                                    text: None,
                                },
                            })
                            .await?;
                        let started = Instant::now();
                        let (status, stdout, stderr) =
                            execute_command_in_sandbox(&payload.cmd, test_dir, timeout_secs)
                                .await?;
                        output.exit_status = Some(status);
                        output.stdout = stdout.clone();
                        output.stderr = stderr.clone();
                        transport
                            .send(ClientMessage::Ack {
                                payload: Ack {
                                    task_id,
                                    command_seq: 1,
                                    executed_command: payload.cmd.clone(),
                                    cwd_before: test_dir.display().to_string(),
                                    cwd_after: test_dir.display().to_string(),
                                    started_at: chrono::Utc::now(),
                                    exit_status: status,
                                    stdout_b64: Some(
                                        base64::engine::general_purpose::STANDARD.encode(stdout),
                                    ),
                                    stdout_truncated: false,
                                    stderr_b64: Some(
                                        base64::engine::general_purpose::STANDARD.encode(stderr),
                                    ),
                                    stderr_truncated: false,
                                    redactions_applied: Vec::new(),
                                    elapsed_ms: started.elapsed().as_millis() as u64,
                                },
                            })
                            .await?;
                    }
                    "verify_proposal" if !sent_abort_for_verify_proposal => {
                        sent_abort_for_verify_proposal = true;
                        transport
                            .send(ClientMessage::UserResponse {
                                payload: UserResponse {
                                    task_id,
                                    decision: UserDecision::Abort,
                                    edited_command: None,
                                    text: None,
                                },
                            })
                            .await?;
                    }
                    "verify_proposal" => {}
                    _ => {}
                }
            }
            ServerMessage::TaskComplete { summary, .. } => {
                if first_response_at.is_none() {
                    first_response_at = Some(Instant::now());
                }
                if summary
                    .to_ascii_lowercase()
                    .contains("validation_incomplete")
                {
                    output.saw_validation_incomplete = true;
                }
                break Instant::now();
            }
            _ => {}
        }
    };

    output.ttft_ms =
        first_response_at.map(|first| first.duration_since(task_started).as_millis() as u64);
    output.stream_window_secs =
        first_response_at.map(|first| completion_at.saturating_duration_since(first).as_secs_f64());
    output.throughput_toks_per_sec_heuristic = first_response_at.and_then(|first| {
        let elapsed = completion_at.saturating_duration_since(first).as_secs_f64();
        if model_text_chars == 0 || elapsed <= 0.0 {
            return None;
        }
        // Approximate token count from emitted UTF-8 chars for lightweight CI gating.
        Some((model_text_chars as f64 / 4.0) / elapsed)
    });

    Ok(output)
}

async fn execute_command_in_sandbox(
    command: &str,
    cwd: &Path,
    timeout_secs: u64,
) -> Result<(i32, String, String)> {
    let child = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run command: {command}"))?;

    let wait = child.wait_with_output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), wait)
        .await
        .context("sandbox command timed out")??;
    let status = out.status.code().unwrap_or(-1);
    Ok((
        status,
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

fn evaluate_expected(test: &TestCase, test_dir: &Path, output: &TaskRunOutput) -> Result<()> {
    if test.expected.forbid_proposed_command.unwrap_or(false) && output.proposed_command.is_some() {
        bail!(
            "test {} expected no proposed command but observed {:?}",
            test.id,
            output.proposed_command
        );
    }

    match test.mode.as_str() {
        "verify_event" => {
            let Some(expected) = test.expected.event_type.as_deref() else {
                bail!("verify_event test {} missing expected.event_type", test.id);
            };
            if !matches_expected_event(expected, output) {
                bail!("expected event '{expected}' was not observed");
            }
            return Ok(());
        }
        "verify_proposal" | "execute" => {}
        other => bail!("unsupported test mode {other}"),
    }

    if !test.expected.command_regex.is_empty() {
        let subject = output
            .proposed_command
            .as_deref()
            .unwrap_or(output.trace.as_str());
        let mut matched = false;
        for re in &test.expected.command_regex {
            if pattern_matches(re, subject).with_context(|| format!("invalid regex {re}"))? {
                matched = true;
                break;
            }
        }
        if !matched {
            bail!(
                "proposed command did not match any expected regex; subject='{}'",
                subject
            );
        }
    }

    if test.mode == "execute" {
        if let Some(must_succeed) = test.expected.must_succeed
            && must_succeed
            && output.exit_status.unwrap_or(-1) != 0
        {
            bail!(
                "expected success but exit status was {:?}",
                output.exit_status
            );
        }
        for needle in &test.expected.stdout_contains {
            if !output.stdout.contains(needle) {
                bail!("stdout missing expected substring '{}'", needle);
            }
        }
        if !test.expected.stdout_order.is_empty() {
            let mut cursor = 0usize;
            for needle in &test.expected.stdout_order {
                let slice = &output.stdout[cursor..];
                let Some(pos) = slice.find(needle) else {
                    bail!("stdout order check failed; missing '{}'", needle);
                };
                cursor += pos + needle.len();
            }
        }
        if let Some(fs_expect) = &test.expected.filesystem_state_after {
            for rel in &fs_expect.exists {
                if !test_dir.join(rel).exists() {
                    bail!("expected path to exist after run: {}", rel);
                }
            }
            for rel in &fs_expect.not_exists {
                if test_dir.join(rel).exists() {
                    bail!("expected path to be absent after run: {}", rel);
                }
            }
        }
    }
    Ok(())
}

fn pattern_matches(pattern: &str, subject: &str) -> Result<bool> {
    if let Ok(compiled) = Regex::new(pattern) {
        if compiled.is_match(subject) {
            return Ok(true);
        }
        let normalized = normalize_subject_for_regex(subject);
        if normalized != subject && compiled.is_match(&normalized) {
            return Ok(true);
        }
        return Ok(false);
    }

    // Support simple negative-lookahead forms used by fixture patterns:
    // ^(?!.*\bBAD\b).*<tail-regex>
    if let Some(rest) = pattern.strip_prefix("^(?!.*\\b")
        && let Some((blocked, tail)) = rest.split_once("\\b).*")
    {
        if subject
            .to_ascii_lowercase()
            .contains(&blocked.to_ascii_lowercase())
        {
            return Ok(false);
        }
        let tail_re = Regex::new(tail)?;
        return Ok(tail_re.is_match(subject));
    }

    let compiled = Regex::new(pattern)?;
    if compiled.is_match(subject) {
        return Ok(true);
    }
    let normalized = normalize_subject_for_regex(subject);
    Ok(normalized != subject && compiled.is_match(&normalized))
}

fn normalize_subject_for_regex(subject: &str) -> String {
    subject
        .replace("\\ ", " ")
        .replace(['\'', '"'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn matches_expected_event(expected: &str, output: &TaskRunOutput) -> bool {
    let normalized = expected.to_ascii_lowercase();
    if normalized.contains("safetyfloor") || normalized.contains("safety_floor") {
        return output.saw_safety_floor;
    }
    if normalized.contains("needsclarification") || normalized.contains("needs_clarification") {
        return output.saw_needs_clarification;
    }
    if normalized.contains("unknowncommand") || normalized.contains("unknown_command") {
        return output.saw_unknown_command;
    }
    if normalized.contains("validationincomplete") || normalized.contains("validation_incomplete") {
        return output.saw_validation_incomplete;
    }
    false
}

fn summarize(reports: &[TestReport]) -> Summary {
    let total = reports.len();
    let passed = reports.iter().filter(|r| r.passed).count();
    let failed = total.saturating_sub(passed);
    let mut by_category = BTreeMap::<String, CategorySummary>::new();
    let mut retrieval_total = 0usize;
    let mut retrieval_hit_top1 = 0usize;
    let mut retrieval_hit_top5 = 0usize;

    for report in reports {
        let entry = by_category.entry(report.category.clone()).or_default();
        entry.total += 1;
        if report.passed {
            entry.passed += 1;
        }

        if let Some(score) = &report.retrieval_score {
            retrieval_total += 1;
            if score.best_rank == Some(1) {
                retrieval_hit_top1 += 1;
            }
            if score.hit {
                retrieval_hit_top5 += 1;
            }
        }
    }

    Summary {
        total,
        passed,
        failed,
        by_category,
        retrieval_hit_rate_top1: if retrieval_total == 0 {
            0.0
        } else {
            retrieval_hit_top1 as f64 / retrieval_total as f64
        },
        retrieval_hit_rate_top5: if retrieval_total == 0 {
            0.0
        } else {
            retrieval_hit_top5 as f64 / retrieval_total as f64
        },
    }
}

fn print_human_summary(results: &HarnessResults, results_path: &Path) {
    println!(
        "benchmark_environment: os={} arch={} cpu=\"{}\" hardware_class={} logical_cpus={} total_memory_mb={:?} provider={} model={} profile={}",
        results.benchmark_environment.os,
        results.benchmark_environment.arch,
        results.benchmark_environment.cpu,
        results.benchmark_environment.hardware_class,
        results.benchmark_environment.logical_cpus,
        results.benchmark_environment.total_memory_mb,
        results.benchmark_environment.provider,
        results.benchmark_environment.model,
        results.benchmark_environment.performance_profile
    );
    println!("suite_version: {}", results.suite_version);
    println!("duration_secs: {}", results.duration_secs);
    println!(
        "summary: passed={} failed={} total={}",
        results.summary.passed, results.summary.failed, results.summary.total
    );
    println!(
        "retrieval hit rates: top1={:.2} top5={:.2}",
        results.summary.retrieval_hit_rate_top1, results.summary.retrieval_hit_rate_top5
    );
    println!("results_file: {}", results_path.display());
    if results.summary.failed > 0 {
        println!("failed tests:");
        for t in &results.tests {
            if !t.passed {
                println!(
                    "- {} [{}] {}",
                    t.id,
                    t.category,
                    t.error.as_deref().unwrap_or("")
                );
            }
        }
    }
}

fn load_perf_gates(path: &Path) -> Result<PerfGateConfig> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let parsed = toml::from_str::<PerfGateConfig>(&raw)
        .with_context(|| format!("parse perf gates {}", path.display()))?;
    Ok(parsed)
}

fn apply_hardware_gate_profile(
    gates: &PerfGateConfig,
    env: &BenchmarkEnvironment,
) -> PerfGateConfig {
    let mut effective = gates.clone();
    let profile = match classify_hardware_class(env) {
        HardwareClass::AppleM2ProMaxLocal => {
            effective.hardware_profiles.apple_m2_pro_max_local.clone()
        }
        HardwareClass::AppleM3ProLocal => effective.hardware_profiles.apple_m3_pro_local.clone(),
        HardwareClass::AppleM3MaxLocal => effective.hardware_profiles.apple_m3_max_local.clone(),
        HardwareClass::Other => None,
    };
    if let Some(profile) = profile {
        if profile.ttft_ms.is_some() {
            effective.ttft_ms = profile.ttft_ms;
        }
        if profile.model_load_ms.is_some() {
            effective.model_load_ms = profile.model_load_ms;
        }
        if profile.throughput_toks_per_sec.is_some() {
            effective.throughput_toks_per_sec = profile.throughput_toks_per_sec;
        }
        if profile.embedding_chunks_per_sec.is_some() {
            effective.embedding_chunks_per_sec = profile.embedding_chunks_per_sec;
        }
        if profile.observed_command_overhead_ms.is_some() {
            effective.observed_command_overhead_ms = profile.observed_command_overhead_ms;
        }
    }
    effective
}

fn print_perf_summary(reports: &[TestReport]) {
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.retrieval_latency_ms))
    {
        println!(
            "perf retrieval_latency_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.retrieval_50k_latency_ms))
    {
        println!(
            "perf retrieval_50k_latency_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.retrieval_50k_lexical_ms))
    {
        println!(
            "perf retrieval_50k_lexical_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.task_latency_ms)) {
        println!(
            "perf task_latency_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.ttft_ms)) {
        println!(
            "perf ttft_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.model_load_ms)) {
        println!(
            "perf model_load_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.model_resident_mb)) {
        println!(
            "perf model_resident_mb: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.indexer_resident_mb))
    {
        println!(
            "perf indexer_resident_mb: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.orchestration_resident_mb))
    {
        println!(
            "perf orchestration_resident_mb: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.rss_mb)) {
        println!(
            "perf rss_mb: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.kv_cache_mb)) {
        println!(
            "perf kv_cache_mb: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_float_stats(reports.iter().filter_map(|r| r.throughput_toks_per_sec))
    {
        println!(
            "perf throughput_toks_per_sec: p50={:.2} p95={:.2} min={:.2} max={:.2}",
            stats.p50, stats.p95, stats.min, stats.max
        );
    }
    let mut throughput_sources = BTreeMap::<String, u64>::new();
    for source in reports
        .iter()
        .filter_map(|r| r.throughput_source.as_deref())
        .map(str::to_string)
    {
        *throughput_sources.entry(source).or_default() += 1;
    }
    if !throughput_sources.is_empty() {
        println!("perf throughput_sources:");
        for (source, count) in throughput_sources {
            println!("  {}: {}", source, count);
        }
    }
    if let Some(stats) =
        collect_float_stats(reports.iter().filter_map(|r| r.embedding_chunks_per_sec))
    {
        println!(
            "perf embedding_chunks_per_sec: p50={:.2} p95={:.2} min={:.2} max={:.2}",
            stats.p50, stats.p95, stats.min, stats.max
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.full_reindex_ms)) {
        println!(
            "perf full_reindex_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.delta_reindex_ms)) {
        println!(
            "perf delta_reindex_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_latency_stats(reports.iter().filter_map(|r| r.index_disk_mb)) {
        println!(
            "perf index_disk_mb: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) = collect_float_stats(
        reports
            .iter()
            .filter_map(|r| r.ollama_orchestration_overhead_ms),
    ) {
        println!(
            "perf ollama_orchestration_overhead_ms: p50={:.2} p95={:.2} min={:.2} max={:.2}",
            stats.p50, stats.p95, stats.min, stats.max
        );
    }
    if let Some(stats) = collect_float_stats(
        reports
            .iter()
            .filter_map(|r| r.observed_command_overhead_ms),
    ) {
        println!(
            "perf observed_command_overhead_ms: p50={:.2} p95={:.2} min={:.2} max={:.2}",
            stats.p50, stats.p95, stats.min, stats.max
        );
    }
    if let Some(stats) = collect_float_stats(
        reports
            .iter()
            .filter_map(|r| r.observed_command_capture_overhead_ms),
    ) {
        println!(
            "perf observed_command_capture_overhead_ms: p50={:.2} p95={:.2} min={:.2} max={:.2}",
            stats.p50, stats.p95, stats.min, stats.max
        );
    }
    if let Some(stats) = collect_float_stats(reports.iter().filter_map(|r| r.idle_cpu_pct)) {
        println!(
            "perf idle_cpu_pct: p50={:.2} p95={:.2} min={:.2} max={:.2}",
            stats.p50, stats.p95, stats.min, stats.max
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.source_ledger_ref_count))
    {
        println!(
            "perf source_ledger_ref_count: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.source_ledger_overhead_ms))
    {
        println!(
            "perf source_ledger_overhead_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.tool_routing_overhead_ms))
    {
        println!(
            "perf tool_routing_overhead_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.pre_provider_overhead_ms))
    {
        println!(
            "perf pre_provider_overhead_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.planning_loop_overhead_ms))
    {
        println!(
            "perf planning_loop_overhead_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.web_extract_latency_ms))
    {
        println!(
            "perf web_extract_latency_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.web_extract_latency_p95_ms))
    {
        println!(
            "perf web_extract_latency_p95_ms: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    if let Some(stats) =
        collect_latency_stats(reports.iter().filter_map(|r| r.web_extract_rss_delta_mb))
    {
        println!(
            "perf web_extract_rss_delta_mb: p50={} p95={} max={}",
            stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
    let stage_stats = collect_stage_stats(reports);
    for (name, stats) in stage_stats {
        println!(
            "perf stage {}: p50={} p95={} max={}",
            name, stats.p50_ms, stats.p95_ms, stats.max_ms
        );
    }
}

fn check_perf_gates(reports: &[TestReport], gates: &PerfGateConfig) -> Option<String> {
    let mut violations = Vec::<String>::new();

    if let Some(gate) = gates.retrieval_latency_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.retrieval_latency_ms)) {
            Some(stats) => evaluate_gate("retrieval_latency_ms", stats, gate, &mut violations),
            None => violations.push("missing retrieval latency samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.retrieval_50k_latency_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.retrieval_50k_latency_ms)) {
            Some(stats) => evaluate_gate("retrieval_50k_latency_ms", stats, gate, &mut violations),
            None => violations
                .push("missing retrieval_50k_latency_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.retrieval_50k_lexical_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.retrieval_50k_lexical_ms)) {
            Some(stats) => evaluate_gate("retrieval_50k_lexical_ms", stats, gate, &mut violations),
            None => violations
                .push("missing retrieval_50k_lexical_ms samples for perf gate".to_string()),
        }
    }

    if let Some(gate) = gates.task_latency_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.task_latency_ms)) {
            Some(stats) => evaluate_gate("task_latency_ms", stats, gate, &mut violations),
            None => violations.push("missing task latency samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.ttft_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.ttft_ms)) {
            Some(stats) => evaluate_gate("ttft_ms", stats, gate, &mut violations),
            None => violations.push("missing ttft samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.model_load_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.model_load_ms)) {
            Some(stats) => evaluate_gate("model_load_ms", stats, gate, &mut violations),
            None => violations.push("missing model_load_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.model_resident_mb.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.model_resident_mb)) {
            Some(stats) => evaluate_gate("model_resident_mb", stats, gate, &mut violations),
            None => violations.push("missing model_resident_mb samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.rss_mb.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.rss_mb)) {
            Some(stats) => evaluate_gate("rss_mb", stats, gate, &mut violations),
            None => violations.push("missing rss_mb samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.indexer_resident_mb.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.indexer_resident_mb)) {
            Some(stats) => evaluate_gate("indexer_resident_mb", stats, gate, &mut violations),
            None => {
                violations.push("missing indexer_resident_mb samples for perf gate".to_string())
            }
        }
    }
    if let Some(gate) = gates.orchestration_resident_mb.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.orchestration_resident_mb)) {
            Some(stats) => evaluate_gate("orchestration_resident_mb", stats, gate, &mut violations),
            None => violations
                .push("missing orchestration_resident_mb samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.kv_cache_mb.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.kv_cache_mb)) {
            Some(stats) => evaluate_gate("kv_cache_mb", stats, gate, &mut violations),
            None => violations.push("missing kv_cache_mb samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.throughput_toks_per_sec.as_ref() {
        match collect_float_stats(reports.iter().filter_map(|r| r.throughput_toks_per_sec)) {
            Some(stats) => {
                evaluate_float_gate("throughput_toks_per_sec", stats, gate, &mut violations)
            }
            None => violations.push("missing throughput samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.embedding_chunks_per_sec.as_ref() {
        match collect_float_stats(reports.iter().filter_map(|r| r.embedding_chunks_per_sec)) {
            Some(stats) => {
                evaluate_float_gate("embedding_chunks_per_sec", stats, gate, &mut violations)
            }
            None => violations
                .push("missing embedding_chunks_per_sec samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.full_reindex_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.full_reindex_ms)) {
            Some(stats) => evaluate_gate("full_reindex_ms", stats, gate, &mut violations),
            None => violations.push("missing full_reindex_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.delta_reindex_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.delta_reindex_ms)) {
            Some(stats) => evaluate_gate("delta_reindex_ms", stats, gate, &mut violations),
            None => violations.push("missing delta_reindex_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.index_disk_mb.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.index_disk_mb)) {
            Some(stats) => evaluate_gate("index_disk_mb", stats, gate, &mut violations),
            None => violations.push("missing index_disk_mb samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.ollama_orchestration_overhead_ms.as_ref()
        && let Some(stats) = collect_float_stats(
            reports
                .iter()
                .filter_map(|r| r.ollama_orchestration_overhead_ms),
        )
    {
        evaluate_float_gate(
            "ollama_orchestration_overhead_ms",
            stats,
            gate,
            &mut violations,
        );
    }
    if let Some(gate) = gates.observed_command_overhead_ms.as_ref() {
        match collect_float_stats(
            reports
                .iter()
                .filter_map(|r| r.observed_command_overhead_ms),
        ) {
            Some(stats) => {
                evaluate_float_gate("observed_command_overhead_ms", stats, gate, &mut violations)
            }
            None => violations
                .push("missing observed_command_overhead_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.observed_command_capture_overhead_ms.as_ref() {
        match collect_float_stats(
            reports
                .iter()
                .filter_map(|r| r.observed_command_capture_overhead_ms),
        ) {
            Some(stats) => evaluate_float_gate(
                "observed_command_capture_overhead_ms",
                stats,
                gate,
                &mut violations,
            ),
            None => violations.push(
                "missing observed_command_capture_overhead_ms samples for perf gate".to_string(),
            ),
        }
    }
    if let Some(gate) = gates.idle_cpu_pct.as_ref() {
        match collect_float_stats(reports.iter().filter_map(|r| r.idle_cpu_pct)) {
            Some(stats) => evaluate_float_gate("idle_cpu_pct", stats, gate, &mut violations),
            None => violations.push("missing idle_cpu_pct samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.source_ledger_ref_count.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.source_ledger_ref_count)) {
            Some(stats) => evaluate_gate("source_ledger_ref_count", stats, gate, &mut violations),
            None => {
                violations.push("missing source_ledger_ref_count samples for perf gate".to_string())
            }
        }
    }
    if let Some(gate) = gates.source_ledger_overhead_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.source_ledger_overhead_ms)) {
            Some(stats) => evaluate_gate("source_ledger_overhead_ms", stats, gate, &mut violations),
            None => violations
                .push("missing source_ledger_overhead_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.tool_routing_overhead_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.tool_routing_overhead_ms)) {
            Some(stats) => evaluate_gate("tool_routing_overhead_ms", stats, gate, &mut violations),
            None => violations
                .push("missing tool_routing_overhead_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.pre_provider_overhead_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.pre_provider_overhead_ms)) {
            Some(stats) => evaluate_gate("pre_provider_overhead_ms", stats, gate, &mut violations),
            None => violations
                .push("missing pre_provider_overhead_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.planning_loop_overhead_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.planning_loop_overhead_ms)) {
            Some(stats) => evaluate_gate("planning_loop_overhead_ms", stats, gate, &mut violations),
            None => violations
                .push("missing planning_loop_overhead_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.web_extract_latency_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.web_extract_latency_ms)) {
            Some(stats) => evaluate_gate("web_extract_latency_ms", stats, gate, &mut violations),
            None => {
                violations.push("missing web_extract_latency_ms samples for perf gate".to_string())
            }
        }
    }
    if let Some(gate) = gates.web_extract_latency_p95_ms.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.web_extract_latency_p95_ms)) {
            Some(stats) => {
                evaluate_gate("web_extract_latency_p95_ms", stats, gate, &mut violations)
            }
            None => violations
                .push("missing web_extract_latency_p95_ms samples for perf gate".to_string()),
        }
    }
    if let Some(gate) = gates.web_extract_rss_delta_mb.as_ref() {
        match collect_latency_stats(reports.iter().filter_map(|r| r.web_extract_rss_delta_mb)) {
            Some(stats) => evaluate_gate("web_extract_rss_delta_mb", stats, gate, &mut violations),
            None => violations
                .push("missing web_extract_rss_delta_mb samples for perf gate".to_string()),
        }
    }

    let stage_stats = collect_stage_stats(reports);
    for (stage, gate) in &gates.stage_timings_ms {
        let effective_stats = if let Some(stats) = stage_stats.get(stage) {
            Some(*stats)
        } else if stage == "provider_orchestration_ms" {
            stage_stats.get("runtime_stub_provider_ms").copied()
        } else {
            None
        };
        match effective_stats {
            Some(stats) => evaluate_gate(
                &format!("stage_timings_ms.{stage}"),
                stats,
                gate,
                &mut violations,
            ),
            None => violations.push(format!(
                "missing stage timing samples for gate stage_timings_ms.{stage}"
            )),
        }
    }

    if violations.is_empty() {
        None
    } else {
        Some(violations.join("\n"))
    }
}

fn evaluate_gate(
    label: &str,
    stats: LatencyStats,
    gate: &PerfGateThreshold,
    violations: &mut Vec<String>,
) {
    if let Some(limit) = gate.p50_ms
        && stats.p50_ms > limit
    {
        violations.push(format!("{label} p50={} > {}", stats.p50_ms, limit));
    }
    if let Some(limit) = gate.p95_ms
        && stats.p95_ms > limit
    {
        violations.push(format!("{label} p95={} > {}", stats.p95_ms, limit));
    }
    if let Some(limit) = gate.max_ms
        && stats.max_ms > limit
    {
        violations.push(format!("{label} max={} > {}", stats.max_ms, limit));
    }
}

fn evaluate_float_gate(
    label: &str,
    stats: FloatStats,
    gate: &PerfGateFloatThreshold,
    violations: &mut Vec<String>,
) {
    if let Some(limit) = gate.p50_min
        && stats.p50 < limit
    {
        violations.push(format!("{label} p50={:.3} < {:.3}", stats.p50, limit));
    }
    if let Some(limit) = gate.p95_min
        && stats.p95 < limit
    {
        violations.push(format!("{label} p95={:.3} < {:.3}", stats.p95, limit));
    }
    if let Some(limit) = gate.min
        && stats.min < limit
    {
        violations.push(format!("{label} min={:.3} < {:.3}", stats.min, limit));
    }
    if let Some(limit) = gate.p50_max
        && stats.p50 > limit
    {
        violations.push(format!("{label} p50={:.3} > {:.3}", stats.p50, limit));
    }
    if let Some(limit) = gate.p95_max
        && stats.p95 > limit
    {
        violations.push(format!("{label} p95={:.3} > {:.3}", stats.p95, limit));
    }
    if let Some(limit) = gate.max
        && stats.max > limit
    {
        violations.push(format!("{label} max={:.3} > {:.3}", stats.max, limit));
    }
}

fn collect_stage_stats(reports: &[TestReport]) -> BTreeMap<String, LatencyStats> {
    let mut samples = BTreeMap::<String, Vec<u64>>::new();
    for report in reports {
        for (name, value) in &report.stage_timings_ms {
            samples.entry(name.clone()).or_default().push(*value);
        }
    }
    samples
        .into_iter()
        .filter_map(|(name, values)| collect_latency_stats(values.into_iter()).map(|s| (name, s)))
        .collect()
}

fn collect_latency_stats(values: impl Iterator<Item = u64>) -> Option<LatencyStats> {
    let mut sorted = values.collect::<Vec<_>>();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_unstable();
    Some(LatencyStats {
        p50_ms: percentile(&sorted, 0.50),
        p95_ms: percentile(&sorted, 0.95),
        max_ms: *sorted.last().unwrap_or(&0),
    })
}

fn collect_float_stats(values: impl Iterator<Item = f64>) -> Option<FloatStats> {
    let mut sorted = values.collect::<Vec<_>>();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(FloatStats {
        p50: percentile_f64(&sorted, 0.50),
        p95: percentile_f64(&sorted, 0.95),
        min: *sorted.first().unwrap_or(&0.0),
        max: *sorted.last().unwrap_or(&0.0),
    })
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let p = p.clamp(0.0, 1.0);
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn percentile_f64(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let p = p.clamp(0.0, 1.0);
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn default_capabilities() -> ShellCapabilities {
    ShellCapabilities {
        prompt_mode: true,
        session_mode: true,
        single_key_approval: true,
        edit_approval: true,
        execute_in_real_shell: true,
        command_completion_ack: true,
        stdout_stderr_capture: true,
        all_interactive_command_observation: true,
        terminal_context_capture: true,
        alias_capture: true,
        function_capture: true,
        builtin_inventory: true,
        shell_native_history: true,
    }
}

fn env_subset() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for key in ["PATH", "PWD", "TERM", "SHELL"] {
        if let Ok(v) = std::env::var(key) {
            out.insert(key.to_string(), v);
        }
    }
    out
}

fn env_subset_with_pwd(cwd: &Path) -> BTreeMap<String, String> {
    let mut env = env_subset();
    env.insert("PWD".to_string(), cwd.display().to_string());
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_rounds_to_expected_index() {
        let sorted = vec![10, 20, 30, 40, 50];
        assert_eq!(percentile(&sorted, 0.50), 30);
        assert_eq!(percentile(&sorted, 0.95), 50);
    }

    #[test]
    fn daemon_runtime_feature_args_local_stub_enables_runtime_stub() {
        assert_eq!(
            daemon_runtime_feature_args(HarnessProvider::Local, false),
            vec!["--no-default-features", "--features", "runtime-stub"]
        );
    }

    #[test]
    fn daemon_runtime_feature_args_local_real_uses_default_runtime() {
        assert!(daemon_runtime_feature_args(HarnessProvider::Local, true).is_empty());
    }

    #[test]
    fn daemon_runtime_feature_args_ollama_never_enables_runtime_stub() {
        assert!(daemon_runtime_feature_args(HarnessProvider::Ollama, false).is_empty());
    }

    #[test]
    fn parse_positive_u64_accepts_positive_numbers_only() {
        assert_eq!(parse_positive_u64(Some("30".to_string())), Some(30));
        assert_eq!(parse_positive_u64(Some(" 60 ".to_string())), Some(60));
        assert_eq!(parse_positive_u64(Some("0".to_string())), None);
        assert_eq!(parse_positive_u64(Some("-1".to_string())), None);
        assert_eq!(parse_positive_u64(Some("abc".to_string())), None);
        assert_eq!(parse_positive_u64(None), None);
    }

    #[test]
    fn select_timeout_secs_prefers_specific_then_global_then_default() {
        assert_eq!(
            select_timeout_secs(Some("240".to_string()), Some("300".to_string()), 180),
            240
        );
        assert_eq!(
            select_timeout_secs(Some("bad".to_string()), Some("300".to_string()), 180),
            300
        );
        assert_eq!(
            select_timeout_secs(Some("0".to_string()), Some("45".to_string()), 180),
            45
        );
        assert_eq!(
            select_timeout_secs(Some("bad".to_string()), Some("bad".to_string()), 180),
            180
        );
    }

    #[test]
    fn classify_hardware_class_detects_apple_profiles() {
        let mut env = BenchmarkEnvironment {
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            cpu: "Apple M3 Max".to_string(),
            hardware_class: "other".to_string(),
            logical_cpus: 12,
            total_memory_mb: Some(36_864),
            provider: "local".to_string(),
            model: "gemma-4-E4B-it-Q4_K_M.gguf".to_string(),
            performance_profile: "performance".to_string(),
        };
        assert_eq!(
            classify_hardware_class(&env),
            HardwareClass::AppleM3MaxLocal
        );
        env.cpu = "Apple M3 Pro".to_string();
        assert_eq!(
            classify_hardware_class(&env),
            HardwareClass::AppleM3ProLocal
        );
        env.cpu = "Apple M2 Pro".to_string();
        assert_eq!(
            classify_hardware_class(&env),
            HardwareClass::AppleM2ProMaxLocal
        );
        env.provider = "ollama".to_string();
        assert_eq!(classify_hardware_class(&env), HardwareClass::Other);
    }

    #[test]
    fn apply_hardware_gate_profile_overrides_expected_fields() {
        let env = BenchmarkEnvironment {
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            cpu: "Apple M2 Max".to_string(),
            hardware_class: "apple_m2_pro_max_local".to_string(),
            logical_cpus: 10,
            total_memory_mb: Some(32_768),
            provider: "local".to_string(),
            model: "gemma-4-E4B-it-Q4_K_M.gguf".to_string(),
            performance_profile: "performance".to_string(),
        };
        let base = PerfGateConfig {
            retrieval_latency_ms: None,
            retrieval_50k_latency_ms: None,
            retrieval_50k_lexical_ms: None,
            task_latency_ms: None,
            ttft_ms: Some(PerfGateThreshold {
                p50_ms: Some(800),
                p95_ms: Some(1200),
                max_ms: Some(1500),
            }),
            model_load_ms: None,
            model_resident_mb: None,
            rss_mb: None,
            indexer_resident_mb: None,
            orchestration_resident_mb: None,
            kv_cache_mb: None,
            full_reindex_ms: None,
            delta_reindex_ms: None,
            index_disk_mb: None,
            source_ledger_ref_count: None,
            source_ledger_overhead_ms: None,
            tool_routing_overhead_ms: None,
            pre_provider_overhead_ms: None,
            planning_loop_overhead_ms: None,
            web_extract_latency_ms: None,
            web_extract_latency_p95_ms: None,
            web_extract_rss_delta_mb: None,
            throughput_toks_per_sec: None,
            embedding_chunks_per_sec: None,
            ollama_orchestration_overhead_ms: None,
            observed_command_overhead_ms: Some(PerfGateFloatThreshold {
                p50_min: None,
                p95_min: None,
                min: None,
                p50_max: Some(20.0),
                p95_max: Some(25.0),
                max: Some(50.0),
            }),
            observed_command_capture_overhead_ms: None,
            idle_cpu_pct: None,
            stage_timings_ms: BTreeMap::new(),
            hardware_profiles: PerfGateHardwareProfiles {
                apple_m2_pro_max_local: Some(PerfGateHardwareProfile {
                    ttft_ms: Some(PerfGateThreshold {
                        p50_ms: Some(400),
                        p95_ms: Some(1500),
                        max_ms: Some(1500),
                    }),
                    model_load_ms: None,
                    throughput_toks_per_sec: None,
                    embedding_chunks_per_sec: None,
                    observed_command_overhead_ms: Some(PerfGateFloatThreshold {
                        p50_min: None,
                        p95_min: None,
                        min: None,
                        p50_max: Some(10.0),
                        p95_max: Some(25.0),
                        max: Some(50.0),
                    }),
                }),
                apple_m3_pro_local: None,
                apple_m3_max_local: None,
            },
        };

        let applied = apply_hardware_gate_profile(&base, &env);
        assert_eq!(
            applied.ttft_ms.and_then(|g| g.p50_ms),
            Some(400),
            "ttft should use strict m2 profile override"
        );
        assert_eq!(
            applied.observed_command_overhead_ms.and_then(|g| g.p50_max),
            Some(10.0),
            "observer overhead should use strict m2 profile override"
        );
    }

    #[test]
    fn perf_gate_passes_within_thresholds() {
        let report = TestReport {
            id: "t".to_string(),
            mode: "execute".to_string(),
            category: "cat".to_string(),
            passed: true,
            duration_ms: 10,
            retrieval_score: None,
            retrieval_latency_ms: Some(80),
            retrieval_50k_latency_ms: Some(30),
            retrieval_50k_lexical_ms: Some(8),
            task_latency_ms: Some(120),
            ttft_ms: Some(100),
            throughput_toks_per_sec: Some(35.0),
            throughput_source: Some("provider_reported_tokens".to_string()),
            model_load_ms: Some(500),
            model_resident_mb: Some(4200),
            indexer_resident_mb: Some(180),
            orchestration_resident_mb: Some(220),
            last_task_prompt_tokens: Some(120),
            last_task_completion_tokens: Some(300),
            last_task_usage_reported: Some(true),
            embedding_chunks_per_sec: Some(500.0),
            full_reindex_ms: Some(4_000),
            delta_reindex_ms: Some(2_000),
            index_disk_mb: Some(220),
            ollama_orchestration_overhead_ms: Some(12.0),
            observed_command_overhead_ms: Some(1.0),
            observed_command_capture_overhead_ms: Some(4.0),
            idle_cpu_pct: Some(0.3),
            source_ledger_ref_count: Some(12),
            source_ledger_overhead_ms: Some(2),
            tool_routing_overhead_ms: Some(3),
            pre_provider_overhead_ms: Some(20),
            planning_loop_overhead_ms: Some(50),
            web_extract_latency_ms: Some(110),
            web_extract_latency_p95_ms: Some(180),
            web_extract_rss_delta_mb: Some(22),
            stage_timings_ms: BTreeMap::from([
                ("classify_ms".to_string(), 1),
                ("task_total_ms".to_string(), 120),
            ]),
            rss_mb: Some(900),
            kv_cache_mb: Some(120),
            proposed_command: None,
            exit_status: None,
            error: None,
        };
        let gates = PerfGateConfig {
            retrieval_latency_ms: Some(PerfGateThreshold {
                p50_ms: Some(100),
                p95_ms: Some(200),
                max_ms: Some(500),
            }),
            retrieval_50k_latency_ms: Some(PerfGateThreshold {
                p50_ms: Some(35),
                p95_ms: Some(50),
                max_ms: Some(100),
            }),
            retrieval_50k_lexical_ms: Some(PerfGateThreshold {
                p50_ms: Some(10),
                p95_ms: Some(20),
                max_ms: Some(50),
            }),
            task_latency_ms: Some(PerfGateThreshold {
                p50_ms: Some(150),
                p95_ms: Some(250),
                max_ms: Some(600),
            }),
            ttft_ms: Some(PerfGateThreshold {
                p50_ms: Some(120),
                p95_ms: Some(180),
                max_ms: Some(500),
            }),
            model_load_ms: Some(PerfGateThreshold {
                p50_ms: Some(2_000),
                p95_ms: Some(8_000),
                max_ms: Some(30_000),
            }),
            model_resident_mb: Some(PerfGateThreshold {
                p50_ms: Some(4_800),
                p95_ms: Some(5_200),
                max_ms: Some(5_400),
            }),
            rss_mb: Some(PerfGateThreshold {
                p50_ms: Some(1000),
                p95_ms: Some(1200),
                max_ms: Some(1500),
            }),
            indexer_resident_mb: Some(PerfGateThreshold {
                p50_ms: Some(220),
                p95_ms: Some(260),
                max_ms: Some(300),
            }),
            orchestration_resident_mb: Some(PerfGateThreshold {
                p50_ms: Some(240),
                p95_ms: Some(280),
                max_ms: Some(320),
            }),
            kv_cache_mb: Some(PerfGateThreshold {
                p50_ms: Some(200),
                p95_ms: Some(300),
                max_ms: Some(400),
            }),
            full_reindex_ms: Some(PerfGateThreshold {
                p50_ms: Some(60_000),
                p95_ms: Some(300_000),
                max_ms: Some(1_800_000),
            }),
            delta_reindex_ms: Some(PerfGateThreshold {
                p50_ms: Some(5_000),
                p95_ms: Some(30_000),
                max_ms: Some(120_000),
            }),
            index_disk_mb: Some(PerfGateThreshold {
                p50_ms: Some(250),
                p95_ms: Some(300),
                max_ms: Some(600),
            }),
            source_ledger_ref_count: Some(PerfGateThreshold {
                p50_ms: Some(64),
                p95_ms: Some(128),
                max_ms: Some(256),
            }),
            source_ledger_overhead_ms: Some(PerfGateThreshold {
                p50_ms: Some(5),
                p95_ms: Some(20),
                max_ms: Some(50),
            }),
            tool_routing_overhead_ms: Some(PerfGateThreshold {
                p50_ms: Some(10),
                p95_ms: Some(25),
                max_ms: Some(75),
            }),
            pre_provider_overhead_ms: Some(PerfGateThreshold {
                p50_ms: Some(75),
                p95_ms: Some(200),
                max_ms: Some(1_000),
            }),
            planning_loop_overhead_ms: Some(PerfGateThreshold {
                p50_ms: Some(150),
                p95_ms: Some(500),
                max_ms: Some(1_500),
            }),
            web_extract_latency_ms: Some(PerfGateThreshold {
                p50_ms: Some(250),
                p95_ms: Some(300),
                max_ms: Some(600),
            }),
            web_extract_latency_p95_ms: Some(PerfGateThreshold {
                p50_ms: Some(300),
                p95_ms: Some(350),
                max_ms: Some(700),
            }),
            web_extract_rss_delta_mb: Some(PerfGateThreshold {
                p50_ms: Some(50),
                p95_ms: Some(80),
                max_ms: Some(120),
            }),
            throughput_toks_per_sec: Some(PerfGateFloatThreshold {
                p50_min: Some(25.0),
                p95_min: Some(20.0),
                min: Some(10.0),
                p50_max: None,
                p95_max: None,
                max: None,
            }),
            embedding_chunks_per_sec: Some(PerfGateFloatThreshold {
                p50_min: Some(100.0),
                p95_min: Some(80.0),
                min: Some(50.0),
                p50_max: None,
                p95_max: None,
                max: None,
            }),
            ollama_orchestration_overhead_ms: Some(PerfGateFloatThreshold {
                p50_min: None,
                p95_min: None,
                min: None,
                p50_max: Some(75.0),
                p95_max: Some(150.0),
                max: Some(300.0),
            }),
            observed_command_overhead_ms: Some(PerfGateFloatThreshold {
                p50_min: None,
                p95_min: None,
                min: None,
                p50_max: Some(10.0),
                p95_max: Some(25.0),
                max: Some(50.0),
            }),
            observed_command_capture_overhead_ms: Some(PerfGateFloatThreshold {
                p50_min: None,
                p95_min: None,
                min: None,
                p50_max: Some(25.0),
                p95_max: Some(50.0),
                max: Some(100.0),
            }),
            idle_cpu_pct: Some(PerfGateFloatThreshold {
                p50_min: None,
                p95_min: None,
                min: None,
                p50_max: Some(1.0),
                p95_max: Some(2.0),
                max: Some(4.0),
            }),
            stage_timings_ms: BTreeMap::from([(
                "task_total_ms".to_string(),
                PerfGateThreshold {
                    p50_ms: Some(130),
                    p95_ms: Some(200),
                    max_ms: Some(700),
                },
            )]),
            hardware_profiles: PerfGateHardwareProfiles::default(),
        };

        assert!(check_perf_gates(&[report], &gates).is_none());
    }
}
