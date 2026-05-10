use anyhow::{Context, Result, anyhow};
use base64::Engine;
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use termlm_config::load_or_create;
use termlm_protocol::{
    Ack, AliasDef, ClientMessage, FunctionDef, MAX_FRAME_BYTES, ObservedCommand, RegisterShell,
    ReindexMode, RetrieveRequest, ServerMessage, ShellCapabilities, ShellContext, ShellKind,
    StartTask, UserDecision, UserResponse,
};
use tokio::net::UnixStream;
use tokio_serde::formats::Json;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use uuid::Uuid;

mod upgrade;

#[derive(Debug, Parser)]
#[command(name = "termlm")]
#[command(bin_name = "termlm")]
#[command(about = "termlm helper CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(alias = "update")]
    Upgrade {
        #[arg(long, hide = true)]
        repo: Option<String>,
        #[arg(long, hide = true)]
        tag: Option<String>,
    },
    Status {
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },
    ReloadConfig,
    Stop,
    Ping,
    Init {
        #[command(subcommand)]
        shell: InitCommand,
    },
    Doctor {
        #[arg(long, default_value_t = false)]
        strict: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Uninstall {
        #[arg(long, default_value_t = false)]
        yes: bool,
        #[arg(long, default_value_t = false)]
        keep_models: bool,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    #[command(hide = true)]
    Bridge,
    #[command(hide = true)]
    Helper {
        #[arg(long)]
        ready_file: String,
    },
    #[command(hide = true)]
    RegisterShell,
    #[command(hide = true)]
    UnregisterShell {
        #[arg(long)]
        shell_id: String,
    },
    #[command(hide = true)]
    SendShellContext {
        #[arg(long)]
        shell_id: String,
        #[arg(long, default_value = "")]
        context_hash: String,
        #[arg(long = "alias")]
        alias: Vec<String>,
        #[arg(long = "function")]
        function: Vec<String>,
        #[arg(long = "builtin")]
        builtin: Vec<String>,
    },
    #[command(name = "observe-command")]
    #[command(hide = true)]
    Observe {
        #[arg(long)]
        shell_id: String,
        #[arg(long, default_value_t = 0)]
        command_seq: u64,
        #[arg(long)]
        raw_command: String,
        #[arg(long)]
        expanded_command: String,
        #[arg(long)]
        cwd_before: String,
        #[arg(long)]
        cwd_after: String,
        #[arg(long)]
        exit_status: i32,
        #[arg(long)]
        started_at_ms: Option<i64>,
        #[arg(long, default_value_t = 0)]
        duration_ms: u64,
        #[arg(long)]
        stdout_b64: Option<String>,
        #[arg(long, default_value_t = false)]
        stdout_truncated: bool,
        #[arg(long)]
        stderr_b64: Option<String>,
        #[arg(long, default_value_t = false)]
        stderr_truncated: bool,
        #[arg(long, default_value = "none")]
        output_capture_status: String,
    },
    Reindex {
        #[arg(long, value_enum, default_value = "delta", conflicts_with_all = ["full", "compact"])]
        mode: ReindexModeArg,
        #[arg(long, default_value_t = false, conflicts_with = "compact")]
        full: bool,
        #[arg(long, default_value_t = false, conflicts_with = "full")]
        compact: bool,
    },
    #[command(hide = true)]
    Retrieve {
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        top_k: Option<u32>,
    },
    #[command(hide = true)]
    RunTask {
        #[arg(long)]
        prompt: String,
        #[arg(long, default_value = "?")]
        mode: String,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long)]
        shell_id: Option<String>,
    },
    #[command(hide = true)]
    RespondTask {
        #[arg(long)]
        task_id: String,
        #[arg(long, value_enum)]
        decision: DecisionArg,
        #[arg(long)]
        edited_command: Option<String>,
        #[arg(long)]
        text: Option<String>,
    },
    #[command(hide = true)]
    AckTask {
        #[arg(long)]
        task_id: String,
        #[arg(long, default_value_t = 1)]
        command_seq: u64,
        #[arg(long)]
        command: String,
        #[arg(long)]
        cwd_before: String,
        #[arg(long)]
        cwd_after: String,
        #[arg(long)]
        exit_status: i32,
        #[arg(long)]
        started_at_ms: Option<i64>,
        #[arg(long)]
        stdout_b64: Option<String>,
        #[arg(long, default_value_t = false)]
        stdout_truncated: bool,
        #[arg(long)]
        stderr_b64: Option<String>,
        #[arg(long, default_value_t = false)]
        stderr_truncated: bool,
        #[arg(long, default_value_t = 0)]
        elapsed_ms: u64,
    },
}

#[derive(Debug, Subcommand)]
enum InitCommand {
    Zsh {
        #[arg(long, default_value_t = false)]
        print_only: bool,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Debug, Clone, ValueEnum)]
enum DecisionArg {
    Approved,
    Rejected,
    Edited,
    ApproveAll,
    Abort,
    Clarification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReindexModeArg {
    Delta,
    Full,
    Compact,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum BridgeCommand {
    StartTask {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<Uuid>,
        mode: String,
        prompt: String,
        cwd: String,
    },
    UserResponse {
        task_id: Uuid,
        decision: UserDecision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edited_command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Ack {
        task_id: Uuid,
        #[serde(default = "default_command_seq")]
        command_seq: u64,
        command: String,
        cwd_before: String,
        cwd_after: String,
        exit_status: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at_ms: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_b64: Option<String>,
        #[serde(default)]
        stdout_truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_b64: Option<String>,
        #[serde(default)]
        stderr_truncated: bool,
        #[serde(default)]
        elapsed_ms: u64,
    },
    ObserveCommand {
        #[serde(default = "default_command_seq")]
        command_seq: u64,
        raw_command: String,
        expanded_command: String,
        cwd_before: String,
        cwd_after: String,
        exit_status: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at_ms: Option<i64>,
        #[serde(default)]
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_b64: Option<String>,
        #[serde(default)]
        stdout_truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_b64: Option<String>,
        #[serde(default)]
        stderr_truncated: bool,
        #[serde(default = "default_capture_status")]
        output_capture_status: String,
    },
    ShellContext {
        #[serde(default)]
        context_hash: String,
        #[serde(default)]
        aliases: Vec<AliasDef>,
        #[serde(default)]
        functions: Vec<FunctionDef>,
        #[serde(default)]
        builtins: Vec<String>,
    },
    Reindex {
        #[serde(default = "default_reindex_mode")]
        mode: ReindexModeArg,
    },
    Retrieve {
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        top_k: Option<u32>,
    },
    ProviderHealth,
    Status,
    Ping,
    Shutdown,
    UnregisterShell,
}

const fn default_command_seq() -> u64 {
    1
}

fn default_capture_status() -> String {
    "none".to_string()
}

const fn default_reindex_mode() -> ReindexModeArg {
    ReindexModeArg::Delta
}

const ENV_SHELL_PID: &str = "TERMLM_SHELL_PID";
const ENV_SHELL_TTY: &str = "TERMLM_SHELL_TTY";
const ENV_SHELL_KIND: &str = "TERMLM_SHELL_KIND";
const ENV_SHELL_VERSION: &str = "TERMLM_SHELL_VERSION";
const ENV_ADAPTER_VERSION: &str = "TERMLM_ADAPTER_VERSION";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellRegistrationMeta {
    shell_pid: u32,
    tty: String,
    shell_kind: ShellKind,
    shell_version: String,
    adapter_version: String,
}

fn env_nonempty_with<F>(lookup: &F, name: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(name)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn resolve_shell_registration_meta_with<F>(lookup: F) -> ShellRegistrationMeta
where
    F: Fn(&str) -> Option<String>,
{
    let shell_pid = env_nonempty_with(&lookup, ENV_SHELL_PID)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or_else(std::process::id);
    let tty = env_nonempty_with(&lookup, ENV_SHELL_TTY)
        .or_else(|| env_nonempty_with(&lookup, "TTY"))
        .unwrap_or_else(|| "unknown".to_string());
    let shell_kind = detect_shell_kind_with(&lookup);
    let shell_version = env_nonempty_with(&lookup, ENV_SHELL_VERSION)
        .unwrap_or_else(|| detect_shell_version_with(&lookup, &shell_kind));
    let adapter_version = env_nonempty_with(&lookup, ENV_ADAPTER_VERSION)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    ShellRegistrationMeta {
        shell_pid,
        tty,
        shell_kind,
        shell_version,
        adapter_version,
    }
}

fn resolve_shell_registration_meta() -> ShellRegistrationMeta {
    resolve_shell_registration_meta_with(|name| std::env::var(name).ok())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Upgrade { repo, tag } = &cli.cmd {
        return upgrade::run_upgrade(repo.clone(), tag.clone()).await;
    }

    let cfg = load_or_create(None)?.config;
    let stream_idle_timeout =
        Duration::from_secs((cfg.inference.token_idle_timeout_secs + 5).max(15));

    if matches!(cli.cmd, Command::ReloadConfig) {
        let pid_path = resolve_runtime_path(&cfg.daemon.pid_file);
        signal_config_reload(&pid_path)?;
        println!("reload signal sent");
        return Ok(());
    }
    if let Command::Init { shell } = &cli.cmd {
        match shell {
            InitCommand::Zsh { print_only, force } => run_init_zsh(*print_only, *force)?,
        }
        return Ok(());
    }
    if let Command::Doctor { strict, json } = &cli.cmd {
        run_doctor(&cfg, *strict, *json).await?;
        return Ok(());
    }
    if let Command::Uninstall {
        yes,
        keep_models,
        dry_run,
    } = &cli.cmd
    {
        run_uninstall(&cfg, *yes, *keep_models, *dry_run)?;
        return Ok(());
    }

    let socket = resolve_socket_path(&cfg.daemon.socket_path);

    let stream = UnixStream::connect(&socket)
        .await
        .with_context(|| format!("cannot connect to {}", socket.display()))?;

    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_BYTES)
        .new_codec();
    let framed = Framed::new(stream, codec);
    let mut transport =
        tokio_serde::Framed::new(framed, Json::<ServerMessage, ClientMessage>::default());

    match cli.cmd {
        Command::Upgrade { .. } => unreachable!("upgrade is handled before config/socket setup"),
        Command::ReloadConfig => unreachable!("reload-config is handled before socket connect"),
        Command::Init { .. } => unreachable!("init is handled before socket connect"),
        Command::Doctor { .. } => unreachable!("doctor is handled before socket connect"),
        Command::Uninstall { .. } => {
            unreachable!("uninstall is handled before socket connect")
        }
        Command::Status { verbose } => {
            transport.send(ClientMessage::Status).await?;
            if let Some(Ok(msg)) = transport.next().await {
                match msg {
                    ServerMessage::StatusReport {
                        pid,
                        uptime_secs,
                        socket_path,
                        provider,
                        model,
                        endpoint,
                        provider_healthy,
                        provider_health_latency_ms,
                        provider_context_window,
                        provider_structured_mode,
                        provider_supports_native_tool_calls,
                        provider_supports_json_mode,
                        provider_remote,
                        model_load_ms,
                        rss_mb,
                        model_resident_mb,
                        indexer_resident_mb,
                        orchestration_resident_mb,
                        kv_cache_mb,
                        last_task_prompt_tokens,
                        last_task_completion_tokens,
                        last_task_usage_reported,
                        last_task_source_refs,
                        last_task_source_ledger,
                        stage_timings_ms,
                        active_shells,
                        active_tasks,
                        index_progress,
                        web,
                        ..
                    } => {
                        println!("pid: {pid}");
                        println!("socket_path: {socket_path}");
                        println!("provider: {provider}");
                        println!("model: {model}");
                        if let Some(endpoint) = endpoint {
                            println!("endpoint: {endpoint}");
                        }
                        println!("provider_healthy: {provider_healthy}");
                        if let Some(latency_ms) = provider_health_latency_ms {
                            println!("provider_health_latency_ms: {latency_ms}");
                        }
                        println!("provider_context_window: {provider_context_window}");
                        println!("provider_structured_mode: {provider_structured_mode}");
                        println!(
                            "provider_tooling: native_tool_calls={} json_mode={}",
                            provider_supports_native_tool_calls, provider_supports_json_mode
                        );
                        println!("provider_remote: {provider_remote}");
                        println!("uptime_secs: {uptime_secs}");
                        println!("model_load_ms: {model_load_ms}");
                        println!("rss_mb: {rss_mb}");
                        println!("model_resident_mb: {model_resident_mb}");
                        println!("indexer_resident_mb: {indexer_resident_mb}");
                        println!("orchestration_resident_mb: {orchestration_resident_mb}");
                        println!("kv_cache_mb: {kv_cache_mb}");
                        if let Some(prompt_tokens) = last_task_prompt_tokens {
                            println!("last_task_prompt_tokens: {prompt_tokens}");
                        }
                        if let Some(completion_tokens) = last_task_completion_tokens {
                            println!("last_task_completion_tokens: {completion_tokens}");
                        }
                        println!("last_task_usage_reported: {last_task_usage_reported}");
                        println!("last_task_source_refs: {last_task_source_refs}");
                        println!("active_shells: {active_shells}");
                        println!("active_tasks: {active_tasks}");
                        println!(
                            "index_progress: phase={} percent={:.1}",
                            index_progress.phase, index_progress.percent
                        );
                        println!("web: enabled={} provider={}", web.enabled, web.provider);
                        if verbose && !last_task_source_ledger.is_empty() {
                            println!("last_task_source_ledger:");
                            for r in last_task_source_ledger {
                                let mut line = format!(
                                    "  [{}] {} hash={} redacted={} truncated={} at={}",
                                    r.source_type,
                                    r.source_id,
                                    r.hash,
                                    r.redacted,
                                    r.truncated,
                                    r.observed_at.to_rfc3339(),
                                );
                                if let Some(section) = r.section {
                                    line.push_str(&format!(" section={section}"));
                                }
                                if let Some(detail) = r.detail {
                                    line.push_str(&format!(
                                        " detail={}",
                                        detail.replace('\n', " ")
                                    ));
                                }
                                println!("{line}");
                            }
                        }
                        if verbose && !stage_timings_ms.is_empty() {
                            println!("stage_timings_ms:");
                            for (name, ms) in stage_timings_ms {
                                println!("  {name}: {ms}");
                            }
                        }
                    }
                    other => println!("unexpected response: {other:?}"),
                }
            }
        }
        Command::Stop => {
            transport.send(ClientMessage::Shutdown).await?;
            println!("shutdown requested");
        }
        Command::Ping => {
            transport.send(ClientMessage::Ping).await?;
            if let Some(Ok(msg)) = transport.next().await {
                println!("{msg:?}");
            }
        }
        Command::Bridge => {
            run_bridge(&mut transport).await?;
        }
        Command::Helper { ready_file } => {
            let shell_id = register_shell(&mut transport).await?;
            let ready_path = std::path::PathBuf::from(ready_file);
            if let Some(parent) = ready_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            let tmp = ready_path.with_extension("tmp");
            std::fs::write(&tmp, format!("{shell_id}\n"))
                .with_context(|| format!("write {}", tmp.display()))?;
            std::fs::rename(&tmp, &ready_path)
                .with_context(|| format!("rename {} -> {}", tmp.display(), ready_path.display()))?;

            loop {
                match transport.next().await {
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        return Err(e).context("helper daemon stream failure");
                    }
                    None => {
                        return Err(anyhow!("helper lost daemon connection"));
                    }
                }
            }
        }
        Command::RegisterShell => {
            let shell_id = register_shell(&mut transport).await?;
            emit_machine_event(serde_json::json!({
                "event": "shell_registered",
                "shell_id": shell_id,
            }));
        }
        Command::UnregisterShell { shell_id } => {
            transport
                .send(ClientMessage::UnregisterShell {
                    shell_id: Uuid::parse_str(&shell_id)?,
                })
                .await?;
        }
        Command::SendShellContext {
            shell_id,
            context_hash,
            alias,
            function,
            builtin,
        } => {
            let shell_id = Uuid::parse_str(&shell_id)?;
            let aliases = alias
                .into_iter()
                .filter_map(|row| {
                    let (name, expansion) = row.split_once('=')?;
                    Some(AliasDef {
                        name: name.to_string(),
                        expansion: expansion.to_string(),
                    })
                })
                .collect::<Vec<_>>();
            let functions = function
                .into_iter()
                .filter_map(|row| {
                    let (name, body_prefix) = row.split_once('|')?;
                    Some(FunctionDef {
                        name: name.to_string(),
                        body_prefix: body_prefix.to_string(),
                    })
                })
                .collect::<Vec<_>>();

            transport
                .send(ClientMessage::ShellContext {
                    payload: ShellContext {
                        shell_id,
                        shell_kind: detect_shell_kind(),
                        context_hash,
                        aliases,
                        functions,
                        builtins: builtin,
                    },
                })
                .await?;
        }
        Command::Observe {
            shell_id,
            command_seq,
            raw_command,
            expanded_command,
            cwd_before,
            cwd_after,
            exit_status,
            started_at_ms,
            duration_ms,
            stdout_b64,
            stdout_truncated,
            stderr_b64,
            stderr_truncated,
            output_capture_status,
        } => {
            let started_at = parse_started_at_ms(started_at_ms)?;
            transport
                .send(ClientMessage::ObservedCommand {
                    payload: ObservedCommand {
                        shell_id: Uuid::parse_str(&shell_id)?,
                        command_seq,
                        raw_command,
                        expanded_command,
                        cwd_before,
                        cwd_after,
                        started_at,
                        exit_status,
                        duration_ms,
                        stdout_b64,
                        stdout_truncated,
                        stderr_b64,
                        stderr_truncated,
                        output_capture_status,
                    },
                })
                .await?;
        }
        Command::Reindex {
            mode,
            full,
            compact,
        } => {
            let resolved_mode = if full {
                ReindexModeArg::Full
            } else if compact {
                ReindexModeArg::Compact
            } else {
                mode
            };
            transport
                .send(ClientMessage::Reindex {
                    mode: match resolved_mode {
                        ReindexModeArg::Delta => ReindexMode::Delta,
                        ReindexModeArg::Full => ReindexMode::Full,
                        ReindexModeArg::Compact => ReindexMode::Compact,
                    },
                })
                .await?;
            if let Some(Ok(msg)) = transport.next().await {
                println!("{msg:?}");
            }
        }
        Command::Retrieve { prompt, top_k } => {
            transport
                .send(ClientMessage::Retrieve {
                    payload: RetrieveRequest { prompt, top_k },
                })
                .await?;
            if let Some(Ok(msg)) = transport.next().await {
                println!("{msg:?}");
            }
        }
        Command::RunTask {
            prompt,
            mode,
            cwd,
            shell_id,
        } => {
            let shell_id = if let Some(id) = shell_id {
                Uuid::parse_str(&id)?
            } else {
                register_shell(&mut transport).await?
            };
            let task_id = Uuid::now_v7();
            let start = StartTask {
                task_id,
                shell_id,
                shell_kind: detect_shell_kind(),
                shell_version: detect_shell_version(),
                mode,
                prompt,
                cwd,
                env_subset: env_subset(),
            };
            transport
                .send(ClientMessage::StartTask { payload: start })
                .await?;
            emit_machine_event(serde_json::json!({
                "event": "task_started",
                "task_id": task_id,
            }));

            if wait_task_events(&mut transport, task_id, stream_idle_timeout).await? {
                transport
                    .send(ClientMessage::UserResponse {
                        payload: UserResponse {
                            task_id,
                            decision: UserDecision::Abort,
                            edited_command: None,
                            text: Some("client_timeout".to_string()),
                        },
                    })
                    .await
                    .ok();
            }
        }
        Command::RespondTask {
            task_id,
            decision,
            edited_command,
            text,
        } => {
            let parsed = Uuid::parse_str(&task_id)?;
            let user_decision = match decision {
                DecisionArg::Approved => UserDecision::Approved,
                DecisionArg::Rejected => UserDecision::Rejected,
                DecisionArg::Edited => UserDecision::Edited,
                DecisionArg::ApproveAll => UserDecision::ApproveAllInTask,
                DecisionArg::Abort => UserDecision::Abort,
                DecisionArg::Clarification => UserDecision::Clarification,
            };
            transport
                .send(ClientMessage::UserResponse {
                    payload: UserResponse {
                        task_id: parsed,
                        decision: user_decision.clone(),
                        edited_command,
                        text,
                    },
                })
                .await?;

            if matches!(
                user_decision,
                UserDecision::Rejected | UserDecision::Abort | UserDecision::Clarification
            ) {
                if wait_task_events(&mut transport, parsed, stream_idle_timeout).await? {
                    transport
                        .send(ClientMessage::UserResponse {
                            payload: UserResponse {
                                task_id: parsed,
                                decision: UserDecision::Abort,
                                edited_command: None,
                                text: Some("client_timeout".to_string()),
                            },
                        })
                        .await
                        .ok();
                }
            } else {
                drain_immediate_events(&mut transport, parsed, Duration::from_millis(250)).await?;
            }
        }
        Command::AckTask {
            task_id,
            command_seq,
            command,
            cwd_before,
            cwd_after,
            exit_status,
            started_at_ms,
            stdout_b64,
            stdout_truncated,
            stderr_b64,
            stderr_truncated,
            elapsed_ms,
        } => {
            let parsed = Uuid::parse_str(&task_id)?;
            let started_at = parse_started_at_ms(started_at_ms)?;
            transport
                .send(ClientMessage::Ack {
                    payload: Ack {
                        task_id: parsed,
                        command_seq,
                        executed_command: command,
                        cwd_before,
                        cwd_after,
                        started_at,
                        exit_status,
                        stdout_b64,
                        stdout_truncated,
                        stderr_b64,
                        stderr_truncated,
                        redactions_applied: Vec::new(),
                        elapsed_ms,
                    },
                })
                .await?;

            let _ = wait_task_events(&mut transport, parsed, stream_idle_timeout).await?;
        }
    }

    Ok(())
}

async fn run_bridge(
    transport: &mut tokio_serde::Framed<
        Framed<UnixStream, LengthDelimitedCodec>,
        ServerMessage,
        ClientMessage,
        Json<ServerMessage, ClientMessage>,
    >,
) -> Result<()> {
    let shell_id = register_shell(transport).await?;
    emit_machine_event(serde_json::json!({
        "event": "shell_registered",
        "shell_id": shell_id,
    }));

    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<Option<String>>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = stdin_tx.send(None);
                    break;
                }
                Ok(_) => {
                    if line.ends_with('\n') {
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }
                    if stdin_tx.send(Some(line)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = stdin_tx.send(None);
                    break;
                }
            }
        }
    });

    loop {
        tokio::select! {
            maybe_line = stdin_rx.recv() => {
                match maybe_line {
                    Some(Some(line)) => {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<BridgeCommand>(line) {
                            Ok(command) => {
                                handle_bridge_command(transport, shell_id, command).await?;
                            }
                            Err(e) => {
                                emit_machine_event(serde_json::json!({
                                    "event": "error",
                                    "kind": "bad_protocol",
                                    "message_b64": base64_string(&format!("invalid bridge command JSON: {e}")),
                                }));
                            }
                        }
                    }
                    Some(None) | None => {
                        transport
                            .send(ClientMessage::UnregisterShell { shell_id })
                            .await
                            .ok();
                        break;
                    }
                }
            }
            frame = transport.next() => {
                match frame {
                    Some(Ok(msg)) => {
                        emit_server_machine_event(msg);
                    }
                    Some(Err(e)) => {
                        return Err(anyhow!("bridge transport error: {e}"));
                    }
                    None => {
                        return Err(anyhow!("bridge lost daemon connection"));
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_bridge_command(
    transport: &mut tokio_serde::Framed<
        Framed<UnixStream, LengthDelimitedCodec>,
        ServerMessage,
        ClientMessage,
        Json<ServerMessage, ClientMessage>,
    >,
    shell_id: Uuid,
    command: BridgeCommand,
) -> Result<()> {
    match command {
        BridgeCommand::StartTask {
            task_id,
            mode,
            prompt,
            cwd,
        } => {
            let task_id = task_id.unwrap_or_else(Uuid::now_v7);
            transport
                .send(ClientMessage::StartTask {
                    payload: StartTask {
                        task_id,
                        shell_id,
                        shell_kind: detect_shell_kind(),
                        shell_version: detect_shell_version(),
                        mode,
                        prompt,
                        cwd,
                        env_subset: env_subset(),
                    },
                })
                .await?;
            emit_machine_event(serde_json::json!({
                "event": "task_started",
                "task_id": task_id,
            }));
        }
        BridgeCommand::UserResponse {
            task_id,
            decision,
            edited_command,
            text,
        } => {
            transport
                .send(ClientMessage::UserResponse {
                    payload: UserResponse {
                        task_id,
                        decision,
                        edited_command,
                        text,
                    },
                })
                .await?;
        }
        BridgeCommand::Ack {
            task_id,
            command_seq,
            command,
            cwd_before,
            cwd_after,
            exit_status,
            started_at_ms,
            stdout_b64,
            stdout_truncated,
            stderr_b64,
            stderr_truncated,
            elapsed_ms,
        } => {
            transport
                .send(ClientMessage::Ack {
                    payload: Ack {
                        task_id,
                        command_seq,
                        executed_command: command,
                        cwd_before,
                        cwd_after,
                        started_at: parse_started_at_ms(started_at_ms)?,
                        exit_status,
                        stdout_b64,
                        stdout_truncated,
                        stderr_b64,
                        stderr_truncated,
                        redactions_applied: Vec::new(),
                        elapsed_ms,
                    },
                })
                .await?;
        }
        BridgeCommand::ObserveCommand {
            command_seq,
            raw_command,
            expanded_command,
            cwd_before,
            cwd_after,
            exit_status,
            started_at_ms,
            duration_ms,
            stdout_b64,
            stdout_truncated,
            stderr_b64,
            stderr_truncated,
            output_capture_status,
        } => {
            transport
                .send(ClientMessage::ObservedCommand {
                    payload: ObservedCommand {
                        shell_id,
                        command_seq,
                        raw_command,
                        expanded_command,
                        cwd_before,
                        cwd_after,
                        started_at: parse_started_at_ms(started_at_ms)?,
                        exit_status,
                        duration_ms,
                        stdout_b64,
                        stdout_truncated,
                        stderr_b64,
                        stderr_truncated,
                        output_capture_status,
                    },
                })
                .await?;
        }
        BridgeCommand::ShellContext {
            context_hash,
            aliases,
            functions,
            builtins,
        } => {
            transport
                .send(ClientMessage::ShellContext {
                    payload: ShellContext {
                        shell_id,
                        shell_kind: detect_shell_kind(),
                        context_hash,
                        aliases,
                        functions,
                        builtins,
                    },
                })
                .await?;
        }
        BridgeCommand::Reindex { mode } => {
            transport
                .send(ClientMessage::Reindex {
                    mode: match mode {
                        ReindexModeArg::Delta => ReindexMode::Delta,
                        ReindexModeArg::Full => ReindexMode::Full,
                        ReindexModeArg::Compact => ReindexMode::Compact,
                    },
                })
                .await?;
        }
        BridgeCommand::Retrieve { prompt, top_k } => {
            transport
                .send(ClientMessage::Retrieve {
                    payload: RetrieveRequest { prompt, top_k },
                })
                .await?;
        }
        BridgeCommand::ProviderHealth => {
            transport.send(ClientMessage::ProviderHealth).await?;
        }
        BridgeCommand::Status => {
            transport.send(ClientMessage::Status).await?;
        }
        BridgeCommand::Ping => {
            transport.send(ClientMessage::Ping).await?;
        }
        BridgeCommand::Shutdown => {
            transport.send(ClientMessage::Shutdown).await?;
        }
        BridgeCommand::UnregisterShell => {
            transport
                .send(ClientMessage::UnregisterShell { shell_id })
                .await?;
        }
    }
    Ok(())
}

fn emit_server_machine_event(msg: ServerMessage) {
    match msg {
        ServerMessage::ShellRegistered { shell_id, .. } => {
            emit_machine_event(serde_json::json!({
                "event": "shell_registered",
                "shell_id": shell_id,
            }));
        }
        ServerMessage::ModelText { task_id, chunk } => {
            emit_machine_event(serde_json::json!({
                "event": "model_text",
                "task_id": task_id,
                "chunk_b64": base64_string(&chunk),
            }));
        }
        ServerMessage::ProposedCommand { payload } => {
            emit_machine_event(serde_json::json!({
                "event": "proposed_command",
                "task_id": payload.task_id,
                "cmd_b64": base64_string(&payload.cmd),
                "requires_approval": payload.requires_approval,
                "risk_level": payload.risk_level,
                "payload": payload,
            }));
        }
        ServerMessage::NeedsClarification { task_id, question } => {
            emit_machine_event(serde_json::json!({
                "event": "needs_clarification",
                "task_id": task_id,
                "question_b64": base64_string(&question),
            }));
        }
        ServerMessage::TaskComplete {
            task_id,
            reason,
            summary,
        } => {
            emit_machine_event(serde_json::json!({
                "event": "task_complete",
                "task_id": task_id,
                "reason": reason,
                "summary_b64": base64_string(&summary),
            }));
        }
        ServerMessage::Error {
            task_id,
            kind,
            message,
            matched_pattern,
        } => {
            emit_machine_event(serde_json::json!({
                "event": "error",
                "task_id": task_id,
                "kind": kind,
                "message_b64": base64_string(&message),
                "matched_pattern_b64": matched_pattern.map(|s| base64_string(&s)),
            }));
        }
        ServerMessage::RetrievalResult { chunks } => {
            for chunk in chunks {
                let rendered = render_retrieved_chunk_machine(&chunk);
                emit_machine_event(serde_json::json!({
                    "event": "retrieval_chunk",
                    "chunk_b64": base64_string(&rendered),
                }));
            }
        }
        status @ ServerMessage::StatusReport { .. } => {
            emit_machine_event(serde_json::json!({
                "event": "status_report",
                "payload": status,
            }));
        }
        ServerMessage::IndexProgress(progress) => {
            emit_machine_event(serde_json::json!({
                "event": "index_progress",
                "payload": progress,
            }));
        }
        ServerMessage::IndexUpdate {
            added,
            updated,
            removed,
        } => {
            emit_machine_event(serde_json::json!({
                "event": "index_update",
                "added": added,
                "updated": updated,
                "removed": removed,
            }));
        }
        ServerMessage::ProviderStatus {
            provider,
            model,
            endpoint,
            healthy,
            remote,
        } => {
            emit_machine_event(serde_json::json!({
                "event": "provider_status",
                "provider": provider,
                "model": model,
                "endpoint": endpoint,
                "healthy": healthy,
                "remote": remote,
            }));
        }
        ServerMessage::Pong => {
            emit_machine_event(serde_json::json!({
                "event": "pong",
            }));
        }
    }
}

fn parse_started_at_ms(started_at_ms: Option<i64>) -> Result<DateTime<Utc>> {
    let Some(ms) = started_at_ms else {
        return Ok(Utc::now());
    };
    DateTime::from_timestamp_millis(ms)
        .with_context(|| format!("invalid --started-at-ms timestamp: {ms}"))
}

async fn wait_task_events(
    transport: &mut tokio_serde::Framed<
        Framed<UnixStream, LengthDelimitedCodec>,
        ServerMessage,
        ClientMessage,
        Json<ServerMessage, ClientMessage>,
    >,
    task_id: Uuid,
    idle_timeout: Duration,
) -> Result<bool> {
    loop {
        match tokio::time::timeout(idle_timeout, transport.next()).await {
            Ok(Some(Ok(msg))) => {
                if print_machine_message(msg, task_id) {
                    return Ok(false);
                }
            }
            Ok(Some(Err(e))) => return Err(anyhow!("transport error: {e}")),
            Ok(None) => return Ok(false),
            Err(_) => {
                emit_machine_event(serde_json::json!({
                    "event": "timeout",
                    "task_id": task_id,
                }));
                return Ok(true);
            }
        }
    }
}

async fn drain_immediate_events(
    transport: &mut tokio_serde::Framed<
        Framed<UnixStream, LengthDelimitedCodec>,
        ServerMessage,
        ClientMessage,
        Json<ServerMessage, ClientMessage>,
    >,
    task_id: Uuid,
    window: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(now);
        match tokio::time::timeout(remaining, transport.next()).await {
            Ok(Some(Ok(msg))) => {
                if print_machine_message(msg, task_id) {
                    return Ok(());
                }
            }
            Ok(Some(Err(e))) => return Err(anyhow!("transport error: {e}")),
            Ok(None) | Err(_) => return Ok(()),
        }
    }
}

fn print_machine_message(msg: ServerMessage, task_id: Uuid) -> bool {
    match msg {
        ServerMessage::ShellRegistered { shell_id, .. } => {
            emit_machine_event(serde_json::json!({
                "event": "shell_registered",
                "shell_id": shell_id,
            }));
            false
        }
        ServerMessage::ModelText { chunk, .. } => {
            emit_machine_event(serde_json::json!({
                "event": "model_text",
                "task_id": task_id,
                "chunk_b64": base64_string(&chunk),
            }));
            false
        }
        ServerMessage::ProposedCommand { payload } => {
            emit_machine_event(serde_json::json!({
                "event": "proposed_command",
                "task_id": payload.task_id,
                "cmd_b64": base64_string(&payload.cmd),
                "requires_approval": payload.requires_approval,
                "risk_level": payload.risk_level,
                "payload": payload,
            }));
            true
        }
        ServerMessage::NeedsClarification {
            task_id: tid,
            question,
        } => {
            emit_machine_event(serde_json::json!({
                "event": "needs_clarification",
                "task_id": tid,
                "question_b64": base64_string(&question),
            }));
            true
        }
        ServerMessage::TaskComplete {
            task_id: tid,
            reason,
            summary,
        } => {
            emit_machine_event(serde_json::json!({
                "event": "task_complete",
                "task_id": tid,
                "reason": reason,
                "summary_b64": base64_string(&summary),
            }));
            true
        }
        ServerMessage::Error {
            task_id: tid,
            kind,
            message,
            matched_pattern,
        } => {
            let tid = tid.unwrap_or(task_id);
            emit_machine_event(serde_json::json!({
                "event": "error",
                "task_id": tid,
                "kind": kind,
                "message_b64": base64_string(&message),
                "matched_pattern_b64": matched_pattern.map(|s| base64_string(&s)),
            }));
            false
        }
        ServerMessage::RetrievalResult { chunks } => {
            for chunk in chunks {
                let rendered = render_retrieved_chunk_machine(&chunk);
                emit_machine_event(serde_json::json!({
                    "event": "retrieval_chunk",
                    "task_id": task_id,
                    "chunk_b64": base64_string(&rendered),
                }));
            }
            true
        }
        other => {
            emit_machine_event(serde_json::json!({
                "event": "daemon_event",
                "task_id": task_id,
                "debug": format!("{other:?}"),
            }));
            false
        }
    }
}

fn emit_machine_event(value: serde_json::Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}

fn base64_string(value: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

fn render_retrieved_chunk_machine(chunk: &termlm_protocol::RetrievedChunk) -> String {
    let hash = if chunk.doc_hash.is_empty() {
        "unknown"
    } else {
        chunk.doc_hash.as_str()
    };
    format!(
        "### {} — {}\nSource: {}; extraction_method={}; extracted_at={}; doc_hash={}\n{}",
        chunk.command_name,
        chunk.section_name,
        chunk.path,
        chunk.extraction_method,
        chunk.extracted_at.to_rfc3339(),
        hash,
        chunk.text
    )
}

async fn register_shell(
    transport: &mut tokio_serde::Framed<
        Framed<UnixStream, LengthDelimitedCodec>,
        ServerMessage,
        ClientMessage,
        Json<ServerMessage, ClientMessage>,
    >,
) -> Result<Uuid> {
    let meta = resolve_shell_registration_meta();
    transport
        .send(ClientMessage::RegisterShell {
            payload: RegisterShell {
                shell_pid: meta.shell_pid,
                tty: meta.tty,
                client_version: env!("CARGO_PKG_VERSION").to_string(),
                shell_kind: meta.shell_kind,
                shell_version: meta.shell_version,
                adapter_version: meta.adapter_version,
                capabilities: ShellCapabilities {
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
                },
                env_subset: env_subset(),
            },
        })
        .await?;

    while let Some(frame) = transport.next().await {
        let msg = frame?;
        if let ServerMessage::ShellRegistered { shell_id, .. } = msg {
            return Ok(shell_id);
        }
    }

    Err(anyhow!("did not receive ShellRegistered"))
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

fn parse_shell_kind(raw: &str) -> ShellKind {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return ShellKind::Other("unknown".to_string());
    }
    if normalized.contains("zsh") {
        return ShellKind::Zsh;
    }
    if normalized.contains("bash") {
        return ShellKind::Bash;
    }
    if normalized.contains("fish") {
        return ShellKind::Fish;
    }
    ShellKind::Other(normalized)
}

fn detect_shell_kind_with<F>(lookup: &F) -> ShellKind
where
    F: Fn(&str) -> Option<String>,
{
    env_nonempty_with(lookup, ENV_SHELL_KIND)
        .or_else(|| env_nonempty_with(lookup, "SHELL"))
        .map(|raw| parse_shell_kind(&raw))
        .unwrap_or(ShellKind::Zsh)
}

fn detect_shell_kind() -> ShellKind {
    detect_shell_kind_with(&|name| std::env::var(name).ok())
}

fn detect_shell_version_with<F>(lookup: &F, shell_kind: &ShellKind) -> String
where
    F: Fn(&str) -> Option<String>,
{
    let from_kind = match shell_kind {
        ShellKind::Zsh => env_nonempty_with(lookup, "ZSH_VERSION"),
        ShellKind::Bash => env_nonempty_with(lookup, "BASH_VERSION"),
        ShellKind::Fish => env_nonempty_with(lookup, "FISH_VERSION"),
        ShellKind::Other(_) => None,
    };
    from_kind
        .or_else(|| env_nonempty_with(lookup, "SHELL_VERSION"))
        .unwrap_or_else(|| "unknown".to_string())
}

fn detect_shell_version() -> String {
    let shell_kind = detect_shell_kind();
    detect_shell_version_with(&|name| std::env::var(name).ok(), &shell_kind)
}

fn resolve_socket_path(config_path: &str) -> std::path::PathBuf {
    let xdg = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: geteuid reads current effective uid.
        let uid = unsafe { libc::geteuid() };
        format!("/tmp/termlm-{uid}")
    });

    if config_path.contains("$XDG_RUNTIME_DIR") {
        std::path::PathBuf::from(config_path.replace("$XDG_RUNTIME_DIR", &xdg))
    } else {
        std::path::PathBuf::from(config_path)
    }
}

fn resolve_runtime_path(config_path: &str) -> std::path::PathBuf {
    let xdg = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: geteuid reads current effective uid.
        let uid = unsafe { libc::geteuid() };
        format!("/tmp/termlm-{uid}")
    });

    if config_path.contains("$XDG_RUNTIME_DIR") {
        std::path::PathBuf::from(config_path.replace("$XDG_RUNTIME_DIR", &xdg))
    } else {
        std::path::PathBuf::from(config_path)
    }
}

fn signal_config_reload(pid_path: &std::path::Path) -> Result<()> {
    let raw = std::fs::read_to_string(pid_path)
        .with_context(|| format!("read {}", pid_path.display()))?;
    let pid = raw
        .trim()
        .parse::<i32>()
        .with_context(|| format!("invalid pid in {}", pid_path.display()))?;
    if pid <= 0 {
        return Err(anyhow!("invalid daemon pid: {pid}"));
    }

    // SAFETY: kill sends SIGHUP to a pid read from daemon pidfile.
    let rc = unsafe { libc::kill(pid, libc::SIGHUP) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("failed to send SIGHUP");
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    ok: bool,
    critical: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    overall_ok: bool,
    checks: Vec<DoctorCheck>,
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn install_bin_dir(home: &Path) -> PathBuf {
    std::env::var("TERMLM_INSTALL_BIN_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| expand_tilde(&v, home))
        .unwrap_or_else(|| home.join(".local/bin"))
}

fn install_share_dir(home: &Path) -> PathBuf {
    std::env::var("TERMLM_INSTALL_SHARE_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| expand_tilde(&v, home))
        .unwrap_or_else(|| home.join(".local/share/termlm"))
}

fn models_dir_from_config(cfg: &termlm_config::AppConfig, home: &Path) -> PathBuf {
    std::env::var("TERMLM_MODELS_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| expand_tilde(&v, home))
        .unwrap_or_else(|| expand_tilde(&cfg.model.models_dir, home))
}

fn expand_tilde(raw: &str, home: &Path) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if raw == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(raw)
    }
}

fn source_line_for_share(home: &Path, share_dir: &Path) -> String {
    if let Ok(rest) = share_dir.strip_prefix(home) {
        format!("source ~/{}/plugins/zsh/termlm.plugin.zsh", rest.display())
    } else {
        format!(
            "source {}/plugins/zsh/termlm.plugin.zsh",
            share_dir.display()
        )
    }
}

fn run_init_zsh(print_only: bool, force: bool) -> Result<()> {
    let home = home_dir()?;
    let share_dir = install_share_dir(&home);
    let plugin_path = share_dir.join("plugins/zsh/termlm.plugin.zsh");
    let source_line = source_line_for_share(&home, &share_dir);

    if print_only {
        println!("{source_line}");
        return Ok(());
    }

    let zshrc = home.join(".zshrc");
    let existing = if zshrc.exists() {
        std::fs::read_to_string(&zshrc).with_context(|| format!("read {}", zshrc.display()))?
    } else {
        String::new()
    };

    if existing.contains(&source_line) {
        println!("termlm: ~/.zshrc already contains the canonical source line");
        return Ok(());
    }
    if existing.contains("termlm.plugin.zsh") && !force {
        println!("termlm: existing termlm plugin source entry found in ~/.zshrc");
        println!("termlm: rerun with --force to append canonical line:");
        println!("  {source_line}");
        return Ok(());
    }

    if let Some(parent) = zshrc.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&zshrc)
        .with_context(|| format!("open {}", zshrc.display()))?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, "\n# termlm\n{source_line}")?;

    println!("termlm: added plugin source line to {}", zshrc.display());
    println!("termlm: open a new zsh session after install");
    if !plugin_path.exists() {
        println!(
            "termlm: warning: plugin path not found yet ({})",
            plugin_path.display()
        );
    }
    Ok(())
}

fn find_on_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.exists())
}

async fn run_doctor(cfg: &termlm_config::AppConfig, strict: bool, json: bool) -> Result<()> {
    let home = home_dir()?;
    let share_dir = install_share_dir(&home);
    let plugin_path = share_dir.join("plugins/zsh/termlm.plugin.zsh");
    let zshrc_path = home.join(".zshrc");
    let source_line = source_line_for_share(&home, &share_dir);
    let models_dir = models_dir_from_config(cfg, &home);
    let selected_model = if cfg.model.variant.eq_ignore_ascii_case("e2b") {
        cfg.model.e2b_filename.as_str()
    } else {
        cfg.model.e4b_filename.as_str()
    };
    let selected_model_path = models_dir.join(selected_model);
    let socket_path = resolve_socket_path(&cfg.daemon.socket_path);

    let mut checks = Vec::<DoctorCheck>::new();
    checks.push(DoctorCheck {
        name: "config",
        ok: true,
        critical: true,
        detail: "config loaded and validated".to_string(),
    });

    let core_bin = std::env::var("TERMLM_CORE_BIN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| find_on_path("termlm-core"));
    checks.push(DoctorCheck {
        name: "termlm-core-bin",
        ok: core_bin.as_ref().is_some_and(|p| p.exists()),
        critical: true,
        detail: core_bin
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found on PATH".to_string()),
    });

    checks.push(DoctorCheck {
        name: "zsh-plugin-path",
        ok: plugin_path.exists(),
        critical: true,
        detail: plugin_path.display().to_string(),
    });

    let zshrc_contains_source = zshrc_path.exists()
        && std::fs::read_to_string(&zshrc_path)
            .ok()
            .is_some_and(|raw| raw.contains("termlm.plugin.zsh"));
    checks.push(DoctorCheck {
        name: "zshrc-source",
        ok: zshrc_contains_source,
        critical: false,
        detail: format!("{} (expected line: {source_line})", zshrc_path.display()),
    });

    let model_required = cfg.inference.provider == "local";
    checks.push(DoctorCheck {
        name: "local-model",
        ok: selected_model_path.exists(),
        critical: model_required,
        detail: selected_model_path.display().to_string(),
    });

    checks.push(DoctorCheck {
        name: "daemon-socket-path",
        ok: socket_path.parent().is_some_and(Path::exists),
        critical: false,
        detail: socket_path.display().to_string(),
    });

    let daemon_reachable = match tokio::time::timeout(
        Duration::from_millis(750),
        UnixStream::connect(&socket_path),
    )
    .await
    {
        Ok(Ok(_)) => true,
        Ok(Err(_)) | Err(_) => false,
    };
    checks.push(DoctorCheck {
        name: "daemon-connectivity",
        ok: daemon_reachable,
        critical: false,
        detail: if daemon_reachable {
            format!("connected to {}", socket_path.display())
        } else {
            format!("cannot connect to {}", socket_path.display())
        },
    });

    let overall_ok = checks
        .iter()
        .all(|c| if strict || c.critical { c.ok } else { true });
    let report = DoctorReport { overall_ok, checks };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for check in &report.checks {
            let status = if check.ok { "ok" } else { "fail" };
            let severity = if check.critical {
                "critical"
            } else {
                "advisory"
            };
            println!("[{status}] {} ({severity}) - {}", check.name, check.detail);
        }
        if report.overall_ok {
            println!("termlm doctor: passed");
        } else if strict {
            println!("termlm doctor: failed (strict)");
        } else {
            println!("termlm doctor: warnings present");
        }
    }

    if strict && !report.overall_ok {
        return Err(anyhow!("doctor strict checks failed"));
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path, dry_run: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if dry_run {
        println!("would remove {}", path.display());
        return Ok(true);
    }
    let meta =
        std::fs::symlink_metadata(path).with_context(|| format!("metadata {}", path.display()))?;
    if meta.file_type().is_dir() {
        std::fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    } else {
        std::fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    println!("removed {}", path.display());
    Ok(true)
}

fn run_uninstall(
    cfg: &termlm_config::AppConfig,
    yes: bool,
    keep_models: bool,
    dry_run: bool,
) -> Result<()> {
    if !yes && !dry_run {
        println!("termlm: refusing to uninstall without --yes");
        println!("termlm: rerun with `termlm uninstall --yes`");
        return Ok(());
    }

    let home = home_dir()?;
    let bin_dir = install_bin_dir(&home);
    let share_dir = install_share_dir(&home);
    let models_dir = models_dir_from_config(cfg, &home);
    let zsh_plugin_dir = share_dir.join("plugins/zsh");
    let receipt = share_dir.join("install-receipt.json");

    let candidates = vec![
        bin_dir.join("termlm"),
        bin_dir.join("termlm-client"),
        bin_dir.join("termlm-core"),
        zsh_plugin_dir,
        receipt,
    ];
    for path in candidates {
        let _ = remove_path_if_exists(&path, dry_run)?;
    }

    if keep_models {
        println!("keeping models at {}", models_dir.display());
    } else {
        let _ = remove_path_if_exists(&models_dir, dry_run)?;
    }

    println!("termlm uninstall complete");
    println!("manual step: remove termlm source line from ~/.zshrc if present");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_start_task_parses() {
        let raw = r#"{"op":"start_task","mode":"?","prompt":"list files","cwd":"/tmp"}"#;
        let parsed: BridgeCommand = serde_json::from_str(raw).expect("parse bridge start_task");
        match parsed {
            BridgeCommand::StartTask {
                task_id,
                mode,
                prompt,
                cwd,
            } => {
                assert!(task_id.is_none());
                assert_eq!(mode, "?");
                assert_eq!(prompt, "list files");
                assert_eq!(cwd, "/tmp");
            }
            other => panic!("unexpected bridge command: {other:?}"),
        }
    }

    #[test]
    fn bridge_ack_parses_with_optional_fields() {
        let raw = r#"{
            "op":"ack",
            "task_id":"019f7b37-3412-7aa4-912f-27e0e8ce6a71",
            "command_seq":4,
            "command":"ls -la",
            "cwd_before":"/tmp",
            "cwd_after":"/tmp",
            "exit_status":0,
            "elapsed_ms":25,
            "stdout_b64":"aGVsbG8=",
            "stderr_truncated":false
        }"#;
        let parsed: BridgeCommand = serde_json::from_str(raw).expect("parse bridge ack");
        match parsed {
            BridgeCommand::Ack {
                task_id,
                command_seq,
                command,
                cwd_before,
                cwd_after,
                exit_status,
                elapsed_ms,
                stdout_b64,
                ..
            } => {
                assert_eq!(
                    task_id.to_string(),
                    "019f7b37-3412-7aa4-912f-27e0e8ce6a71".to_string()
                );
                assert_eq!(command_seq, 4);
                assert_eq!(command, "ls -la");
                assert_eq!(cwd_before, "/tmp");
                assert_eq!(cwd_after, "/tmp");
                assert_eq!(exit_status, 0);
                assert_eq!(elapsed_ms, 25);
                assert_eq!(stdout_b64.as_deref(), Some("aGVsbG8="));
            }
            other => panic!("unexpected bridge command: {other:?}"),
        }
    }

    #[test]
    fn registration_meta_prefers_bridge_env_overrides() {
        let mut env = BTreeMap::<String, String>::new();
        env.insert(ENV_SHELL_PID.to_string(), "12345".to_string());
        env.insert(ENV_SHELL_TTY.to_string(), "/dev/ttys999".to_string());
        env.insert(ENV_SHELL_KIND.to_string(), "bash".to_string());
        env.insert(ENV_SHELL_VERSION.to_string(), "5.9-custom".to_string());
        env.insert(ENV_ADAPTER_VERSION.to_string(), "zsh-v1-test".to_string());
        env.insert("TTY".to_string(), "/dev/ttys-fallback".to_string());

        let meta = resolve_shell_registration_meta_with(|name| env.get(name).cloned());

        assert_eq!(meta.shell_pid, 12345);
        assert_eq!(meta.tty, "/dev/ttys999");
        assert!(matches!(meta.shell_kind, ShellKind::Bash));
        assert_eq!(meta.shell_version, "5.9-custom");
        assert_eq!(meta.adapter_version, "zsh-v1-test");
    }

    #[test]
    fn registration_meta_uses_tty_fallback_when_shell_tty_missing() {
        let mut env = BTreeMap::<String, String>::new();
        env.insert("TTY".to_string(), "/dev/ttys201".to_string());

        let meta = resolve_shell_registration_meta_with(|name| env.get(name).cloned());

        assert_eq!(meta.shell_pid, std::process::id());
        assert_eq!(meta.tty, "/dev/ttys201");
        assert!(matches!(meta.shell_kind, ShellKind::Zsh));
        assert_eq!(meta.adapter_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn shell_kind_detection_uses_shell_path_when_override_missing() {
        let mut env = BTreeMap::<String, String>::new();
        env.insert("SHELL".to_string(), "/bin/fish".to_string());
        env.insert("FISH_VERSION".to_string(), "3.7.1".to_string());

        let meta = resolve_shell_registration_meta_with(|name| env.get(name).cloned());
        assert!(matches!(meta.shell_kind, ShellKind::Fish));
        assert_eq!(meta.shell_version, "3.7.1");
    }

    #[test]
    fn upgrade_cli_parses_documented_command() {
        let cli = Cli::try_parse_from(["termlm", "upgrade"]).expect("parse upgrade");
        match cli.cmd {
            Command::Upgrade { repo, tag } => {
                assert!(repo.is_none());
                assert!(tag.is_none());
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn update_alias_maps_to_upgrade_command() {
        let cli = Cli::try_parse_from(["termlm", "update"]).expect("parse update alias");
        match cli.cmd {
            Command::Upgrade { .. } => {}
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn update_alias_is_not_rendered_as_documented_subcommand() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let mut out = Vec::new();
        cmd.write_long_help(&mut out).expect("render long help");
        let help = String::from_utf8(out).expect("help utf8");
        assert!(help.contains("upgrade"));
        assert!(!help.contains("\n  update"));
        assert!(!help.contains("\n  bridge"));
        assert!(!help.contains("\n  register-shell"));
    }

    #[test]
    fn init_zsh_command_parses() {
        let cli = Cli::try_parse_from(["termlm", "init", "zsh"]).expect("parse init zsh");
        match cli.cmd {
            Command::Init { shell } => match shell {
                InitCommand::Zsh { print_only, force } => {
                    assert!(!print_only);
                    assert!(!force);
                }
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn doctor_command_parses_strict_json() {
        let cli =
            Cli::try_parse_from(["termlm", "doctor", "--strict", "--json"]).expect("parse doctor");
        match cli.cmd {
            Command::Doctor { strict, json } => {
                assert!(strict);
                assert!(json);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn uninstall_command_parses_flags() {
        let cli =
            Cli::try_parse_from(["termlm", "uninstall", "--yes", "--keep-models", "--dry-run"])
                .expect("parse uninstall");
        match cli.cmd {
            Command::Uninstall {
                yes,
                keep_models,
                dry_run,
            } => {
                assert!(yes);
                assert!(keep_models);
                assert!(dry_run);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn reindex_cli_accepts_full_flag() {
        let cli = Cli::try_parse_from(["termlm", "reindex", "--full"]).expect("parse cli");
        match cli.cmd {
            Command::Reindex {
                mode,
                full,
                compact,
            } => {
                assert_eq!(mode, ReindexModeArg::Delta);
                assert!(full);
                assert!(!compact);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn reindex_cli_accepts_compact_flag() {
        let cli = Cli::try_parse_from(["termlm", "reindex", "--compact"]).expect("parse cli");
        match cli.cmd {
            Command::Reindex {
                mode,
                full,
                compact,
            } => {
                assert_eq!(mode, ReindexModeArg::Delta);
                assert!(!full);
                assert!(compact);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
