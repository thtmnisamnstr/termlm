use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellCapabilities {
    pub prompt_mode: bool,
    pub session_mode: bool,
    pub single_key_approval: bool,
    pub edit_approval: bool,
    pub execute_in_real_shell: bool,
    pub command_completion_ack: bool,
    pub stdout_stderr_capture: bool,
    pub all_interactive_command_observation: bool,
    pub terminal_context_capture: bool,
    pub alias_capture: bool,
    pub function_capture: bool,
    pub builtin_inventory: bool,
    pub shell_native_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterShell {
    pub shell_pid: u32,
    pub tty: String,
    pub client_version: String,
    pub shell_kind: ShellKind,
    pub shell_version: String,
    pub adapter_version: String,
    pub capabilities: ShellCapabilities,
    pub env_subset: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartTask {
    pub task_id: Uuid,
    pub shell_id: Uuid,
    pub shell_kind: ShellKind,
    pub shell_version: String,
    pub mode: String,
    pub prompt: String,
    pub cwd: String,
    pub env_subset: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserDecision {
    Approved,
    Rejected,
    Edited,
    ApproveAllInTask,
    Abort,
    Clarification,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserResponse {
    pub task_id: Uuid,
    pub decision: UserDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ack {
    pub task_id: Uuid,
    pub command_seq: u64,
    pub executed_command: String,
    pub cwd_before: String,
    pub cwd_after: String,
    pub started_at: DateTime<Utc>,
    pub exit_status: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_b64: Option<String>,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_b64: Option<String>,
    #[serde(default)]
    pub stderr_truncated: bool,
    #[serde(default)]
    pub redactions_applied: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservedCommand {
    pub shell_id: Uuid,
    pub command_seq: u64,
    pub raw_command: String,
    pub expanded_command: String,
    pub cwd_before: String,
    pub cwd_after: String,
    pub started_at: DateTime<Utc>,
    pub exit_status: i32,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_b64: Option<String>,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_b64: Option<String>,
    #[serde(default)]
    pub stderr_truncated: bool,
    pub output_capture_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasDef {
    pub name: String,
    pub expansion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionDef {
    pub name: String,
    pub body_prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellContext {
    pub shell_id: Uuid,
    pub shell_kind: ShellKind,
    pub context_hash: String,
    #[serde(default)]
    pub aliases: Vec<AliasDef>,
    #[serde(default)]
    pub functions: Vec<FunctionDef>,
    #[serde(default)]
    pub builtins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReindexMode {
    Delta,
    Full,
    Compact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetrieveRequest {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ClientMessage {
    RegisterShell {
        #[serde(flatten)]
        payload: RegisterShell,
    },
    StartTask {
        #[serde(flatten)]
        payload: StartTask,
    },
    UserResponse {
        #[serde(flatten)]
        payload: UserResponse,
    },
    Ack {
        #[serde(flatten)]
        payload: Ack,
    },
    ObservedCommand {
        #[serde(flatten)]
        payload: ObservedCommand,
    },
    UnregisterShell {
        shell_id: Uuid,
    },
    Shutdown,
    Status,
    ShellContext {
        #[serde(flatten)]
        payload: ShellContext,
    },
    Reindex {
        mode: ReindexMode,
    },
    Retrieve {
        #[serde(flatten)]
        payload: RetrieveRequest,
    },
    ProviderHealth,
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroundingRef {
    pub command: String,
    pub source: String,
    #[serde(default)]
    pub sections: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationSummary {
    pub status: String,
    pub planning_rounds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievedChunk {
    pub command_name: String,
    pub section_name: String,
    pub path: String,
    #[serde(default)]
    pub extraction_method: String,
    pub chunk_index: usize,
    pub total_chunks: usize,
    pub doc_hash: String,
    pub extracted_at: DateTime<Utc>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProposedCommand {
    pub task_id: Uuid,
    pub cmd: String,
    pub rationale: String,
    pub intent: String,
    pub expected_effect: String,
    #[serde(default)]
    pub commands_used: Vec<String>,
    pub risk_level: String,
    pub requires_approval: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_match: Option<String>,
    #[serde(default)]
    pub grounding: Vec<GroundingRef>,
    pub validation: ValidationSummary,
    pub round: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskCompleteReason {
    ModelDone,
    Aborted,
    ToolRoundLimit,
    SafetyFloor,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    SafetyFloor,
    ModelStalled,
    ModelLoadFailed,
    InferenceProviderUnavailable,
    BadToolCall,
    UnknownCommand,
    BadProtocol,
    ConfigInvalid,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexProgress {
    pub scanned: u64,
    pub total: u64,
    pub percent: f32,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebStatus {
    pub enabled: bool,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatusSourceRef {
    pub source_type: String,
    pub source_id: String,
    pub hash: String,
    pub redacted: bool,
    pub truncated: bool,
    pub observed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_end: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ServerMessage {
    ShellRegistered {
        shell_id: Uuid,
        accepted_capabilities: Vec<String>,
        provider: String,
        model: String,
        context_tokens: u32,
    },
    ModelText {
        task_id: Uuid,
        chunk: String,
    },
    ProposedCommand {
        #[serde(flatten)]
        payload: ProposedCommand,
    },
    NeedsClarification {
        task_id: Uuid,
        question: String,
    },
    TaskComplete {
        task_id: Uuid,
        reason: TaskCompleteReason,
        summary: String,
    },
    Error {
        task_id: Option<Uuid>,
        kind: ErrorKind,
        message: String,
        matched_pattern: Option<String>,
    },
    StatusReport {
        pid: u32,
        uptime_secs: u64,
        socket_path: String,
        provider: String,
        model: String,
        endpoint: Option<String>,
        provider_healthy: bool,
        provider_health_latency_ms: Option<u64>,
        provider_context_window: u32,
        provider_structured_mode: String,
        provider_supports_native_tool_calls: bool,
        provider_supports_json_mode: bool,
        provider_remote: bool,
        rss_mb: u64,
        model_resident_mb: u64,
        indexer_resident_mb: u64,
        orchestration_resident_mb: u64,
        kv_cache_mb: u64,
        active_shells: usize,
        active_tasks: usize,
        model_load_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_task_prompt_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_task_completion_tokens: Option<u64>,
        #[serde(default)]
        last_task_usage_reported: bool,
        last_task_source_refs: usize,
        #[serde(default)]
        last_task_source_ledger: Vec<StatusSourceRef>,
        #[serde(default)]
        stage_timings_ms: BTreeMap<String, u64>,
        #[serde(default)]
        index_chunk_count: u64,
        index_progress: IndexProgress,
        web: WebStatus,
    },
    IndexProgress(IndexProgress),
    IndexUpdate {
        added: Vec<String>,
        updated: Vec<String>,
        removed: Vec<String>,
    },
    ProviderStatus {
        provider: String,
        model: String,
        endpoint: Option<String>,
        healthy: bool,
        remote: bool,
    },
    RetrievalResult {
        chunks: Vec<RetrievedChunk>,
    },
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_size_constant_is_one_mebibyte() {
        assert_eq!(MAX_FRAME_BYTES, 1024 * 1024);
    }

    #[test]
    fn client_message_round_trip() {
        let msg = ClientMessage::Ping;
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: ClientMessage = serde_json::from_str(&json).expect("parse");
        assert!(matches!(parsed, ClientMessage::Ping));
    }
}
