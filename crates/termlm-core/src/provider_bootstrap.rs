use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use sha2::Digest;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use termlm_config::AppConfig;
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ModelAssetRecord {
    pub(crate) filename: String,
    pub(crate) kind: String,
    pub(crate) sha256: String,
    pub(crate) size_bytes: u64,
    pub(crate) source_url: Option<String>,
    pub(crate) last_verified_unix: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ModelAssetManifest {
    pub(crate) schema_version: u32,
    pub(crate) assets: BTreeMap<String, ModelAssetRecord>,
}

impl Default for ModelAssetManifest {
    fn default() -> Self {
        Self {
            schema_version: 1,
            assets: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DownloadFileMeta {
    pub(crate) sha256: String,
    pub(crate) size_bytes: u64,
}

pub(crate) struct EnsureModelAssetArgs<'a> {
    pub(crate) kind: &'a str,
    pub(crate) filename: &'a str,
    pub(crate) path: &'a Path,
    pub(crate) download_url: Option<&'a str>,
    pub(crate) allow_download: bool,
    pub(crate) required: bool,
    pub(crate) expected_sha256: Option<String>,
    pub(crate) timeout: std::time::Duration,
    pub(crate) user_agent: &'a str,
}

pub(crate) fn resolve_models_dir(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

pub(crate) async fn ensure_required_model_assets(cfg: &AppConfig) -> Result<()> {
    let models_dir = resolve_models_dir(&cfg.model.models_dir);
    std::fs::create_dir_all(&models_dir)
        .with_context(|| format!("create models dir {}", models_dir.display()))?;
    let manifest_path = model_asset_manifest_path(&models_dir);
    let mut manifest = load_model_asset_manifest(&manifest_path).unwrap_or_else(|e| {
        warn!(
            "failed to load model asset manifest at {}: {e:#}; rebuilding",
            manifest_path.display()
        );
        ModelAssetManifest::default()
    });
    let mut manifest_changed = false;

    if cfg.inference.provider == "local" {
        let (selected_variant, selected_filename) = if cfg.model.variant.eq_ignore_ascii_case("E2B")
        {
            ("E2B", cfg.model.e2b_filename.as_str())
        } else {
            ("E4B", cfg.model.e4b_filename.as_str())
        };
        let selected_path = models_dir.join(selected_filename);
        let url = resolve_model_download_url(selected_variant, selected_filename)?;
        manifest_changed |= ensure_model_asset(
            &mut manifest,
            EnsureModelAssetArgs {
                kind: "inference_model",
                filename: selected_filename,
                path: &selected_path,
                download_url: Some(url.as_str()),
                allow_download: cfg.model.auto_download,
                required: true,
                expected_sha256: expected_model_variant_sha256(selected_variant),
                timeout: std::time::Duration::from_secs(cfg.ollama.request_timeout_secs.max(300)),
                user_agent: "termlm/0.1 model-bootstrap",
            },
        )
        .await
        .with_context(|| {
            format!(
                "failed ensuring local model variant {selected_variant} at {}",
                selected_path.display()
            )
        })?;

        if !cfg.model.download_only_selected_variant {
            for (variant, filename) in [
                ("E4B", cfg.model.e4b_filename.as_str()),
                ("E2B", cfg.model.e2b_filename.as_str()),
            ] {
                if variant == selected_variant {
                    continue;
                }
                let path = models_dir.join(filename);
                let url = resolve_model_download_url(variant, filename)?;
                match ensure_model_asset(
                    &mut manifest,
                    EnsureModelAssetArgs {
                        kind: "inference_model",
                        filename,
                        path: &path,
                        download_url: Some(url.as_str()),
                        allow_download: cfg.model.auto_download,
                        required: false,
                        expected_sha256: expected_model_variant_sha256(variant),
                        timeout: std::time::Duration::from_secs(
                            cfg.ollama.request_timeout_secs.max(300),
                        ),
                        user_agent: "termlm/0.1 model-bootstrap",
                    },
                )
                .await
                {
                    Ok(changed) => {
                        manifest_changed |= changed;
                    }
                    Err(e) => {
                        warn!(
                            "optional local model variant {variant} setup issue at {}: {e:#}",
                            path.display()
                        );
                    }
                }
            }
        }
    }

    if cfg.indexer.embedding_provider == "local" {
        let embed_path = models_dir.join(&cfg.indexer.embed_filename);
        let embed_url = resolve_embedding_download_url(&cfg.indexer.embed_filename);
        match ensure_model_asset(
            &mut manifest,
            EnsureModelAssetArgs {
                kind: "embedding_model",
                filename: &cfg.indexer.embed_filename,
                path: &embed_path,
                download_url: embed_url.as_deref(),
                allow_download: cfg.model.auto_download,
                required: false,
                expected_sha256: expected_embedding_sha256(&cfg.indexer.embed_filename),
                timeout: std::time::Duration::from_secs(cfg.ollama.request_timeout_secs.max(300)),
                user_agent: "termlm/0.1 embed-bootstrap",
            },
        )
        .await
        {
            Ok(changed) => {
                manifest_changed |= changed;
            }
            Err(e) => {
                warn!(
                    "embedding model setup issue at {}: {e:#}",
                    embed_path.display()
                );
            }
        }
    }

    if manifest_changed {
        write_model_asset_manifest_atomic(&manifest_path, &manifest)?;
    }

    Ok(())
}

pub(crate) fn model_asset_manifest_path(models_dir: &Path) -> PathBuf {
    models_dir.join("assets.manifest.json")
}

pub(crate) fn load_model_asset_manifest(path: &Path) -> Result<ModelAssetManifest> {
    if !path.exists() {
        return Ok(ModelAssetManifest::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read model asset manifest {}", path.display()))?;
    let mut parsed: ModelAssetManifest = serde_json::from_str(&raw)
        .with_context(|| format!("parse model asset manifest {}", path.display()))?;
    if parsed.schema_version == 0 {
        parsed.schema_version = 1;
    }
    Ok(parsed)
}

pub(crate) fn write_model_asset_manifest_atomic(
    path: &Path,
    manifest: &ModelAssetManifest,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let payload = serde_json::to_vec_pretty(manifest).context("serialize model asset manifest")?;
    std::fs::write(&tmp, payload).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

pub(crate) fn expected_model_variant_sha256(variant: &str) -> Option<String> {
    let key = format!("TERMLM_MODEL_{}_SHA256", variant.to_ascii_uppercase());
    std::env::var(key)
        .ok()
        .and_then(|value| normalize_sha256(&value))
}

pub(crate) const DEFAULT_EMBED_FILENAME_BGE_Q4KM: &str = "bge-small-en-v1.5.Q4_K_M.gguf";
pub(crate) const DEFAULT_EMBED_URL_BGE_Q4KM: &str = "https://huggingface.co/ChristianAzinn/bge-small-en-v1.5-gguf/resolve/main/bge-small-en-v1.5.Q4_K_M.gguf";
pub(crate) const DEFAULT_EMBED_SHA256_BGE_Q4KM: &str =
    "d8c2e0e38bce043562bbc6f437c638c2538bfe02cadfe6476a01f906bfde6d40";

pub(crate) fn expected_embedding_sha256(filename: &str) -> Option<String> {
    if let Ok(value) = std::env::var("TERMLM_EMBED_MODEL_SHA256")
        && let Some(parsed) = normalize_sha256(&value)
    {
        return Some(parsed);
    }
    let key = format!(
        "TERMLM_EMBED_MODEL_{}_SHA256",
        filename
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    );
    if let Some(parsed) = std::env::var(key)
        .ok()
        .and_then(|value| normalize_sha256(&value))
    {
        return Some(parsed);
    }
    match filename {
        DEFAULT_EMBED_FILENAME_BGE_Q4KM => Some(DEFAULT_EMBED_SHA256_BGE_Q4KM.to_string()),
        _ => None,
    }
}

pub(crate) fn normalize_sha256(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() == 64 && normalized.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(normalized)
    } else {
        None
    }
}

pub(crate) fn model_asset_key(kind: &str, filename: &str) -> String {
    format!("{kind}:{filename}")
}

pub(crate) fn record_model_asset(
    manifest: &mut ModelAssetManifest,
    kind: &str,
    filename: &str,
    source_url: Option<&str>,
    sha256: &str,
    size_bytes: u64,
) -> bool {
    let key = model_asset_key(kind, filename);
    let next = ModelAssetRecord {
        filename: filename.to_string(),
        kind: kind.to_string(),
        sha256: sha256.to_string(),
        size_bytes,
        source_url: source_url.map(ToString::to_string),
        last_verified_unix: chrono::Utc::now().timestamp(),
    };
    if let Some(existing) = manifest.assets.get(&key)
        && existing.filename == next.filename
        && existing.kind == next.kind
        && existing.sha256 == next.sha256
        && existing.size_bytes == next.size_bytes
        && existing.source_url == next.source_url
    {
        return false;
    }
    manifest.assets.insert(key, next);
    true
}

pub(crate) fn compute_file_sha256(path: &Path) -> Result<(String, u64)> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    let mut total = 0u64;
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        total = total.saturating_add(read as u64);
    }
    if total == 0 {
        bail!("model asset is empty: {}", path.display());
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

fn should_force_model_hash() -> bool {
    std::env::var("TERMLM_VERIFY_MODEL_SHA")
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "always")
        })
        .unwrap_or(false)
}

pub(crate) fn can_trust_cached_model_asset_record(
    manifest: &ModelAssetManifest,
    kind: &str,
    filename: &str,
    path: &Path,
    expected_sha256: Option<&str>,
) -> Result<bool> {
    if should_force_model_hash() {
        return Ok(false);
    }
    let Some(previous) = manifest.assets.get(&model_asset_key(kind, filename)) else {
        return Ok(false);
    };
    let metadata =
        std::fs::metadata(path).with_context(|| format!("read metadata for {}", path.display()))?;
    let size_bytes = metadata.len();
    if size_bytes == 0 {
        bail!("model asset is empty: {}", path.display());
    }
    if previous.size_bytes != size_bytes {
        return Ok(false);
    }
    if let Some(expected) = expected_sha256 {
        return Ok(previous.sha256 == expected);
    }
    Ok(normalize_sha256(&previous.sha256).is_some())
}

pub(crate) async fn ensure_model_asset(
    manifest: &mut ModelAssetManifest,
    args: EnsureModelAssetArgs<'_>,
) -> Result<bool> {
    let EnsureModelAssetArgs {
        kind,
        filename,
        path,
        download_url,
        allow_download,
        required,
        expected_sha256,
        timeout,
        user_agent,
    } = args;
    let expected = expected_sha256.as_deref();
    if !path.exists() {
        if !allow_download {
            if required {
                bail!(
                    "required model asset is missing: {} (enable [model].auto_download=true)",
                    path.display()
                );
            }
            bail!(
                "optional model asset missing at {} and auto_download=false",
                path.display()
            );
        }
        let url = download_url.ok_or_else(|| {
            anyhow!(
                "model asset missing at {} and no download URL configured",
                path.display()
            )
        })?;
        info!(
            kind = kind,
            file = filename,
            "model asset missing; downloading"
        );
        let meta = download_file_to_path(url, path, timeout, user_agent).await?;
        if let Some(expected_sha) = expected
            && meta.sha256 != expected_sha
        {
            let _ = tokio::fs::remove_file(path).await;
            bail!(
                "checksum mismatch for downloaded {}: expected {} got {}",
                path.display(),
                expected_sha,
                meta.sha256
            );
        }
        info!(
            kind = kind,
            file = filename,
            "model asset download complete"
        );
        return Ok(record_model_asset(
            manifest,
            kind,
            filename,
            Some(url),
            &meta.sha256,
            meta.size_bytes,
        ));
    }

    if can_trust_cached_model_asset_record(manifest, kind, filename, path, expected)? {
        return Ok(false);
    }

    let (actual_sha, size_bytes) = compute_file_sha256(path)?;
    if let Some(expected_sha) = expected
        && actual_sha != expected_sha
    {
        if allow_download {
            if let Some(url) = download_url {
                warn!(
                    "checksum mismatch for {}; redownloading from {}",
                    path.display(),
                    url
                );
                let meta = download_file_to_path(url, path, timeout, user_agent).await?;
                if meta.sha256 != expected_sha {
                    let _ = tokio::fs::remove_file(path).await;
                    bail!(
                        "checksum mismatch after redownload {}: expected {} got {}",
                        path.display(),
                        expected_sha,
                        meta.sha256
                    );
                }
                return Ok(record_model_asset(
                    manifest,
                    kind,
                    filename,
                    Some(url),
                    &meta.sha256,
                    meta.size_bytes,
                ));
            }
            if required {
                bail!(
                    "checksum mismatch for {} and no download URL configured",
                    path.display()
                );
            }
        } else if required {
            bail!(
                "checksum mismatch for {} and auto_download=false",
                path.display()
            );
        }
    }

    let key = model_asset_key(kind, filename);
    if let Some(previous) = manifest.assets.get(&key)
        && previous.sha256 != actual_sha
    {
        warn!(
            "model asset checksum changed for {}: {} -> {}",
            path.display(),
            previous.sha256,
            actual_sha
        );
    }
    Ok(record_model_asset(
        manifest,
        kind,
        filename,
        download_url,
        &actual_sha,
        size_bytes,
    ))
}

pub(crate) fn resolve_embedding_download_url(filename: &str) -> Option<String> {
    if let Ok(url) = std::env::var("TERMLM_EMBED_MODEL_URL")
        && !url.trim().is_empty()
    {
        return Some(url);
    }
    if let Ok(base) = std::env::var("TERMLM_EMBED_MODEL_BASE_URL")
        && !base.trim().is_empty()
    {
        return Some(format!("{}/{}", base.trim_end_matches('/'), filename));
    }
    match filename {
        DEFAULT_EMBED_FILENAME_BGE_Q4KM => Some(DEFAULT_EMBED_URL_BGE_Q4KM.to_string()),
        _ => None,
    }
}

pub(crate) fn resolve_model_download_url(variant: &str, filename: &str) -> Result<String> {
    let env_key = format!("TERMLM_MODEL_{}_URL", variant.to_ascii_uppercase());
    if let Ok(url) = std::env::var(&env_key)
        && !url.trim().is_empty()
    {
        return Ok(url);
    }
    if let Ok(base) = std::env::var("TERMLM_MODEL_BASE_URL")
        && !base.trim().is_empty()
    {
        return Ok(format!("{}/{}", base.trim_end_matches('/'), filename));
    }

    let url = match variant.to_ascii_uppercase().as_str() {
        "E4B" => {
            format!("https://huggingface.co/ggml-org/gemma-4-E4B-it-GGUF/resolve/main/{filename}")
        }
        "E2B" => {
            format!("https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/{filename}")
        }
        other => bail!("unsupported model variant for download: {other}"),
    };
    Ok(url)
}

pub(crate) async fn download_file_to_path(
    url: &str,
    destination: &Path,
    timeout: std::time::Duration,
    user_agent: &str,
) -> Result<DownloadFileMeta> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("destination has no parent: {}", destination.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .context("build download client")?;
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, user_agent)
        .send()
        .await
        .with_context(|| format!("request {url}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("download failed with status {status}: {body}");
    }

    let tmp_path = destination.with_extension("partial");
    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .with_context(|| format!("create {}", tmp_path.display()))?;
    let mut stream = response.bytes_stream();
    let mut total_bytes = 0u64;
    let mut hasher = sha2::Sha256::new();
    while let Some(next) = stream.next().await {
        let chunk = next.with_context(|| format!("stream {url}"))?;
        file.write_all(&chunk)
            .await
            .with_context(|| format!("write {}", tmp_path.display()))?;
        total_bytes = total_bytes.saturating_add(chunk.len() as u64);
        hasher.update(&chunk);
    }
    file.flush()
        .await
        .with_context(|| format!("flush {}", tmp_path.display()))?;
    file.sync_all()
        .await
        .with_context(|| format!("sync {}", tmp_path.display()))?;
    drop(file);

    if total_bytes == 0 {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        bail!("download produced empty file: {url}");
    }

    tokio::fs::rename(&tmp_path, destination)
        .await
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), destination.display()))?;
    Ok(DownloadFileMeta {
        sha256: format!("{:x}", hasher.finalize()),
        size_bytes: total_bytes,
    })
}

pub(crate) async fn validate_provider_boot(cfg: &AppConfig) -> Result<()> {
    match cfg.inference.provider.as_str() {
        "local" => Ok(()),
        "ollama" => {
            termlm_inference::OllamaProvider::validate_endpoint(
                &cfg.ollama.endpoint,
                cfg.ollama.allow_remote,
                cfg.ollama.allow_plain_http_remote,
            )?;

            if cfg.ollama.healthcheck_on_start
                && !check_ollama_health(
                    &cfg.ollama.endpoint,
                    &cfg.ollama.model,
                    cfg.inference.tool_calling_required,
                    cfg.ollama.connect_timeout_secs,
                    cfg.ollama.request_timeout_secs,
                )
                .await
                .unwrap_or(false)
            {
                bail!("inference provider unavailable: ollama healthcheck failed");
            }
            Ok(())
        }
        other => bail!("unsupported inference provider: {other}"),
    }
}

async fn check_ollama_health(
    endpoint: &str,
    model: &str,
    tool_calling_required: bool,
    connect_timeout: u64,
    request_timeout: u64,
) -> Result<bool> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(connect_timeout))
        .timeout(std::time::Duration::from_secs(request_timeout))
        .build()?;

    let tags_url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    let response = client.get(tags_url).send().await?;
    if !response.status().is_success() {
        return Ok(false);
    }
    let tags_value = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let model_available = tags_value
        .get("models")
        .and_then(|v| v.as_array())
        .map(|models| {
            models.iter().any(|row| {
                let name = row.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                ollama_model_matches(name, model)
            })
        })
        .unwrap_or(false);
    if !model_available {
        return Ok(false);
    }

    let show_url = format!("{}/api/show", endpoint.trim_end_matches('/'));
    let show = client
        .post(show_url)
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await?;
    if !show.status().is_success() {
        return Ok(false);
    }
    let show_value = show
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let supports_tools = show_value
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|caps| {
            caps.iter()
                .filter_map(|v| v.as_str())
                .any(|cap| cap.eq_ignore_ascii_case("tools"))
        })
        .unwrap_or(false);
    let supports_json_mode = probe_ollama_json_mode(&client, endpoint, model)
        .await
        .unwrap_or(false);
    if tool_calling_required && !(supports_tools || supports_json_mode) {
        return Ok(false);
    }

    Ok(true)
}

async fn probe_ollama_json_mode(
    client: &reqwest::Client,
    endpoint: &str,
    model: &str,
) -> Result<bool> {
    let chat_url = format!("{}/api/chat", endpoint.trim_end_matches('/'));
    let payload = serde_json::json!({
        "model": model,
        "stream": false,
        "format": "json",
        "messages": [
            {
                "role": "user",
                "content": "Return JSON with key ok and value true."
            }
        ]
    });
    let resp = client.post(chat_url).json(&payload).send().await?;
    if !resp.status().is_success() {
        return Ok(false);
    }
    let value = resp
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let content = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if content.trim().is_empty() {
        return Ok(false);
    }
    Ok(serde_json::from_str::<serde_json::Value>(content.trim()).is_ok())
}

pub(crate) fn ollama_model_matches(candidate: &str, requested: &str) -> bool {
    if candidate.eq_ignore_ascii_case(requested) {
        return true;
    }
    let normalized_candidate = candidate.trim().to_ascii_lowercase();
    let normalized_requested = requested.trim().to_ascii_lowercase();
    if normalized_candidate == normalized_requested {
        return true;
    }
    normalized_candidate
        .trim_end_matches(":latest")
        .eq_ignore_ascii_case(normalized_requested.trim_end_matches(":latest"))
}

pub(crate) fn is_loopback_endpoint(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}
