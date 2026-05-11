use crate::tool_parser::parse_tagged_tool_calls;
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
            .clamp(16, 4096)
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

            let chat_template = model
                .chat_template(None)
                .ok()
                .or_else(|| LlamaChatTemplate::new("chatml").ok());

            Ok::<Arc<LoadedRuntime>, anyhow::Error>(Arc::new(LoadedRuntime {
                model_path,
                context_tokens,
                gpu_layers,
                threads,
                model,
                chat_template,
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
    fn build_context_params(runtime: &LoadedRuntime, n_ctx: u32) -> LlamaContextParams {
        let mut params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(n_ctx);
        if runtime.threads > 0 {
            params = params
                .with_n_threads(runtime.threads)
                .with_n_threads_batch(runtime.threads);
        }
        params
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

        let n_ctx = embedding_runtime.context_tokens.max(512);
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
        for text in texts {
            let mut tokens = embedding_runtime
                .model
                .str_to_token(&text, AddBos::Always)
                .or_else(|_| embedding_runtime.model.str_to_token(&text, AddBos::Never))
                .context("tokenize embedding input")?;
            if tokens.is_empty() {
                rows.push(vec![0.0f32; embed_dim]);
                continue;
            }
            if tokens.len() >= n_ctx as usize {
                tokens.truncate((n_ctx as usize).saturating_sub(1).max(1));
            }

            ctx.clear_kv_cache();
            let mut batch = LlamaBatch::new(n_ctx as usize, 1);
            batch
                .add_sequence(&tokens, 0, false)
                .context("prepare embedding batch")?;
            ctx.decode(&mut batch).context("decode embedding batch")?;
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
        let tools_json = serde_json::to_string(&Self::to_openai_tools(&request.tools))?;
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
                    tools_json: Some(&tools_json),
                    tool_choice: Some("auto"),
                    json_schema: None,
                    grammar: None,
                    reasoning_format: None,
                    chat_template_kwargs: Some("{}"),
                    add_generation_prompt: true,
                    use_jinja: true,
                    parallel_tool_calls: true,
                    enable_thinking: request.think,
                    add_bos: false,
                    add_eos: false,
                    parse_tool_calls: true,
                },
            )
            .context("apply local chat template")?;

        let mut prompt_tokens = runtime
            .model
            .str_to_token(&template_result.prompt, AddBos::Always)
            .or_else(|_| {
                runtime
                    .model
                    .str_to_token(&template_result.prompt, AddBos::Never)
            })
            .context("tokenize local prompt")?;

        if prompt_tokens.is_empty() {
            prompt_tokens = runtime
                .model
                .str_to_token(" ", AddBos::Always)
                .context("tokenize fallback prompt")?;
        }

        let prompt_token_count = prompt_tokens.len() as u64;
        let max_output_tokens = Self::max_output_tokens(&request);
        let n_ctx = runtime
            .context_tokens
            .max((prompt_tokens.len() + max_output_tokens + 8) as u32)
            .max(512);

        let mut ctx = runtime
            .model
            .new_context(
                &runtime.backend,
                Self::build_context_params(&runtime, n_ctx),
            )
            .context("create local llama context")?;

        let mut batch = LlamaBatch::new(n_ctx as usize, 1);
        let last_index = prompt_tokens.len().saturating_sub(1) as i32;
        for (idx, token) in (0_i32..).zip(prompt_tokens.iter()) {
            batch
                .add(*token, idx, &[0], idx == last_index)
                .context("fill local prompt batch")?;
        }
        ctx.decode(&mut batch).context("local prompt decode")?;

        let mut preserved_tokens = HashSet::new();
        for token_str in &template_result.preserved_tokens {
            if let Ok(tokens) = runtime.model.str_to_token(token_str, AddBos::Never)
                && tokens.len() == 1
            {
                preserved_tokens.insert(tokens[0]);
            }
        }

        let mut sampler = if let Some(grammar) = template_result.grammar.as_deref() {
            match LlamaSampler::grammar(&runtime.model, grammar, "root") {
                Ok(grammar_sampler) => {
                    LlamaSampler::chain_simple([grammar_sampler, LlamaSampler::greedy()])
                }
                Err(_) => LlamaSampler::greedy(),
            }
        } else {
            LlamaSampler::greedy()
        };

        let mut generated = String::new();
        let mut completion_tokens = 0_u64;
        let mut n_cur = batch.n_tokens();
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

            let sample_idx = batch.n_tokens().saturating_sub(1);
            let token = sampler.sample(&ctx, sample_idx);
            if runtime.model.is_eog_token(token) {
                break;
            }
            completion_tokens = completion_tokens.saturating_add(1);

            let piece = runtime
                .model
                .token_to_piece(token, &mut decoder, preserved_tokens.contains(&token), None)
                .context("decode sampled token")?;
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

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .context("append sampled token")?;
            n_cur += 1;
            ctx.decode(&mut batch).context("decode sampled token")?;
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

        if tool_calls.is_empty()
            && !generated.is_empty()
            && let Ok(parsed_tagged) = parse_tagged_tool_calls(&generated)
        {
            tool_calls = parsed_tagged;
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

            tokio::spawn(async move {
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
