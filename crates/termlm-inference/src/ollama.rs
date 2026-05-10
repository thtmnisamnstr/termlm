use crate::{
    ChatMessage, ChatRequest, InferenceProvider, ProviderCapabilities, ProviderEvent,
    ProviderHealth, ProviderKind, ProviderStream, ProviderUsage, StructuredOutputMode, ToolCall,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;

#[derive(Debug, Clone)]
pub struct OllamaProvider {
    pub endpoint: String,
    pub model: String,
    pub keep_alive: Option<String>,
    pub allow_remote: bool,
    pub allow_plain_http_remote: bool,
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    client: Arc<Mutex<Option<Client>>>,
    cancel_flags: Arc<Mutex<BTreeMap<String, Arc<AtomicBool>>>>,
}

#[derive(Debug, Deserialize)]
struct OllamaChatChunk {
    message: Option<OllamaMessage>,
    done: Option<bool>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    content: Option<String>,
    thinking: Option<String>,
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCall {
    function: OllamaFunction,
}

#[derive(Debug, Deserialize)]
struct OllamaFunction {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OllamaShowResponse {
    #[serde(default)]
    capabilities: Vec<String>,
    details: Option<OllamaShowDetails>,
    #[serde(default)]
    model_info: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OllamaShowDetails {
    family: Option<String>,
    families: Option<Vec<String>>,
}

struct BuildChatPayloadArgs<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    tools: &'a [crate::ToolSchema],
    stream: bool,
    think: bool,
    options: &'a BTreeMap<String, serde_json::Value>,
    keep_alive: Option<&'a str>,
    force_json_format: bool,
}

impl OllamaProvider {
    pub fn validate_endpoint(
        endpoint: &str,
        allow_remote: bool,
        allow_plain_http_remote: bool,
    ) -> Result<()> {
        let url = reqwest::Url::parse(endpoint).context("invalid endpoint URL")?;
        let host = url.host_str().ok_or_else(|| anyhow!("missing host"))?;

        let is_loopback =
            host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";

        if !is_loopback && !allow_remote {
            bail!("non-loopback ollama endpoint requires allow_remote=true");
        }

        if url.scheme() == "http" && !is_loopback && !allow_plain_http_remote {
            bail!("plain HTTP remote ollama endpoint requires allow_plain_http_remote=true");
        }

        Ok(())
    }

    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        allow_remote: bool,
        allow_plain_http_remote: bool,
        connect_timeout_secs: u64,
        request_timeout_secs: u64,
        keep_alive: impl Into<String>,
    ) -> Result<Self> {
        let endpoint = endpoint.into();
        Self::validate_endpoint(&endpoint, allow_remote, allow_plain_http_remote)?;
        let keep_alive = keep_alive.into();
        let keep_alive = if keep_alive.trim().is_empty() {
            None
        } else {
            Some(keep_alive)
        };

        Ok(Self {
            endpoint,
            model: model.into(),
            keep_alive,
            allow_remote,
            allow_plain_http_remote,
            connect_timeout_secs,
            request_timeout_secs,
            client: Arc::new(Mutex::new(None)),
            cancel_flags: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    fn build_client(&self) -> Result<Client> {
        Ok(Client::builder()
            .connect_timeout(std::time::Duration::from_secs(self.connect_timeout_secs))
            .timeout(std::time::Duration::from_secs(self.request_timeout_secs))
            .build()?)
    }

    fn chat_url(&self) -> String {
        format!("{}/api/chat", self.endpoint.trim_end_matches('/'))
    }

    fn tags_url(&self) -> String {
        format!("{}/api/tags", self.endpoint.trim_end_matches('/'))
    }

    fn show_url(&self) -> String {
        format!("{}/api/show", self.endpoint.trim_end_matches('/'))
    }

    fn serialize_messages(messages: Vec<ChatMessage>) -> Vec<serde_json::Value> {
        messages
            .into_iter()
            .map(|m| {
                let mut obj = serde_json::Map::new();
                obj.insert("role".to_string(), serde_json::Value::String(m.role));
                obj.insert("content".to_string(), serde_json::Value::String(m.content));

                if let Some(name) = m.tool_name {
                    obj.insert("tool_name".to_string(), serde_json::Value::String(name));
                }

                if let Some(thinking) = m.thinking {
                    obj.insert("thinking".to_string(), serde_json::Value::String(thinking));
                }

                if !m.tool_calls.is_empty() {
                    let calls = m
                        .tool_calls
                        .into_iter()
                        .map(|tc| {
                            serde_json::json!({
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments,
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    obj.insert("tool_calls".to_string(), serde_json::Value::Array(calls));
                }

                serde_json::Value::Object(obj)
            })
            .collect()
    }

    fn build_chat_payload(args: BuildChatPayloadArgs<'_>) -> serde_json::Value {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "model".to_string(),
            serde_json::Value::String(args.model.to_string()),
        );
        payload.insert(
            "messages".to_string(),
            serde_json::Value::Array(Self::serialize_messages(args.messages.to_vec())),
        );
        payload.insert("stream".to_string(), serde_json::Value::Bool(args.stream));
        payload.insert("think".to_string(), serde_json::Value::Bool(args.think));
        if let Some(keep_alive) = args.keep_alive.filter(|v| !v.trim().is_empty()) {
            payload.insert(
                "keep_alive".to_string(),
                serde_json::Value::String(keep_alive.to_string()),
            );
        }
        payload.insert(
            "options".to_string(),
            serde_json::to_value(args.options).unwrap_or_else(|_| serde_json::json!({})),
        );
        if !args.tools.is_empty() {
            payload.insert(
                "tools".to_string(),
                serde_json::Value::Array(
                    args.tools
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "type": "function",
                                "function": {
                                    "name": t.name,
                                    "description": t.description,
                                    "parameters": t.parameters,
                                }
                            })
                        })
                        .collect::<Vec<_>>(),
                ),
            );
        }
        if args.force_json_format {
            payload.insert(
                "format".to_string(),
                serde_json::Value::String("json".to_string()),
            );
        }
        serde_json::Value::Object(payload)
    }

    fn is_tools_unsupported_error(status: StatusCode, body: &str) -> bool {
        if status != StatusCode::BAD_REQUEST {
            return false;
        }
        let normalized = body.to_ascii_lowercase();
        normalized.contains("does not support tools")
            || normalized.contains("doesn't support tools")
            || normalized.contains("tool calling is not supported")
            || normalized.contains("tool calls are not supported")
    }

    fn push_events_from_chunk(
        chunk: OllamaChatChunk,
        queue: &mut VecDeque<Result<ProviderEvent>>,
    ) -> Result<bool> {
        if let Some(err) = chunk.error {
            bail!("ollama stream error: {err}");
        }

        if let Some(msg) = chunk.message {
            if let Some(thinking) = msg.thinking
                && !thinking.is_empty()
            {
                queue.push_back(Ok(ProviderEvent::ThinkingChunk { content: thinking }));
            }
            if let Some(content) = msg.content
                && !content.is_empty()
            {
                queue.push_back(Ok(ProviderEvent::TextChunk { content }));
            }
            if let Some(tool_calls) = msg.tool_calls {
                for tc in tool_calls {
                    queue.push_back(Ok(ProviderEvent::ToolCall {
                        call: ToolCall {
                            name: tc.function.name,
                            arguments: tc.function.arguments,
                        },
                    }));
                }
            }
        }

        let done = chunk.done.unwrap_or(false);
        if done && let Some(completion_tokens) = chunk.eval_count {
            queue.push_back(Ok(ProviderEvent::Usage {
                usage: ProviderUsage {
                    prompt_tokens: chunk.prompt_eval_count.unwrap_or(0),
                    completion_tokens,
                },
            }));
        }

        Ok(done)
    }

    fn parse_ndjson_line(line: &str, queue: &mut VecDeque<Result<ProviderEvent>>) -> Result<bool> {
        if line.trim().is_empty() {
            return Ok(false);
        }
        let chunk: OllamaChatChunk =
            serde_json::from_str(line).context("invalid ollama NDJSON chunk")?;
        Self::push_events_from_chunk(chunk, queue)
    }

    pub fn is_private_ip(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.octets() == Ipv4Addr::new(169, 254, 169, 254).octets()
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unique_local()
                    || v6.is_unicast_link_local()
                    || v6 == Ipv6Addr::from_bits(0xfe80_0000_0000_0000_0000_0000_0000_0000)
            }
        }
    }

    async fn probe_json_mode(client: &Client, chat_url: &str, model: &str) -> bool {
        let payload = serde_json::json!({
            "model": model,
            "stream": false,
            "format": "json",
            "messages": [
                {
                    "role": "user",
                    "content": "Return a compact JSON object with key ok and value true."
                }
            ]
        });
        let resp = match client.post(chat_url).json(&payload).send().await {
            Ok(v) => v,
            Err(_) => return false,
        };
        if !resp.status().is_success() {
            return false;
        }
        let value: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return false,
        };
        let content = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if content.trim().is_empty() {
            return false;
        }
        serde_json::from_str::<serde_json::Value>(content.trim()).is_ok()
    }
}

#[async_trait]
impl InferenceProvider for OllamaProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    async fn load_or_connect(&mut self) -> Result<()> {
        let client = self.build_client()?;
        let mut guard = self.client.lock().await;
        *guard = Some(client);
        Ok(())
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ProviderStream> {
        let task_id = request.task_id.clone();
        let model = request.model;
        let messages = request.messages;
        let tools = request.tools;
        let stream = request.stream;
        let think = request.think;
        let options = request.options;

        let cancel_flag = if stream {
            if let Some(id) = task_id.clone() {
                let flag = Arc::new(AtomicBool::new(false));
                self.cancel_flags.lock().await.insert(id, flag.clone());
                Some(flag)
            } else {
                None
            }
        } else {
            None
        };
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| anyhow!("provider is not connected"))?
            .clone();
        drop(guard);

        let primary_payload = Self::build_chat_payload(BuildChatPayloadArgs {
            model: &model,
            messages: &messages,
            tools: &tools,
            stream,
            think,
            options: &options,
            keep_alive: self.keep_alive.as_deref(),
            force_json_format: false,
        });
        let mut response = client
            .post(self.chat_url())
            .json(&primary_payload)
            .send()
            .await
            .context("ollama request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if !tools.is_empty() && Self::is_tools_unsupported_error(status, &body) {
                let fallback_payload = Self::build_chat_payload(BuildChatPayloadArgs {
                    model: &model,
                    messages: &messages,
                    tools: &[],
                    stream,
                    think,
                    options: &options,
                    keep_alive: self.keep_alive.as_deref(),
                    force_json_format: true,
                });
                response = client
                    .post(self.chat_url())
                    .json(&fallback_payload)
                    .send()
                    .await
                    .context("ollama fallback request failed")?;
                if !response.status().is_success() {
                    let fallback_status = response.status();
                    let fallback_body = response.text().await.unwrap_or_default();
                    bail!(
                        "ollama fallback request failed: status {fallback_status}: {fallback_body}"
                    );
                }
            } else {
                bail!("ollama request failed: status {status}: {body}");
            }
        }

        if !stream {
            let chunk: OllamaChatChunk =
                response.json().await.context("invalid ollama response")?;
            let mut events = VecDeque::new();
            let _ = Self::push_events_from_chunk(chunk, &mut events)?;
            events.push_back(Ok(ProviderEvent::Done));
            if let Some(id) = task_id {
                self.cancel_flags.lock().await.remove(&id);
            }
            return Ok(Box::pin(stream::iter(events)));
        }

        let mut bytes_stream = response.bytes_stream();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ProviderEvent>>(256);
        let cancel_flag_stream = cancel_flag.clone();
        let cancel_map = self.cancel_flags.clone();
        let task_id_stream = task_id.clone();

        tokio::spawn(async move {
            let mut buf = String::new();
            let mut queue = VecDeque::new();
            let mut done = false;

            while let Some(next) = bytes_stream.next().await {
                if cancel_flag_stream
                    .as_ref()
                    .map(|f| f.load(Ordering::Relaxed))
                    .unwrap_or(false)
                {
                    let _ = tx.send(Err(anyhow!("ollama request cancelled"))).await;
                    if let Some(id) = &task_id_stream {
                        cancel_map.lock().await.remove(id);
                    }
                    return;
                }
                match next {
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(nl) = buf.find('\n') {
                            let line = buf[..nl].trim().to_string();
                            buf.drain(..=nl);
                            match Self::parse_ndjson_line(&line, &mut queue) {
                                Ok(is_done) => {
                                    while let Some(evt) = queue.pop_front() {
                                        if tx.send(evt).await.is_err() {
                                            if let Some(id) = &task_id_stream {
                                                cancel_map.lock().await.remove(id);
                                            }
                                            return;
                                        }
                                    }
                                    if is_done {
                                        done = true;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send(Err(e)).await;
                                    if let Some(id) = &task_id_stream {
                                        cancel_map.lock().await.remove(id);
                                    }
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(anyhow!("ollama stream read error: {e}"))).await;
                        if let Some(id) = &task_id_stream {
                            cancel_map.lock().await.remove(id);
                        }
                        return;
                    }
                }
            }

            if !buf.trim().is_empty() {
                match Self::parse_ndjson_line(buf.trim(), &mut queue) {
                    Ok(is_done) => {
                        while let Some(evt) = queue.pop_front() {
                            if tx.send(evt).await.is_err() {
                                if let Some(id) = &task_id_stream {
                                    cancel_map.lock().await.remove(id);
                                }
                                return;
                            }
                        }
                        if is_done {
                            done = true;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        if let Some(id) = &task_id_stream {
                            cancel_map.lock().await.remove(id);
                        }
                        return;
                    }
                }
            }

            if !done {
                let _ = tx
                    .send(Err(anyhow!("ollama stream ended before done=true")))
                    .await;
                if let Some(id) = &task_id_stream {
                    cancel_map.lock().await.remove(id);
                }
                return;
            }

            let _ = tx.send(Ok(ProviderEvent::Done)).await;
            if let Some(id) = &task_id_stream {
                cancel_map.lock().await.remove(id);
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn cancel(&self, task_id: &str) -> Result<()> {
        if let Some(flag) = self.cancel_flags.lock().await.get(task_id).cloned() {
            flag.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn health(&self) -> Result<ProviderHealth> {
        let start = std::time::Instant::now();
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| anyhow!("provider is not connected"))?
            .clone();
        drop(guard);

        let res = client.get(self.tags_url()).send().await;
        match res {
            Ok(r) if r.status().is_success() => Ok(ProviderHealth {
                healthy: true,
                latency_ms: start.elapsed().as_millis() as u64,
                details: "ok".to_string(),
            }),
            Ok(r) => Ok(ProviderHealth {
                healthy: false,
                latency_ms: start.elapsed().as_millis() as u64,
                details: format!("status {}", r.status()),
            }),
            Err(e) => Ok(ProviderHealth {
                healthy: false,
                latency_ms: start.elapsed().as_millis() as u64,
                details: e.to_string(),
            }),
        }
    }

    async fn capabilities(&self) -> Result<ProviderCapabilities> {
        let default_caps = ProviderCapabilities {
            context_window: 8192,
            supports_streaming: true,
            supports_native_tool_calls: false,
            supports_json_mode: true,
            structured_mode: StructuredOutputMode::StrictJsonFallback,
            model_family: "ollama".to_string(),
        };

        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| anyhow!("provider is not connected"))?
            .clone();
        drop(guard);

        let resp = client
            .post(self.show_url())
            .json(&serde_json::json!({ "model": self.model }))
            .send()
            .await;

        let Ok(resp) = resp else {
            return Ok(default_caps);
        };
        if !resp.status().is_success() {
            return Ok(default_caps);
        }

        let show: OllamaShowResponse = match resp.json().await {
            Ok(v) => v,
            Err(_) => return Ok(default_caps),
        };

        let context_window = show
            .model_info
            .get("llama.context_length")
            .and_then(|v| v.as_u64())
            .or_else(|| {
                show.model_info
                    .get("general.context_length")
                    .and_then(|v| v.as_u64())
            })
            .unwrap_or(default_caps.context_window as u64) as u32;
        let supports_tools = show.capabilities.iter().any(|c| c == "tools");
        let supports_json_mode =
            Self::probe_json_mode(&client, &self.chat_url(), &self.model).await;
        let model_family = show
            .details
            .as_ref()
            .and_then(|d| d.family.clone())
            .or_else(|| {
                show.details
                    .as_ref()
                    .and_then(|d| d.families.as_ref().and_then(|f| f.first().cloned()))
            })
            .unwrap_or_else(|| default_caps.model_family.clone());

        Ok(ProviderCapabilities {
            context_window,
            supports_streaming: true,
            supports_native_tool_calls: supports_tools,
            supports_json_mode,
            structured_mode: if supports_tools {
                StructuredOutputMode::NativeToolCalling
            } else {
                StructuredOutputMode::StrictJsonFallback
            },
            model_family,
        })
    }

    async fn shutdown(&self) -> Result<()> {
        self.cancel_flags.lock().await.clear();
        let mut guard = self.client.lock().await;
        *guard = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_endpoint_without_opt_in() {
        let err = OllamaProvider::new("http://10.0.0.2:11434", "m", false, false, 3, 10, "5m")
            .expect_err("should fail");
        assert!(err.to_string().contains("allow_remote=true"));
    }

    #[test]
    fn rejects_plain_http_remote_without_opt_in() {
        let err = OllamaProvider::new("http://example.com:11434", "m", true, false, 3, 10, "5m")
            .expect_err("should fail");
        assert!(err.to_string().contains("allow_plain_http_remote=true"));
    }

    #[test]
    fn allows_loopback_http() {
        OllamaProvider::new("http://127.0.0.1:11434", "m", false, false, 3, 10, "5m")
            .expect("loopback should be allowed");
    }

    #[test]
    fn parses_stream_tool_call_chunk() {
        let line = r#"{"message":{"tool_calls":[{"function":{"name":"execute_shell_command","arguments":{"cmd":"ls -la"}}}]},"done":false}"#;
        let mut queue = VecDeque::new();
        let done = OllamaProvider::parse_ndjson_line(line, &mut queue).expect("parse");
        assert!(!done);
        let evt = queue.pop_front().expect("event").expect("ok event");
        match evt {
            ProviderEvent::ToolCall { call } => {
                assert_eq!(call.name, "execute_shell_command");
                assert_eq!(call.arguments["cmd"], "ls -la");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn serialize_tool_role_message() {
        let msgs = vec![ChatMessage::tool(
            "lookup_command_docs",
            "{\"name\":\"git\"}",
        )];
        let encoded = OllamaProvider::serialize_messages(msgs);
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0]["role"], "tool");
        assert_eq!(encoded[0]["tool_name"], "lookup_command_docs");
    }

    #[tokio::test]
    async fn capabilities_fallback_reports_strict_json_when_probe_unavailable() {
        let mut provider = OllamaProvider::new("http://127.0.0.1:9", "m", false, false, 1, 1, "5m")
            .expect("construct provider");
        provider.load_or_connect().await.expect("connect");
        let caps = provider.capabilities().await.expect("capabilities");
        assert!(caps.supports_json_mode);
        assert!(!caps.supports_native_tool_calls);
        assert!(matches!(
            caps.structured_mode,
            StructuredOutputMode::StrictJsonFallback
        ));
    }

    #[test]
    fn tools_unsupported_error_detection_is_narrow_and_case_insensitive() {
        assert!(OllamaProvider::is_tools_unsupported_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":"model does not support tools"}"#
        ));
        assert!(OllamaProvider::is_tools_unsupported_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":"Tool Calling Is Not Supported"}"#
        ));
        assert!(!OllamaProvider::is_tools_unsupported_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":"invalid request"}"#
        ));
        assert!(!OllamaProvider::is_tools_unsupported_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"error":"does not support tools"}"#
        ));
    }

    #[test]
    fn fallback_payload_uses_json_format_and_omits_tools() {
        let payload = OllamaProvider::build_chat_payload(BuildChatPayloadArgs {
            model: "demo",
            messages: &[ChatMessage::user("hello")],
            tools: &[],
            stream: true,
            think: false,
            options: &BTreeMap::new(),
            keep_alive: Some("5m"),
            force_json_format: true,
        });
        assert_eq!(payload["format"], "json");
        assert_eq!(payload["keep_alive"], "5m");
        assert!(payload.get("tools").is_none());
    }
}
