pub mod local_llama;
pub mod ollama;
pub mod tool_parser;

use anyhow::Result;
use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::pin::Pin;

pub use local_llama::LocalLlamaProvider;
pub use ollama::OllamaProvider;

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Local,
    Ollama,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutputMode {
    NativeToolCalling,
    StrictJsonFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub context_window: u32,
    pub supports_streaming: bool,
    pub supports_native_tool_calls: bool,
    pub supports_json_mode: bool,
    pub structured_mode: StructuredOutputMode,
    pub model_family: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderHealth {
    pub healthy: bool,
    pub latency_ms: u64,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSchema>,
    pub stream: bool,
    pub think: bool,
    pub options: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_name: None,
            thinking: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_name: None,
            thinking: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_name: None,
            thinking: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn tool(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_name: Some(name.into()),
            thinking: None,
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    TextChunk { content: String },
    ThinkingChunk { content: String },
    ToolCall { call: ToolCall },
    Usage { usage: ProviderUsage },
    Done,
}

#[async_trait]
pub trait InferenceProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    async fn load_or_connect(&mut self) -> Result<()>;
    async fn chat_stream(&self, request: ChatRequest) -> Result<ProviderStream>;
    async fn cancel(&self, task_id: &str) -> Result<()>;
    async fn health(&self) -> Result<ProviderHealth>;
    async fn capabilities(&self) -> Result<ProviderCapabilities>;
    async fn shutdown(&self) -> Result<()>;
}
