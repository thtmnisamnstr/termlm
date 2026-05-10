use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use termlm_protocol::{
    ClientMessage, ProposedCommand, RegisterShell, ServerMessage, ShellCapabilities, ShellKind,
    TaskCompleteReason, ValidationSummary,
};
use tokio::net::{UnixListener, UnixStream};
use tokio_serde::formats::Json;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use uuid::Uuid;

fn demo_capabilities() -> ShellCapabilities {
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

fn server_transport(
    stream: UnixStream,
) -> tokio_serde::Framed<
    Framed<UnixStream, LengthDelimitedCodec>,
    ClientMessage,
    ServerMessage,
    Json<ClientMessage, ServerMessage>,
> {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(1024 * 1024)
        .new_codec();
    let framed = Framed::new(stream, codec);
    tokio_serde::Framed::new(framed, Json::<ClientMessage, ServerMessage>::default())
}

fn client_transport(
    stream: UnixStream,
) -> tokio_serde::Framed<
    Framed<UnixStream, LengthDelimitedCodec>,
    ServerMessage,
    ClientMessage,
    Json<ServerMessage, ClientMessage>,
> {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(1024 * 1024)
        .new_codec();
    let framed = Framed::new(stream, codec);
    tokio_serde::Framed::new(framed, Json::<ServerMessage, ClientMessage>::default())
}

#[tokio::test(flavor = "current_thread")]
async fn register_start_abort_roundtrip() -> Result<()> {
    let short = Uuid::now_v7().simple().to_string();
    let socket_path =
        std::path::PathBuf::from(format!("/tmp/termlm-mock-daemon-{}.sock", &short[..12]));
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut transport = server_transport(stream);
        let shell_id = Uuid::now_v7();
        while let Some(frame) = transport.next().await {
            match frame? {
                ClientMessage::RegisterShell {
                    payload: RegisterShell { .. },
                } => {
                    transport
                        .send(ServerMessage::ShellRegistered {
                            shell_id,
                            accepted_capabilities: vec![
                                "prompt_mode".to_string(),
                                "session_mode".to_string(),
                                "execute_in_real_shell".to_string(),
                            ],
                            provider: "local".to_string(),
                            model: "gemma-4-E4B".to_string(),
                            context_tokens: 8192,
                        })
                        .await?;
                }
                ClientMessage::StartTask { payload } => {
                    transport
                        .send(ServerMessage::ModelText {
                            task_id: payload.task_id,
                            chunk: "Planning...".to_string(),
                        })
                        .await?;
                    transport
                        .send(ServerMessage::ProposedCommand {
                            payload: ProposedCommand {
                                task_id: payload.task_id,
                                cmd: "ls -la".to_string(),
                                rationale: "Show all files.".to_string(),
                                intent: "List files.".to_string(),
                                expected_effect: "Read-only listing.".to_string(),
                                commands_used: vec!["ls".to_string()],
                                risk_level: "read_only".to_string(),
                                requires_approval: true,
                                critical_match: None,
                                grounding: Vec::new(),
                                validation: ValidationSummary {
                                    status: "passed".to_string(),
                                    planning_rounds: 1,
                                },
                                round: 1,
                            },
                        })
                        .await?;
                }
                ClientMessage::UserResponse { payload } => {
                    transport
                        .send(ServerMessage::TaskComplete {
                            task_id: payload.task_id,
                            reason: TaskCompleteReason::Aborted,
                            summary: "aborted by user".to_string(),
                        })
                        .await?;
                    break;
                }
                _ => {}
            }
        }
        Result::<()>::Ok(())
    });

    let stream = UnixStream::connect(&socket_path).await?;
    let mut client = client_transport(stream);
    let shell_pid = std::process::id();

    client
        .send(ClientMessage::RegisterShell {
            payload: RegisterShell {
                shell_pid,
                tty: "mock".to_string(),
                client_version: "test".to_string(),
                shell_kind: ShellKind::Zsh,
                shell_version: "5.9".to_string(),
                adapter_version: "test".to_string(),
                capabilities: demo_capabilities(),
                env_subset: std::collections::BTreeMap::new(),
            },
        })
        .await?;

    let shell_id = match client.next().await {
        Some(Ok(ServerMessage::ShellRegistered { shell_id, .. })) => shell_id,
        other => panic!("expected ShellRegistered, got {other:?}"),
    };

    let task_id = Uuid::now_v7();
    client
        .send(ClientMessage::StartTask {
            payload: termlm_protocol::StartTask {
                task_id,
                shell_id,
                shell_kind: ShellKind::Zsh,
                shell_version: "5.9".to_string(),
                mode: "?".to_string(),
                prompt: "list files".to_string(),
                cwd: "/tmp".to_string(),
                env_subset: std::collections::BTreeMap::new(),
            },
        })
        .await?;

    match client.next().await {
        Some(Ok(ServerMessage::ModelText { task_id: tid, .. })) => assert_eq!(tid, task_id),
        other => panic!("expected ModelText, got {other:?}"),
    }
    match client.next().await {
        Some(Ok(ServerMessage::ProposedCommand { payload })) => {
            assert_eq!(payload.task_id, task_id);
            assert_eq!(payload.cmd, "ls -la");
        }
        other => panic!("expected ProposedCommand, got {other:?}"),
    }

    client
        .send(ClientMessage::UserResponse {
            payload: termlm_protocol::UserResponse {
                task_id,
                decision: termlm_protocol::UserDecision::Abort,
                edited_command: None,
                text: Some("stop".to_string()),
            },
        })
        .await?;

    match client.next().await {
        Some(Ok(ServerMessage::TaskComplete {
            task_id: tid,
            reason,
            ..
        })) => {
            assert_eq!(tid, task_id);
            assert_eq!(reason, TaskCompleteReason::Aborted);
        }
        other => panic!("expected TaskComplete, got {other:?}"),
    }

    server_task.await??;
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}
