use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use termlm_protocol::{
    Ack, ClientMessage, RegisterShell, ServerMessage, ShellCapabilities, ShellKind,
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

#[tokio::test(flavor = "current_thread")]
async fn plugin_side_ack_round_trip() -> Result<()> {
    let short = Uuid::now_v7().simple().to_string();
    let socket_path =
        std::path::PathBuf::from(format!("/tmp/termlm-mock-plugin-{}.sock", &short[..12]));
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;

    let (tx, rx) = tokio::sync::oneshot::channel::<Ack>();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(1024 * 1024)
            .new_codec();
        let framed = Framed::new(stream, codec);
        let mut transport =
            tokio_serde::Framed::new(framed, Json::<ClientMessage, ServerMessage>::default());

        let mut tx = Some(tx);
        while let Some(frame) = transport.next().await {
            match frame? {
                ClientMessage::RegisterShell { .. } => {
                    transport
                        .send(ServerMessage::ShellRegistered {
                            shell_id: Uuid::now_v7(),
                            accepted_capabilities: vec!["prompt_mode".to_string()],
                            provider: "local".to_string(),
                            model: "gemma-4-E4B".to_string(),
                            context_tokens: 8192,
                        })
                        .await?;
                }
                ClientMessage::Ack { payload } => {
                    if let Some(sender) = tx.take() {
                        let _ = sender.send(payload);
                    }
                    break;
                }
                _ => {}
            }
        }

        Result::<()>::Ok(())
    });

    let stream = UnixStream::connect(&socket_path).await?;
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(1024 * 1024)
        .new_codec();
    let framed = Framed::new(stream, codec);
    let mut client =
        tokio_serde::Framed::new(framed, Json::<ServerMessage, ClientMessage>::default());

    client
        .send(ClientMessage::RegisterShell {
            payload: RegisterShell {
                shell_pid: std::process::id(),
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

    let _ = client.next().await;

    let task_id = Uuid::now_v7();
    client
        .send(ClientMessage::Ack {
            payload: Ack {
                task_id,
                command_seq: 1,
                executed_command: "ls -la".to_string(),
                cwd_before: "/tmp".to_string(),
                cwd_after: "/tmp".to_string(),
                started_at: chrono::Utc::now(),
                exit_status: 0,
                stdout_b64: None,
                stdout_truncated: false,
                stderr_b64: None,
                stderr_truncated: false,
                redactions_applied: Vec::new(),
                elapsed_ms: 10,
            },
        })
        .await?;

    let ack = rx.await.expect("ack must be captured");
    assert_eq!(ack.task_id, task_id);
    assert_eq!(ack.executed_command, "ls -la");

    server_task.await??;
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}
