use crate::tool_parser::{
    extract_partial_execute_shell_command, parse_json_tool_call, parse_tagged_tool_calls,
};
use crate::{
    ChatMessage, ChatRequest, InferenceProvider, ProviderCapabilities, ProviderEvent,
    ProviderHealth, ProviderKind, ProviderStream, ProviderUsage, StructuredOutputMode, ToolCall,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;

#[cfg(feature = "local-runtime")]
use llama_cpp_2::context::LlamaContext;
#[cfg(feature = "local-runtime")]
use llama_cpp_2::context::params::LlamaContextParams;
#[cfg(feature = "local-runtime")]
use llama_cpp_2::llama_backend::LlamaBackend;
#[cfg(feature = "local-runtime")]
use llama_cpp_2::llama_batch::LlamaBatch;
#[cfg(feature = "local-runtime")]
use llama_cpp_2::model::params::LlamaModelParams;
#[cfg(feature = "local-runtime")]
use llama_cpp_2::model::{AddBos, LlamaChatTemplate, LlamaModel};
#[cfg(feature = "local-runtime")]
use llama_cpp_2::sampling::LlamaSampler;
#[cfg(feature = "local-runtime")]
use llama_cpp_2::token::{LlamaToken, logit_bias::LlamaLogitBias};

#[derive(Debug, Clone)]
pub struct LocalLlamaProvider {
    pub model_path: String,
    context_tokens: u32,
    gpu_layers: i32,
    threads: i32,
    #[cfg(feature = "local-runtime")]
    runtime: Arc<Mutex<Option<Arc<LoadedRuntime>>>>,
    #[cfg(feature = "local-runtime")]
    embedding_runtime: Arc<Mutex<Option<Arc<LoadedEmbeddingRuntime>>>>,
    #[cfg(feature = "local-runtime")]
    embedding_request_lock: Arc<Mutex<()>>,
    #[cfg(feature = "local-runtime")]
    generation_request_lock: Arc<Mutex<()>>,
    cancel_flags: Arc<Mutex<BTreeMap<String, Arc<AtomicBool>>>>,
}

#[cfg(feature = "local-runtime")]
#[derive(Debug)]
struct LoadedRuntime {
    model_path: String,
    context_tokens: u32,
    gpu_layers: i32,
    threads: i32,
    model: LlamaModel,
    chat_template: Option<LlamaChatTemplate>,
    ascii_token_biases: Vec<LlamaLogitBias>,
    newline_token_biases: Vec<LlamaLogitBias>,
    backend: LlamaBackend,
}

#[cfg(feature = "local-runtime")]
#[derive(Debug)]
struct LoadedEmbeddingRuntime {
    model_path: String,
    context_tokens: u32,
    threads: i32,
    model: LlamaModel,
    // Keep the base runtime alive so the backend used by the embedding model
    // remains valid for the model lifetime.
    _base_runtime: Arc<LoadedRuntime>,
}

#[cfg(feature = "local-runtime")]
#[derive(Debug, serde::Deserialize)]
struct ParsedToolEnvelope {
    #[serde(default)]
    message: Option<ParsedAssistantMessage>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ParsedToolCall>,
}

#[cfg(feature = "local-runtime")]
#[derive(Debug, serde::Deserialize)]
struct ParsedAssistantMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ParsedToolCall>,
}

#[cfg(feature = "local-runtime")]
#[derive(Debug, serde::Deserialize)]
struct ParsedToolCall {
    #[serde(default)]
    function: Option<ParsedToolFunction>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<serde_json::Value>,
}

#[cfg(feature = "local-runtime")]
#[derive(Debug, serde::Deserialize)]
struct ParsedToolFunction {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[cfg(feature = "local-runtime")]
const GEMMA4_MINIMAL_CHAT_TEMPLATE: &str = r#"
{{- '<bos>' -}}
{%- set loop_messages = messages -%}
{%- if tools or messages[0]['role'] in ['system', 'developer'] -%}
    {{- '<|turn>system\n' -}}
    {%- if messages[0]['role'] in ['system', 'developer'] -%}
        {{- messages[0]['content'] | trim -}}
        {%- set loop_messages = messages[1:] -%}
    {%- endif -%}
    {%- if tools -%}
        {{- '\n\nAvailable tools. When a tool is needed, emit exactly one call as <|tool_call>call:name{arg:<|"|>value<|"|>}<tool_call|>.' -}}
        {%- for tool in tools -%}
            {{- '<|tool>declaration:' + tool['function']['name'] + '{description:<|"|>' + tool['function']['description'] + '<|"|>}<tool|>' -}}
        {%- endfor -%}
    {%- endif -%}
    {{- '<turn|>\n' -}}
{%- endif -%}
{%- for message in loop_messages -%}
    {%- if message['role'] == 'tool' -%}
        {{- '<|tool_response>response:' + (message.get('name') | default(message.get('tool_call_id') | default('tool'))) + '{value:<|"|>' + (message['content'] | trim) + '<|"|>}<tool_response|>' -}}
    {%- else -%}
        {%- set role = 'model' if message['role'] == 'assistant' else message['role'] -%}
        {{- '<|turn>' + role + '\n' -}}
        {{- message['content'] | trim -}}
        {{- '<turn|>\n' -}}
    {%- endif -%}
{%- endfor -%}
{%- if add_generation_prompt -%}
    {{- '<|turn>model\n' -}}
{%- endif -%}
"#;

impl LocalLlamaProvider {
    pub fn new(
        model_path: impl Into<String>,
        context_tokens: u32,
        gpu_layers: i32,
        threads: i32,
    ) -> Self {
        Self {
            model_path: model_path.into(),
            context_tokens: context_tokens.max(1),
            gpu_layers,
            threads,
            #[cfg(feature = "local-runtime")]
            runtime: Arc::new(Mutex::new(None)),
            #[cfg(feature = "local-runtime")]
            embedding_runtime: Arc::new(Mutex::new(None)),
            #[cfg(feature = "local-runtime")]
            embedding_request_lock: Arc::new(Mutex::new(())),
            #[cfg(feature = "local-runtime")]
            generation_request_lock: Arc::new(Mutex::new(())),
            cancel_flags: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn decode_tool_call_name_and_args(
        name: Option<String>,
        arguments: Option<serde_json::Value>,
    ) -> Option<ToolCall> {
        let name = name?.trim().to_string();
        if name.is_empty() {
            return None;
        }
        let arguments = match arguments {
            None => serde_json::Value::Object(serde_json::Map::new()),
            Some(serde_json::Value::String(raw)) => {
                serde_json::from_str(raw.trim()).unwrap_or(serde_json::Value::String(raw))
            }
            Some(v) => v,
        };
        Some(ToolCall { name, arguments })
    }

    fn normalize_model_family(model_path: &str) -> String {
        let lower = model_path.to_ascii_lowercase();
        if lower.contains("gemma") {
            "gemma".to_string()
        } else if lower.contains("qwen") {
            "qwen".to_string()
        } else if lower.contains("llama") {
            "llama".to_string()
        } else if lower.contains("mistral") {
            "mistral".to_string()
        } else {
            "local".to_string()
        }
    }

    fn max_output_tokens(request: &ChatRequest) -> usize {
        let extract_int = |key: &str| {
            request.options.get(key).and_then(|v| {
                v.as_i64()
                    .and_then(|n| usize::try_from(n.max(0)).ok())
                    .or_else(|| {
                        v.as_u64().and_then(|n| {
                            let n = n.min(i64::MAX as u64);
                            usize::try_from(n).ok()
                        })
                    })
            })
        };
        extract_int("num_predict")
            .or_else(|| extract_int("max_tokens"))
            .unwrap_or(384)
            .clamp(1, 4096)
    }

    fn string_list_option(request: &ChatRequest, key: &str) -> Vec<String> {
        match request.options.get(key) {
            Some(serde_json::Value::String(s)) if !s.is_empty() => vec![s.clone()],
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.as_str())
                .map(ToString::to_string)
                .filter(|s| !s.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    }

    fn string_option(request: &ChatRequest, key: &str) -> Option<String> {
        request
            .options
            .get(key)
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string)
    }

    fn bool_option(request: &ChatRequest, key: &str) -> bool {
        request
            .options
            .get(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    fn f32_option(request: &ChatRequest, key: &str) -> Option<f32> {
        request
            .options
            .get(key)
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
    }

    fn i32_option(request: &ChatRequest, key: &str) -> Option<i32> {
        request.options.get(key).and_then(|value| {
            value
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .or_else(|| value.as_u64().and_then(|n| i32::try_from(n).ok()))
        })
    }

    fn u32_option(request: &ChatRequest, key: &str) -> Option<u32> {
        request.options.get(key).and_then(|value| {
            value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .or_else(|| value.as_i64().and_then(|n| u32::try_from(n).ok()))
        })
    }

    fn first_stop_match(text: &str, stops: &[String]) -> Option<usize> {
        stops.iter().filter_map(|stop| text.find(stop)).min()
    }

    #[cfg(feature = "local-runtime")]
    fn build_model_params(gpu_layers: i32) -> LlamaModelParams {
        let mut model_params = LlamaModelParams::default().with_use_mmap(true);
        if gpu_layers >= 0 {
            model_params = model_params.with_n_gpu_layers(gpu_layers as u32);
        } else {
            model_params = model_params.with_n_gpu_layers(1000);
        }
        model_params
    }

    fn to_openai_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
        messages
            .iter()
            .map(|m| {
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "role".to_string(),
                    serde_json::Value::String(m.role.clone()),
                );
                obj.insert(
                    "content".to_string(),
                    serde_json::Value::String(m.content.clone()),
                );
                if m.role == "assistant" && !m.tool_calls.is_empty() {
                    let calls = m
                        .tool_calls
                        .iter()
                        .enumerate()
                        .map(|(i, call)| {
                            let args = if call.arguments.is_object() || call.arguments.is_array() {
                                call.arguments.to_string()
                            } else {
                                call.arguments
                                    .as_str()
                                    .map(ToString::to_string)
                                    .unwrap_or_else(|| "{}".to_string())
                            };
                            json!({
                                "id": format!("call_{i}"),
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": args,
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    obj.insert("tool_calls".to_string(), serde_json::Value::Array(calls));
                }
                if m.role == "tool"
                    && let Some(tool_name) = &m.tool_name
                {
                    obj.insert(
                        "tool_call_id".to_string(),
                        serde_json::Value::String(tool_name.clone()),
                    );
                }
                serde_json::Value::Object(obj)
            })
            .collect()
    }

    fn to_openai_tools(tools: &[crate::ToolSchema]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect()
    }

    #[cfg(feature = "local-runtime")]
    async fn ensure_runtime(&self) -> Result<Arc<LoadedRuntime>> {
        if let Some(existing) = self.runtime.lock().await.as_ref().cloned() {
            return Ok(existing);
        }

        let model_path = self.model_path.clone();
        let context_tokens = self.context_tokens;
        let gpu_layers = self.gpu_layers;
        let threads = self.threads;

        let loaded = tokio::task::spawn_blocking(move || {
            if !Path::new(&model_path).exists() {
                bail!(
                    "local model file does not exist: {} (set [model] paths or use provider=ollama)",
                    model_path
                );
            }

            let mut backend = match LlamaBackend::init() {
                Ok(b) => b,
                Err(e) => {
                    bail!(
                        "failed to initialize local llama backend (process-global singleton): {e}"
                    );
                }
            };
            backend.void_logs();

            let model_params = Self::build_model_params(gpu_layers);

            let model = LlamaModel::load_from_file(&backend, Path::new(&model_path), &model_params)
                .with_context(|| format!("failed to load local model {}", model_path))?;

            let lower_model_path = model_path.to_ascii_lowercase();
            let chat_template = Self::select_chat_template(&model, &lower_model_path);
            let ascii_token_biases = Self::ascii_only_token_biases(&model);
            let newline_token_biases = Self::newline_token_biases(&model);

            Ok::<Arc<LoadedRuntime>, anyhow::Error>(Arc::new(LoadedRuntime {
                model_path,
                context_tokens,
                gpu_layers,
                threads,
                model,
                chat_template,
                ascii_token_biases,
                newline_token_biases,
                backend,
            }))
        })
        .await
        .context("join local runtime loader")??;

        let mut guard = self.runtime.lock().await;
        if let Some(existing) = guard.as_ref().cloned() {
            return Ok(existing);
        }
        *guard = Some(loaded.clone());
        Ok(loaded)
    }

    #[cfg(feature = "local-runtime")]
    fn select_chat_template(
        model: &LlamaModel,
        lower_model_path: &str,
    ) -> Option<LlamaChatTemplate> {
        if lower_model_path.contains("gemma4") || lower_model_path.contains("gemma-4") {
            return LlamaChatTemplate::new(GEMMA4_MINIMAL_CHAT_TEMPLATE).ok();
        }

        if let Ok(template) = model.chat_template(None)
            && Self::chat_template_looks_renderable(&template)
        {
            return Some(template);
        }

        LlamaChatTemplate::new("chatml").ok()
    }

    #[cfg(feature = "local-runtime")]
    fn chat_template_looks_renderable(template: &LlamaChatTemplate) -> bool {
        let Ok(src) = template.to_str() else {
            return false;
        };
        let trimmed = src.trim();
        if trimmed.is_empty() {
            return false;
        }
        if matches!(
            trimmed,
            "gemma4" | "gemma-4" | "gemma" | "llama3" | "chatml"
        ) {
            return false;
        }
        trimmed.contains("{{")
            || trimmed.contains("{%")
            || trimmed.contains("<|turn>")
            || trimmed.contains("<start_of_turn>")
            || trimmed.contains("<|im_start|>")
    }

    #[cfg(feature = "local-runtime")]
    async fn ensure_embedding_runtime(
        &self,
        embed_model_path: &str,
    ) -> Result<Arc<LoadedEmbeddingRuntime>> {
        if let Some(existing) = self.embedding_runtime.lock().await.as_ref().cloned()
            && existing.model_path == embed_model_path
        {
            return Ok(existing);
        }

        let base_runtime = self.ensure_runtime().await?;
        let model_path = embed_model_path.to_string();
        let loaded = tokio::task::spawn_blocking({
            let base_runtime = base_runtime.clone();
            move || {
                if !Path::new(&model_path).exists() {
                    bail!("embedding model file does not exist: {}", model_path);
                }
                let model = LlamaModel::load_from_file(
                    &base_runtime.backend,
                    Path::new(&model_path),
                    &Self::build_model_params(base_runtime.gpu_layers),
                )
                .with_context(|| format!("failed to load embedding model {}", model_path))?;
                Ok::<Arc<LoadedEmbeddingRuntime>, anyhow::Error>(Arc::new(LoadedEmbeddingRuntime {
                    model_path,
                    context_tokens: base_runtime.context_tokens,
                    threads: base_runtime.threads,
                    model,
                    _base_runtime: base_runtime,
                }))
            }
        })
        .await
        .context("join local embedding runtime loader")??;

        let mut guard = self.embedding_runtime.lock().await;
        if let Some(existing) = guard.as_ref().cloned()
            && existing.model_path == loaded.model_path
        {
            return Ok(existing);
        }
        *guard = Some(loaded.clone());
        Ok(loaded)
    }

    #[cfg(feature = "local-runtime")]
    fn parse_oaicompat_tool_calls(parsed_json: &str) -> Vec<ToolCall> {
        let parsed = match serde_json::from_str::<ParsedToolEnvelope>(parsed_json) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let mut calls = Vec::new();
        let mut push_call = |call: ParsedToolCall| {
            if let Some(function) = call.function {
                if let Some(tc) = Self::decode_tool_call_name_and_args(
                    Some(function.name),
                    Some(function.arguments),
                ) {
                    calls.push(tc);
                }
                return;
            }
            if let Some(tc) = Self::decode_tool_call_name_and_args(call.name, call.arguments) {
                calls.push(tc);
            }
        };

        if let Some(msg) = parsed.message {
            for call in msg.tool_calls {
                push_call(call);
            }
        }
        for call in parsed.tool_calls {
            push_call(call);
        }
        calls
    }

    #[cfg(feature = "local-runtime")]
    fn parse_oaicompat_content(parsed_json: &str) -> Option<String> {
        let parsed = serde_json::from_str::<ParsedToolEnvelope>(parsed_json).ok()?;
        if let Some(msg) = parsed.message
            && let Some(content) = msg.content
            && !content.is_empty()
        {
            return Some(content);
        }
        if let Some(content) = parsed.content
            && !content.is_empty()
        {
            return Some(content);
        }
        None
    }

    #[cfg(feature = "local-runtime")]
    fn build_context_params(
        runtime: &LoadedRuntime,
        n_ctx: u32,
        prompt_batch_tokens: u32,
    ) -> LlamaContextParams {
        let prompt_batch_tokens = prompt_batch_tokens.max(1).min(n_ctx.max(1));
        let mut params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(prompt_batch_tokens)
            .with_n_ubatch(prompt_batch_tokens);
        if runtime.threads > 0 {
            params = params
                .with_n_threads(runtime.threads)
                .with_n_threads_batch(runtime.threads);
        }
        params
    }

    #[cfg(feature = "local-runtime")]
    fn decode_prompt_tokens(
        ctx: &mut LlamaContext<'_>,
        prompt_tokens: &[LlamaToken],
        prompt_batch_tokens: usize,
    ) -> Result<()> {
        let prompt_batch_tokens = prompt_batch_tokens.max(1);
        let mut batch = LlamaBatch::new(prompt_batch_tokens, 1);
        let prompt_len = prompt_tokens.len();
        for chunk_start in (0..prompt_len).step_by(prompt_batch_tokens) {
            batch.clear();
            let chunk_end = (chunk_start + prompt_batch_tokens).min(prompt_len);
            for (offset, token) in prompt_tokens[chunk_start..chunk_end].iter().enumerate() {
                let absolute_idx = chunk_start + offset;
                let pos = i32::try_from(absolute_idx).unwrap_or(i32::MAX);
                batch
                    .add(*token, pos, &[0], absolute_idx + 1 == prompt_len)
                    .with_context(|| {
                        format!(
                            "fill local prompt batch token={} pos={} batch_tokens={}",
                            token.0, pos, prompt_batch_tokens
                        )
                    })?;
            }
            ctx.decode(&mut batch).with_context(|| {
                format!(
                    "local prompt decode chunk_start={} chunk_len={} prompt_len={} batch_tokens={}",
                    chunk_start,
                    chunk_end.saturating_sub(chunk_start),
                    prompt_len,
                    prompt_batch_tokens
                )
            })?;
        }
        Ok(())
    }

    #[cfg(feature = "local-runtime")]
    fn invalid_generation_token_biases(
        model: &LlamaModel,
        include_eog: bool,
    ) -> Vec<LlamaLogitBias> {
        let mut biases = Vec::new();
        let n_vocab = model.n_vocab().max(0);
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        for id in 0..n_vocab {
            let token = LlamaToken(id);
            if include_eog && model.is_eog_token(token) {
                biases.push(LlamaLogitBias::new(token, -1.0e9));
                continue;
            }
            if id >= 512 {
                continue;
            }
            let invalid = match model.token_to_piece(token, &mut decoder, true, None) {
                Ok(piece) => {
                    let normalized = piece.trim().to_ascii_lowercase();
                    normalized.is_empty()
                        || piece
                            .chars()
                            .any(|c| c.is_control() && c != '\n' && c != '\t')
                        || normalized.starts_with("<unused")
                        || normalized == "<unk>"
                        || normalized == "<pad>"
                        || normalized == "<bos>"
                        || normalized == "<eos>"
                        || normalized == "<mask>"
                }
                Err(_) => true,
            };
            if invalid {
                biases.push(LlamaLogitBias::new(token, -1.0e9));
            }
        }
        biases
    }

    #[cfg(feature = "local-runtime")]
    fn ascii_only_token_biases(model: &LlamaModel) -> Vec<LlamaLogitBias> {
        let mut biases = Vec::new();
        let n_vocab = model.n_vocab().max(0);
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        for id in 0..n_vocab {
            let token = LlamaToken(id);
            let non_ascii = match model.token_to_piece(token, &mut decoder, true, None) {
                Ok(piece) => piece
                    .chars()
                    .any(|ch| !ch.is_ascii() && !matches!(ch, '▁' | 'Ġ')),
                Err(_) => false,
            };
            if non_ascii {
                biases.push(LlamaLogitBias::new(token, -1.0e9));
            }
        }
        biases
    }

    #[cfg(feature = "local-runtime")]
    fn newline_token_biases(model: &LlamaModel) -> Vec<LlamaLogitBias> {
        let mut biases = Vec::new();
        let n_vocab = model.n_vocab().max(0);
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        for id in 0..n_vocab {
            let token = LlamaToken(id);
            let has_newline = match model.token_to_piece(token, &mut decoder, true, None) {
                Ok(piece) => piece.contains('\n') || piece.contains('\r'),
                Err(_) => false,
            };
            if has_newline {
                biases.push(LlamaLogitBias::new(token, -1.0e9));
            }
        }
        biases
    }

    #[cfg(feature = "local-runtime")]
    fn first_token_prefix_biases(model: &LlamaModel, prefixes: &[String]) -> Vec<LlamaLogitBias> {
        let normalized_prefixes = prefixes
            .iter()
            .map(|prefix| prefix.trim().to_ascii_lowercase())
            .filter(|prefix| !prefix.is_empty())
            .collect::<Vec<_>>();
        if normalized_prefixes.is_empty() {
            return Vec::new();
        }
        let mut biases = Vec::new();
        let n_vocab = model.n_vocab().max(0);
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        for id in 0..n_vocab {
            let token = LlamaToken(id);
            if model.is_eog_token(token) {
                continue;
            }
            let allowed = match model.token_to_piece(token, &mut decoder, true, None) {
                Ok(piece) => {
                    let normalized = Self::normalize_first_token_piece(&piece);
                    !normalized.is_empty() && normalized_prefixes.contains(&normalized)
                }
                Err(_) => false,
            };
            if !allowed {
                biases.push(LlamaLogitBias::new(token, -1.0e9));
            }
        }
        biases
    }

    #[cfg(feature = "local-runtime")]
    fn single_token_string_biases(model: &LlamaModel, values: &[String]) -> Vec<LlamaLogitBias> {
        let mut biases = Vec::new();
        let mut seen = HashSet::new();
        for value in values {
            if value.trim().is_empty() {
                continue;
            }
            if let Ok(tokens) = model.str_to_token(value, AddBos::Never)
                && tokens.len() == 1
                && seen.insert(tokens[0])
            {
                biases.push(LlamaLogitBias::new(tokens[0], -1.0e9));
            }
        }
        biases
    }

    #[cfg(feature = "local-runtime")]
    fn eog_token_biases(model: &LlamaModel) -> Vec<LlamaLogitBias> {
        let n_vocab = model.n_vocab().max(0);
        (0..n_vocab)
            .map(LlamaToken)
            .filter(|token| model.is_eog_token(*token))
            .map(|token| LlamaLogitBias::new(token, -1.0e9))
            .collect()
    }

    #[cfg(feature = "local-runtime")]
    fn normalize_first_token_piece(piece: &str) -> String {
        let normalized = piece
            .trim()
            .trim_start_matches(['▁', 'Ġ'])
            .trim()
            .trim_matches(|ch: char| matches!(ch, '`' | '"' | '\'' | '$'));
        if normalized
            .chars()
            .next()
            .map(|ch| ch.is_ascii_uppercase())
            .unwrap_or(false)
        {
            String::new()
        } else {
            normalized.to_ascii_lowercase()
        }
    }

    #[cfg(feature = "local-runtime")]
    fn build_generation_sampler(
        model: &LlamaModel,
        grammar: Option<&str>,
        biases: &[LlamaLogitBias],
        request: &ChatRequest,
    ) -> LlamaSampler {
        let invalid_token_filter = || LlamaSampler::logit_bias(model.n_vocab(), biases);
        let temperature = Self::f32_option(request, "temperature").unwrap_or(0.0);
        let top_k = Self::i32_option(request, "top_k").unwrap_or(0);
        let top_p = Self::f32_option(request, "top_p").unwrap_or(0.0);
        let min_p = Self::f32_option(request, "min_p").unwrap_or(0.0);
        let seed = Self::u32_option(request, "seed").unwrap_or(0x5445_524d);
        let mut samplers = Vec::new();
        samplers.push(invalid_token_filter());
        if let Some(grammar) = grammar
            && let Ok(grammar_sampler) = LlamaSampler::grammar(model, grammar, "root")
        {
            samplers.push(grammar_sampler);
        }
        if temperature > 0.0 {
            if top_k > 0 {
                samplers.push(LlamaSampler::top_k(top_k));
            }
            if top_p > 0.0 && top_p < 1.0 {
                samplers.push(LlamaSampler::top_p(top_p, 1));
            }
            if min_p > 0.0 {
                samplers.push(LlamaSampler::min_p(min_p, 1));
            }
            samplers.push(LlamaSampler::temp(temperature));
            samplers.push(LlamaSampler::dist(seed));
        } else {
            samplers.push(LlamaSampler::greedy());
        }
        LlamaSampler::chain_simple(samplers)
    }

    #[cfg(feature = "local-runtime")]
    fn normalize_embedding_dim(row: &[f32], dim: usize) -> Vec<f32> {
        let mut out = if row.len() >= dim {
            row[..dim].to_vec()
        } else {
            let mut v = vec![0.0f32; dim];
            let len = row.len();
            v[..len].copy_from_slice(row);
            v
        };
        let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut out {
                *v /= norm;
            }
        }
        out
    }

    #[cfg(feature = "local-runtime")]
    fn embed_texts_blocking(
        embedding_runtime: Arc<LoadedEmbeddingRuntime>,
        embed_dim: usize,
        texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let model_ctx = embedding_runtime.model.n_ctx_train();
        // BGE command-doc chunks are capped near 512 tokens. Reusing the chat
        // context size here makes every embedding allocate an oversized batch.
        let requested_ctx = embedding_runtime.context_tokens.clamp(128, 512);
        let n_ctx = if model_ctx > 0 {
            requested_ctx.min(model_ctx).max(128)
        } else {
            requested_ctx
        };
        let mut ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(n_ctx)
            // Encoder path in llama.cpp requires n_ubatch >= submitted token count.
            // Keep ubatch aligned with batch/context to avoid runtime asserts while
            // embedding long chunks during install/index bootstrap.
            .with_n_ubatch(n_ctx)
            .with_embeddings(true);
        if embedding_runtime.threads > 0 {
            ctx_params = ctx_params
                .with_n_threads(embedding_runtime.threads)
                .with_n_threads_batch(embedding_runtime.threads);
        }
        let mut ctx = embedding_runtime
            .model
            .new_context(&embedding_runtime._base_runtime.backend, ctx_params)
            .context("create local embedding context")?;

        let mut rows = Vec::with_capacity(texts.len());
        let n_vocab = embedding_runtime.model.n_vocab();
        for text in texts {
            let mut tokens = embedding_runtime
                .model
                .str_to_token(&text, AddBos::Never)
                .or_else(|_| embedding_runtime.model.str_to_token(&text, AddBos::Always))
                .context("tokenize embedding input")?;
            tokens.retain(|tok| tok.0 >= 0 && tok.0 < n_vocab);
            if tokens.is_empty() {
                rows.push(vec![0.0f32; embed_dim]);
                continue;
            }
            if tokens.len() >= n_ctx as usize {
                tokens.truncate((n_ctx as usize).saturating_sub(1).max(1));
            }

            ctx.clear_kv_cache();
            let mut batch = LlamaBatch::new(tokens.len(), 1);
            batch
                .add_sequence(&tokens, 0, false)
                .context("prepare embedding batch")?;
            ctx.encode(&mut batch).context("encode embedding batch")?;
            let embedding = ctx.embeddings_seq_ith(0).context("read embedding vector")?;
            rows.push(Self::normalize_embedding_dim(embedding, embed_dim));
        }

        Ok(rows)
    }

    pub async fn embed_texts(
        &self,
        embed_model_path: &str,
        embed_dim: usize,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>> {
        #[cfg(not(feature = "local-runtime"))]
        {
            let _ = (embed_model_path, embed_dim, texts);
            bail!("local-runtime feature is disabled at build time")
        }

        #[cfg(feature = "local-runtime")]
        {
            // Local embedding/tokenization over a shared model can trigger
            // intermittent llama.cpp asserts under concurrent requests.
            // Serialize embedding requests to keep startup/reindex stable.
            let _embed_guard = self.embedding_request_lock.lock().await;
            let embedding_runtime = self.ensure_embedding_runtime(embed_model_path).await?;
            let inputs = texts.to_vec();
            tokio::task::spawn_blocking(move || {
                Self::embed_texts_blocking(embedding_runtime, embed_dim, inputs)
            })
            .await
            .context("join local embedding task")?
        }
    }

    #[cfg(feature = "local-runtime")]
    fn run_generation_blocking(
        runtime: Arc<LoadedRuntime>,
        request: ChatRequest,
        cancel_flag: Option<Arc<AtomicBool>>,
        tx: tokio::sync::mpsc::Sender<Result<ProviderEvent>>,
    ) -> Result<()> {
        let messages_json = serde_json::to_string(&Self::to_openai_messages(&request.messages))?;
        let tools_json = if request.tools.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&Self::to_openai_tools(
                &request.tools,
            ))?)
        };
        let tool_choice = if request.tools.is_empty() {
            None
        } else {
            // llama.cpp's required-tool grammar is not stable for the current
            // local Gemma build. The daemon enforces command-mode tool output
            // at the prompt/parser layer instead.
            Some("auto")
        };
        let template = runtime
            .chat_template
            .clone()
            .or_else(|| LlamaChatTemplate::new("chatml").ok())
            .ok_or_else(|| anyhow!("chat template unavailable for local model"))?;

        let template_result = runtime
            .model
            .apply_chat_template_oaicompat(
                &template,
                &llama_cpp_2::openai::OpenAIChatTemplateParams {
                    messages_json: &messages_json,
                    tools_json: tools_json.as_deref(),
                    tool_choice,
                    json_schema: None,
                    grammar: None,
                    reasoning_format: None,
                    chat_template_kwargs: Some("{}"),
                    add_generation_prompt: true,
                    use_jinja: true,
                    parallel_tool_calls: false,
                    enable_thinking: request.think,
                    add_bos: false,
                    add_eos: false,
                    parse_tool_calls: true,
                },
            )
            .context("apply local chat template")?;
        if let Some(path) = std::env::var_os("TERMLM_DEBUG_LOCAL_PROMPT_PATH") {
            let _ = std::fs::write(path, &template_result.prompt);
        }
        let lower_runtime_model_path = runtime.model_path.to_ascii_lowercase();
        let rendered_prompt_has_bos = template_result.prompt.trim_start().starts_with("<bos>");
        let mut prompt_tokens =
            if lower_runtime_model_path.contains("gemma") && !rendered_prompt_has_bos {
                runtime
                    .model
                    .str_to_token(&template_result.prompt, AddBos::Always)
                    .or_else(|_| {
                        runtime
                            .model
                            .str_to_token(&template_result.prompt, AddBos::Never)
                    })
            } else if lower_runtime_model_path.contains("gemma") {
                runtime
                    .model
                    .str_to_token(&template_result.prompt, AddBos::Never)
                    .or_else(|_| {
                        runtime
                            .model
                            .str_to_token(&template_result.prompt, AddBos::Always)
                    })
            } else {
                runtime
                    .model
                    .str_to_token(&template_result.prompt, AddBos::Never)
                    .or_else(|_| {
                        runtime
                            .model
                            .str_to_token(&template_result.prompt, AddBos::Always)
                    })
            }
            .context("tokenize local prompt")?;
        let n_vocab = runtime.model.n_vocab();
        prompt_tokens.retain(|tok| tok.0 >= 0 && tok.0 < n_vocab);

        if prompt_tokens.is_empty() {
            prompt_tokens = runtime
                .model
                .str_to_token(" ", AddBos::Always)
                .context("tokenize fallback prompt")?;
            prompt_tokens.retain(|tok| tok.0 >= 0 && tok.0 < n_vocab);
        }

        let max_output_tokens = Self::max_output_tokens(&request);
        let stop_sequences = Self::string_list_option(&request, "stop_sequences");
        let model_ctx = runtime.model.n_ctx_train();
        let mut n_ctx = runtime.context_tokens.max(512);
        if model_ctx > 0 {
            n_ctx = n_ctx.min(model_ctx).max(512);
        }
        let reserve_tokens = max_output_tokens.saturating_add(8).max(32);
        let max_prompt_tokens = (n_ctx as usize).saturating_sub(reserve_tokens).max(1);
        if prompt_tokens.len() > max_prompt_tokens {
            let trim_start = prompt_tokens.len().saturating_sub(max_prompt_tokens);
            prompt_tokens = prompt_tokens[trim_start..].to_vec();
        }
        let prompt_token_count = prompt_tokens.len() as u64;
        let required_ctx = (prompt_tokens.len().saturating_add(reserve_tokens))
            .max(512)
            .min(u32::MAX as usize) as u32;
        n_ctx = n_ctx.max(required_ctx);
        if model_ctx > 0 {
            n_ctx = n_ctx.min(model_ctx).max(512);
        }

        let preferred_prompt_batch = (prompt_tokens.len().max(1).min(n_ctx as usize)) as u32;
        let mut batch_candidates = Vec::new();
        for candidate in [
            preferred_prompt_batch,
            n_ctx.clamp(1, 1024),
            preferred_prompt_batch.min(512),
            preferred_prompt_batch.min(256),
            preferred_prompt_batch.min(128),
            preferred_prompt_batch.min(64),
            preferred_prompt_batch.min(32),
            preferred_prompt_batch.min(16),
            preferred_prompt_batch.min(8),
            preferred_prompt_batch.min(4),
            preferred_prompt_batch.min(1),
        ] {
            if candidate > 0 && !batch_candidates.contains(&candidate) {
                batch_candidates.push(candidate);
            }
        }
        let mut ctx = None;
        let mut last_decode_error = None;
        for prompt_batch_tokens in batch_candidates {
            let mut candidate_ctx = runtime
                .model
                .new_context(
                    &runtime.backend,
                    Self::build_context_params(&runtime, n_ctx, prompt_batch_tokens),
                )
                .with_context(|| {
                    format!(
                        "create local llama context n_ctx={} batch_tokens={}",
                        n_ctx, prompt_batch_tokens
                    )
                })?;
            match Self::decode_prompt_tokens(
                &mut candidate_ctx,
                &prompt_tokens,
                prompt_batch_tokens as usize,
            ) {
                Ok(()) => {
                    ctx = Some(candidate_ctx);
                    break;
                }
                Err(err) => {
                    last_decode_error = Some(err);
                }
            }
        }
        let mut ctx = ctx.ok_or_else(|| {
            anyhow!(
                "local prompt decode failed after retrying batch sizes: {}",
                last_decode_error
                    .map(|err| format!("{err:#}"))
                    .unwrap_or_else(|| "unknown decode error".to_string())
            )
        })?;

        let mut preserved_tokens = HashSet::new();
        for token_str in &template_result.preserved_tokens {
            if let Ok(tokens) = runtime.model.str_to_token(token_str, AddBos::Never)
                && tokens.len() == 1
            {
                preserved_tokens.insert(tokens[0]);
            }
        }

        // Generated tool grammars are not stable for the bundled local Gemma
        // template, but small caller-provided grammars are useful for compact
        // planner outputs.
        let request_grammar = Self::string_option(&request, "grammar");
        let grammar = request_grammar.as_deref();
        let ascii_only = Self::bool_option(&request, "ascii_only");
        let mut first_token_biases = Self::invalid_generation_token_biases(&runtime.model, true);
        let suppress_eog = Self::bool_option(&request, "suppress_eog");
        let mut continuation_biases = Self::invalid_generation_token_biases(
            &runtime.model,
            grammar.is_some() || suppress_eog,
        );
        if suppress_eog {
            first_token_biases.extend(Self::eog_token_biases(&runtime.model));
            continuation_biases.extend(Self::eog_token_biases(&runtime.model));
            first_token_biases.extend(Self::single_token_string_biases(
                &runtime.model,
                &template_result.additional_stops,
            ));
            continuation_biases.extend(Self::single_token_string_biases(
                &runtime.model,
                &template_result.additional_stops,
            ));
        }
        if ascii_only {
            first_token_biases.extend(runtime.ascii_token_biases.clone());
            continuation_biases.extend(runtime.ascii_token_biases.clone());
        }
        if Self::bool_option(&request, "suppress_newline") {
            first_token_biases.extend(runtime.newline_token_biases.clone());
            continuation_biases.extend(runtime.newline_token_biases.clone());
        }
        let first_token_prefixes = Self::string_list_option(&request, "first_token_prefixes");
        if !first_token_prefixes.is_empty() {
            first_token_biases.extend(Self::first_token_prefix_biases(
                &runtime.model,
                &first_token_prefixes,
            ));
        }
        let mut first_token_sampler =
            Self::build_generation_sampler(&runtime.model, grammar, &first_token_biases, &request);
        let mut continuation_sampler =
            Self::build_generation_sampler(&runtime.model, grammar, &continuation_biases, &request);

        let mut generated = String::new();
        let mut completion_tokens = 0_u64;
        let mut n_cur = i32::try_from(prompt_tokens.len()).unwrap_or(i32::MAX);
        let max_tokens = n_cur.saturating_add(i32::try_from(max_output_tokens).unwrap_or(2048));
        let mut decoder = encoding_rs::UTF_8.new_decoder();

        while n_cur < max_tokens {
            if cancel_flag
                .as_ref()
                .map(|flag| flag.load(Ordering::Relaxed))
                .unwrap_or(false)
            {
                bail!("local provider request cancelled");
            }

            let token = if completion_tokens == 0 {
                first_token_sampler.sample(&ctx, -1)
            } else {
                continuation_sampler.sample(&ctx, -1)
            };
            if runtime.model.is_eog_token(token) {
                break;
            }
            completion_tokens = completion_tokens.saturating_add(1);

            let piece = runtime
                .model
                .token_to_piece(token, &mut decoder, preserved_tokens.contains(&token), None)
                .with_context(|| format!("decode sampled token piece token={}", token.0))?;
            if !piece.is_empty() {
                generated.push_str(&piece);
                if request.stream {
                    let _ = tx.blocking_send(Ok(ProviderEvent::TextChunk {
                        content: piece.clone(),
                    }));
                }
            }

            if template_result
                .additional_stops
                .iter()
                .any(|stop| !stop.is_empty() && generated.ends_with(stop))
            {
                for stop in &template_result.additional_stops {
                    if !stop.is_empty() && generated.ends_with(stop) {
                        let len = generated.len().saturating_sub(stop.len());
                        generated.truncate(len);
                        break;
                    }
                }
                break;
            }

            if let Some(stop_at) = Self::first_stop_match(&generated, &stop_sequences) {
                generated.truncate(stop_at);
                break;
            }

            // Some local templates stream complete tagged tool-call payloads and
            // then continue generating silent/non-rendered tokens for a while.
            // If we already have a valid tagged tool call, stop immediately so
            // the orchestrator can receive a ToolCall event before idle timeout.
            if generated.contains("tool_call")
                && parse_tagged_tool_calls(&generated)
                    .map(|calls| !calls.is_empty())
                    .unwrap_or(false)
            {
                break;
            }
            if generated.contains("call:execute_shell_command")
                && extract_partial_execute_shell_command(&generated).is_some()
            {
                break;
            }
            if generated.contains("\"name\"")
                && generated.contains("\"arguments\"")
                && parse_json_tool_call(&generated).is_ok()
            {
                break;
            }

            let mut next_batch = LlamaBatch::new(1, 1);
            next_batch
                .add(token, n_cur, &[0], true)
                .context("prepare sampled token batch")?;
            if let Err(e) = ctx.decode(&mut next_batch) {
                if !generated.trim().is_empty() {
                    break;
                }
                bail!("decode sampled token token={} pos={}: {e}", token.0, n_cur);
            }
            n_cur += 1;
            first_token_sampler.accept(token);
            continuation_sampler.accept(token);
        }

        if !request.stream && !generated.is_empty() {
            let _ = tx.blocking_send(Ok(ProviderEvent::TextChunk {
                content: generated.clone(),
            }));
        }

        let mut tool_calls = Vec::new();
        let mut parsed_content = None::<String>;
        if let Ok(parsed_json) = template_result.parse_response_oaicompat(&generated, false) {
            tool_calls = Self::parse_oaicompat_tool_calls(&parsed_json);
            parsed_content = Self::parse_oaicompat_content(&parsed_json);
        }

        if !generated.is_empty()
            && let Ok(parsed_tagged) = parse_tagged_tool_calls(&generated)
            && !parsed_tagged.is_empty()
        {
            // Prefer explicitly tagged tool calls from the raw generation
            // stream. Some template parsers can decode partial/inexact calls
            // from non-JSON output, while tagged calls preserve full args.
            tool_calls = parsed_tagged;
        }
        if tool_calls.is_empty()
            && let Some(partial_call) = extract_partial_execute_shell_command(&generated)
        {
            tool_calls.push(partial_call);
        }

        if !request.stream
            && let Some(content) = parsed_content
            && content != generated
            && !content.is_empty()
        {
            let _ = tx.blocking_send(Ok(ProviderEvent::TextChunk { content }));
        }

        for call in tool_calls {
            let _ = tx.blocking_send(Ok(ProviderEvent::ToolCall { call }));
        }

        let _ = tx.blocking_send(Ok(ProviderEvent::Usage {
            usage: ProviderUsage {
                prompt_tokens: prompt_token_count,
                completion_tokens,
            },
        }));
        let _ = tx.blocking_send(Ok(ProviderEvent::Done));
        Ok(())
    }
}

#[async_trait]
impl InferenceProvider for LocalLlamaProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Local
    }

    async fn load_or_connect(&mut self) -> Result<()> {
        #[cfg(not(feature = "local-runtime"))]
        {
            bail!("local-runtime feature is disabled at build time")
        }

        #[cfg(feature = "local-runtime")]
        {
            let _ = self.ensure_runtime().await?;
            Ok(())
        }
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ProviderStream> {
        #[cfg(not(feature = "local-runtime"))]
        {
            let _ = request;
            bail!("local-runtime feature is disabled at build time")
        }

        #[cfg(feature = "local-runtime")]
        {
            let runtime = self.ensure_runtime().await?;

            let task_id = request.task_id.clone();
            let cancel_flag = if let Some(id) = task_id.clone() {
                let flag = Arc::new(AtomicBool::new(false));
                self.cancel_flags.lock().await.insert(id, flag.clone());
                Some(flag)
            } else {
                None
            };

            let (tx, rx) = tokio::sync::mpsc::channel::<Result<ProviderEvent>>(256);
            let cancel_map = self.cancel_flags.clone();
            let generation_lock = self.generation_request_lock.clone();

            tokio::spawn(async move {
                // Metal-backed llama.cpp contexts are not reliably happy with
                // overlapping prompt decode/generation, especially when one
                // stream has just been cancelled for early-stop. Serialize
                // local generation so a fast prompt loop cannot corrupt the
                // next request.
                let _generation_guard = generation_lock.lock().await;
                let tx_err = tx.clone();
                let task = tokio::task::spawn_blocking(move || {
                    Self::run_generation_blocking(runtime, request, cancel_flag, tx)
                })
                .await;

                match task {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        let _ = tx_err
                            .send(Err(anyhow!("local inference failed: {err}")))
                            .await;
                    }
                    Err(err) => {
                        let _ = tx_err
                            .send(Err(anyhow!("local inference task join failed: {err}")))
                            .await;
                    }
                }

                if let Some(id) = task_id {
                    cancel_map.lock().await.remove(&id);
                }
            });

            Ok(Box::pin(ReceiverStream::new(rx)))
        }
    }

    async fn cancel(&self, task_id: &str) -> Result<()> {
        if let Some(flag) = self.cancel_flags.lock().await.get(task_id).cloned() {
            flag.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn health(&self) -> Result<ProviderHealth> {
        #[cfg(not(feature = "local-runtime"))]
        {
            return Ok(ProviderHealth {
                healthy: false,
                latency_ms: 0,
                details: "local-runtime feature disabled".to_string(),
            });
        }

        #[cfg(feature = "local-runtime")]
        {
            let started = std::time::Instant::now();
            let healthy = self
                .runtime
                .lock()
                .await
                .as_ref()
                .map(|rt| Path::new(&rt.model_path).exists())
                .unwrap_or_else(|| Path::new(&self.model_path).exists());
            Ok(ProviderHealth {
                healthy,
                latency_ms: started.elapsed().as_millis() as u64,
                details: if healthy {
                    "local llama.cpp runtime ready".to_string()
                } else {
                    format!("local model file unavailable: {}", self.model_path)
                },
            })
        }
    }

    async fn capabilities(&self) -> Result<ProviderCapabilities> {
        Ok(ProviderCapabilities {
            context_window: self.context_tokens,
            supports_streaming: true,
            supports_native_tool_calls: true,
            supports_json_mode: true,
            structured_mode: StructuredOutputMode::NativeToolCalling,
            model_family: Self::normalize_model_family(&self.model_path),
        })
    }

    async fn shutdown(&self) -> Result<()> {
        self.cancel_flags.lock().await.clear();
        #[cfg(feature = "local-runtime")]
        {
            *self.runtime.lock().await = None;
            *self.embedding_runtime.lock().await = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_tool_call_args_accepts_json_string() {
        let parsed = LocalLlamaProvider::decode_tool_call_name_and_args(
            Some("execute_shell_command".to_string()),
            Some(serde_json::Value::String(r#"{"cmd":"ls -la"}"#.to_string())),
        )
        .expect("call");
        assert_eq!(parsed.name, "execute_shell_command");
        assert_eq!(parsed.arguments["cmd"], "ls -la");
    }

    #[test]
    fn max_output_tokens_is_bounded() {
        let request = ChatRequest {
            task_id: None,
            model: "x".to_string(),
            messages: vec![],
            tools: vec![],
            stream: true,
            think: false,
            options: BTreeMap::from([("max_tokens".to_string(), json!(100_000))]),
        };
        assert_eq!(LocalLlamaProvider::max_output_tokens(&request), 4096);
    }

    #[test]
    fn model_family_normalization_prefers_gemma() {
        assert_eq!(
            LocalLlamaProvider::normalize_model_family("/tmp/gemma4-e4b.gguf"),
            "gemma"
        );
    }

    #[cfg(feature = "local-runtime")]
    #[test]
    fn parse_oaicompat_tool_call_payload() {
        let raw = r#"{
            "message": {
                "content": "",
                "tool_calls": [
                    {
                        "function": {
                            "name": "lookup_command_docs",
                            "arguments": "{\"name\":\"git\"}"
                        }
                    }
                ]
            }
        }"#;
        let calls = LocalLlamaProvider::parse_oaicompat_tool_calls(raw);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "lookup_command_docs");
        assert_eq!(calls[0].arguments["name"], "git");
    }
}
