use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use sha2::Digest;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use termlm_protocol::{ClientMessage, MAX_FRAME_BYTES, ServerMessage};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio_serde::formats::Json;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

const DEFAULT_GITHUB_REPO: &str = "thtmnisamnstr/termlm";
const GITHUB_API_BASE: &str = "https://api.github.com/repos";
const RELEASE_MANIFEST_NAME: &str = "bundle-manifest.json";

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct BundleManifest {
    schema_version: u32,
    version: String,
    target: String,
    artifact_kind: String,
    includes_models: bool,
}

#[derive(Debug)]
struct InstallLayout {
    bin_dir: PathBuf,
    share_dir: PathBuf,
    plugins_zsh_dir: PathBuf,
}

#[derive(Debug, serde::Serialize)]
struct InstallReceipt {
    schema_version: u32,
    installed_at: String,
    release_tag: String,
    bundle_version: String,
    repository: String,
    platform: String,
    asset_name: String,
    artifact_kind: String,
    includes_models: bool,
}

pub async fn run_upgrade(
    repo_override: Option<String>,
    tag_override: Option<String>,
) -> Result<()> {
    let repo = resolve_repo(repo_override);
    let platform = platform_id().ok_or_else(|| {
        anyhow!(
            "unsupported platform for upgrade: os={} arch={}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let token = github_token();
    let client = reqwest::Client::builder()
        .user_agent(format!("termlm/{}/upgrade", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()
        .context("build upgrade HTTP client")?;

    let release = fetch_release(&client, &repo, tag_override.as_deref(), token.as_deref()).await?;
    let asset = select_upgrade_asset(&release.assets, platform).ok_or_else(|| {
        anyhow!(
            "release {} has no no-models bundle for platform {}",
            release.tag_name,
            platform
        )
    })?;
    let checksums_asset = select_checksums_asset(&release.assets);

    println!(
        "termlm: downloading {} from {}",
        asset.name, release.tag_name
    );
    let temp_dir = tempfile::Builder::new()
        .prefix("termlm-upgrade-")
        .tempdir()
        .context("create upgrade temp directory")?;
    let archive_path = temp_dir.path().join(&asset.name);
    download_to_path(
        &client,
        token.as_deref(),
        &asset.browser_download_url,
        &archive_path,
    )
    .await
    .with_context(|| format!("download {}", asset.browser_download_url))?;

    if let Some(sum_asset) = checksums_asset {
        let sums_path = temp_dir.path().join(&sum_asset.name);
        download_to_path(
            &client,
            token.as_deref(),
            &sum_asset.browser_download_url,
            &sums_path,
        )
        .await
        .with_context(|| format!("download {}", sum_asset.browser_download_url))?;
        verify_checksum_from_sums(&archive_path, &sums_path, &asset.name)
            .context("release checksum validation failed")?;
    } else if allow_missing_checksums() {
        println!(
            "termlm: warning: checksum manifest missing; proceeding due TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS=1"
        );
    } else {
        bail!("release is missing SHA256SUMS; refusing upgrade");
    }

    let extract_dir = temp_dir.path().join("extract");
    fs::create_dir_all(&extract_dir)
        .with_context(|| format!("create {}", extract_dir.display()))?;
    extract_release_bundle(&archive_path, &extract_dir)?;
    let payload_root = locate_payload_root(&extract_dir)?;
    let manifest = load_bundle_manifest(&payload_root)?;
    enforce_no_models_bundle(&payload_root, manifest.as_ref())?;

    let layout = InstallLayout::from_env()?;

    shutdown_daemon_best_effort().await;
    install_payload(&payload_root, &layout)?;

    let manifest = manifest.unwrap_or(BundleManifest {
        schema_version: 1,
        version: release.tag_name.trim_start_matches('v').to_string(),
        target: platform.to_string(),
        artifact_kind: "no-models".to_string(),
        includes_models: false,
    });

    write_install_receipt(
        &layout,
        &InstallReceipt {
            schema_version: 1,
            installed_at: Utc::now().to_rfc3339(),
            release_tag: release.tag_name.clone(),
            bundle_version: manifest.version.clone(),
            repository: repo.clone(),
            platform: manifest.target.clone(),
            asset_name: asset.name.clone(),
            artifact_kind: manifest.artifact_kind.clone(),
            includes_models: manifest.includes_models,
        },
    )?;

    println!(
        "termlm: upgraded to {} (asset: {}, models preserved)",
        release.tag_name, asset.name
    );
    println!(
        "termlm: installed binaries in {} and plugin in {}",
        layout.bin_dir.display(),
        layout.plugins_zsh_dir.display()
    );

    // Explicitly drop tempdir now so all downloaded and extraction artifacts are removed
    // before command exit.
    drop(temp_dir);

    Ok(())
}

impl InstallLayout {
    fn from_env() -> Result<Self> {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .context("HOME is not set; cannot resolve install paths")?;

        let bin_dir = std::env::var("TERMLM_INSTALL_BIN_DIR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|v| expand_tilde(&v, &home))
            .unwrap_or_else(|| home.join(".local/bin"));

        let share_dir = std::env::var("TERMLM_INSTALL_SHARE_DIR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|v| expand_tilde(&v, &home))
            .unwrap_or_else(|| home.join(".local/share/termlm"));

        let plugins_zsh_dir = share_dir.join("plugins").join("zsh");
        Ok(Self {
            bin_dir,
            share_dir,
            plugins_zsh_dir,
        })
    }
}

fn resolve_repo(repo_override: Option<String>) -> String {
    repo_override
        .or_else(|| std::env::var("TERMLM_GITHUB_REPO").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GITHUB_REPO.to_string())
}

fn github_token() -> Option<String> {
    std::env::var("TERMLM_GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .filter(|s| !s.trim().is_empty())
}

async fn fetch_release(
    client: &reqwest::Client,
    repo: &str,
    tag: Option<&str>,
    token: Option<&str>,
) -> Result<GitHubRelease> {
    let url = if let Some(tag) = tag {
        format!("{}/{repo}/releases/tags/{tag}", GITHUB_API_BASE)
    } else {
        format!("{}/{repo}/releases/latest", GITHUB_API_BASE)
    };

    let mut req = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.with_context(|| format!("request {url}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .with_context(|| format!("read response body from {url}"))?;
    if !status.is_success() {
        bail!("GitHub release request failed ({status}): {body}");
    }
    serde_json::from_str(&body).context("parse GitHub release payload")
}

fn platform_id() -> Option<&'static str> {
    platform_id_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn platform_id_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("darwin-arm64"),
        _ => None,
    }
}

fn allow_missing_checksums() -> bool {
    std::env::var("TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS")
        .map(|v| {
            let lowered = v.trim().to_ascii_lowercase();
            lowered == "1" || lowered == "true" || lowered == "yes"
        })
        .unwrap_or(false)
}

fn select_upgrade_asset<'a>(
    assets: &'a [GitHubReleaseAsset],
    platform: &str,
) -> Option<&'a GitHubReleaseAsset> {
    let mut matches = assets
        .iter()
        .filter(|asset| {
            let lower = asset.name.to_ascii_lowercase();
            lower.contains(&platform.to_ascii_lowercase())
                && lower.contains("no-model")
                && (lower.ends_with(".tar.gz") || lower.ends_with(".tgz"))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|asset| asset.name.len());
    matches.pop()
}

fn select_checksums_asset(assets: &[GitHubReleaseAsset]) -> Option<&GitHubReleaseAsset> {
    assets.iter().find(|asset| {
        let lower = asset.name.to_ascii_lowercase();
        lower == "sha256sums"
            || lower == "sha256sums.txt"
            || lower.ends_with("/sha256sums")
            || lower.ends_with("/sha256sums.txt")
    })
}

async fn download_to_path(
    client: &reqwest::Client,
    token: Option<&str>,
    url: &str,
    path: &Path,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }

    let mut req = client.get(url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.with_context(|| format!("request {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("download failed ({status}) for {url}: {body}");
    }

    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("create {}", path.display()))?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("stream {url}"))?;
        file.write_all(&chunk)
            .await
            .with_context(|| format!("write {}", path.display()))?;
    }
    file.flush()
        .await
        .with_context(|| format!("flush {}", path.display()))?;
    Ok(())
}

fn verify_checksum_from_sums(
    archive_path: &Path,
    sums_path: &Path,
    asset_name: &str,
) -> Result<()> {
    let checksums = parse_sha256_sums(sums_path)?;
    let expected = checksums
        .get(asset_name)
        .cloned()
        .or_else(|| {
            Path::new(asset_name)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .and_then(|name| checksums.get(&name).cloned())
        })
        .ok_or_else(|| anyhow!("checksum entry missing for {}", asset_name))?;
    let actual = compute_sha256(archive_path)?;
    if actual != expected {
        bail!(
            "checksum mismatch for {}: expected {} got {}",
            archive_path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

fn parse_sha256_sums(path: &Path) -> Result<BTreeMap<String, String>> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut out = BTreeMap::new();
    for line in reader.lines() {
        let line = line.with_context(|| format!("read line from {}", path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(sha) = parts.next() else {
            continue;
        };
        let Some(raw_name) = parts.next() else {
            continue;
        };
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let name = raw_name.trim_start_matches('*').to_string();
        out.insert(name, sha.to_ascii_lowercase());
    }
    Ok(out)
}

fn compute_sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_release_bundle(archive_path: &Path, destination: &Path) -> Result<()> {
    let file =
        fs::File::open(archive_path).with_context(|| format!("open {}", archive_path.display()))?;
    let name = archive_path.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let reader = flate2::read::GzDecoder::new(file);
        unpack_tar(reader, destination)?;
    } else if name.ends_with(".tar") {
        unpack_tar(file, destination)?;
    } else {
        bail!(
            "unsupported release archive format: {}",
            archive_path.display()
        );
    }
    Ok(())
}

fn unpack_tar<R: Read>(reader: R, destination: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().context("read tar entries")? {
        let mut entry = entry.context("read tar entry")?;
        let rel_path = entry.path().context("read tar path")?.into_owned();
        let out_path = safe_join(destination, &rel_path)?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            bail!("release bundles may not contain symlink/hardlink entries");
        }
        if entry_type.is_dir() {
            fs::create_dir_all(&out_path)
                .with_context(|| format!("create {}", out_path.display()))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        entry
            .unpack(&out_path)
            .with_context(|| format!("unpack {}", out_path.display()))?;
    }
    Ok(())
}

fn safe_join(base: &Path, rel: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::from(base);
    for component in rel.components() {
        match component {
            Component::Normal(seg) => out.push(seg),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("unsafe path in archive: {}", rel.display());
            }
        }
    }
    Ok(out)
}

fn locate_payload_root(extract_dir: &Path) -> Result<PathBuf> {
    if is_payload_root(extract_dir) {
        return Ok(extract_dir.to_path_buf());
    }
    for entry in
        fs::read_dir(extract_dir).with_context(|| format!("read {}", extract_dir.display()))?
    {
        let entry = entry.with_context(|| format!("read {}", extract_dir.display()))?;
        let path = entry.path();
        if entry
            .file_type()
            .with_context(|| format!("metadata {}", path.display()))?
            .is_dir()
            && is_payload_root(&path)
        {
            return Ok(path);
        }
    }
    bail!("release payload missing expected structure (bin/termlm + bin/termlm-core + plugins/zsh)")
}

fn is_payload_root(path: &Path) -> bool {
    let core = path.join("bin").join("termlm-core").exists();
    let cli =
        path.join("bin").join("termlm").exists() || path.join("bin").join("termlm-client").exists();
    let plugin = path
        .join("plugins")
        .join("zsh")
        .join("termlm.plugin.zsh")
        .exists();
    core && cli && plugin
}

fn load_bundle_manifest(payload_root: &Path) -> Result<Option<BundleManifest>> {
    let manifest_path = payload_root.join(RELEASE_MANIFEST_NAME);
    if !manifest_path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let parsed: BundleManifest =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", manifest_path.display()))?;
    if parsed.schema_version == 0 {
        bail!("invalid bundle manifest schema_version=0");
    }
    Ok(Some(parsed))
}

fn enforce_no_models_bundle(payload_root: &Path, manifest: Option<&BundleManifest>) -> Result<()> {
    if let Some(manifest) = manifest
        && (manifest.includes_models || !manifest.artifact_kind.eq_ignore_ascii_case("no-models"))
    {
        bail!(
            "upgrade requires no-models release bundle, got artifact_kind={} includes_models={}",
            manifest.artifact_kind,
            manifest.includes_models
        );
    }
    let models_dir = payload_root.join("models");
    if contains_any_file(&models_dir)? {
        bail!("upgrade bundle unexpectedly contains model artifacts");
    }
    Ok(())
}

fn contains_any_file(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry.with_context(|| format!("read {}", path.display()))?;
        let child = entry.path();
        let ty = entry
            .file_type()
            .with_context(|| format!("metadata {}", child.display()))?;
        if ty.is_file() {
            return Ok(true);
        }
        if ty.is_dir() && contains_any_file(&child)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn install_payload(payload_root: &Path, layout: &InstallLayout) -> Result<()> {
    fs::create_dir_all(&layout.bin_dir)
        .with_context(|| format!("create {}", layout.bin_dir.display()))?;
    fs::create_dir_all(&layout.share_dir)
        .with_context(|| format!("create {}", layout.share_dir.display()))?;

    let bin_root = payload_root.join("bin");
    let termlm_src = if bin_root.join("termlm").exists() {
        bin_root.join("termlm")
    } else if bin_root.join("termlm-client").exists() {
        bin_root.join("termlm-client")
    } else {
        bail!("release bundle missing CLI binary (expected bin/termlm or bin/termlm-client)");
    };
    let core_src = bin_root.join("termlm-core");
    if !core_src.exists() {
        bail!(
            "release bundle missing daemon binary: {}",
            core_src.display()
        );
    }

    install_binary(&termlm_src, &layout.bin_dir.join("termlm"))?;
    install_binary(&core_src, &layout.bin_dir.join("termlm-core"))?;

    let compat_src = if bin_root.join("termlm-client").exists() {
        bin_root.join("termlm-client")
    } else {
        termlm_src.clone()
    };
    install_binary(&compat_src, &layout.bin_dir.join("termlm-client"))?;

    let plugins_src = payload_root.join("plugins").join("zsh");
    if !plugins_src.exists() {
        bail!("release bundle missing zsh plugin directory");
    }
    install_dir_replace(&plugins_src, &layout.plugins_zsh_dir)?;
    Ok(())
}

fn install_binary(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let file_name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "termlm-bin".to_string());
    let tmp = dest.with_file_name(format!(".{file_name}.upgrade-{}", std::process::id()));
    let copy_res =
        fs::copy(src, &tmp).with_context(|| format!("copy {} -> {}", src.display(), tmp.display()));
    if copy_res.is_err() {
        let _ = fs::remove_file(&tmp);
        return copy_res.map(|_| ());
    }
    set_exec_permission(&tmp)?;
    if let Err(e) = fs::rename(&tmp, dest)
        .with_context(|| format!("rename {} -> {}", tmp.display(), dest.display()))
    {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn install_dir_replace(src: &Path, dest: &Path) -> Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow!("invalid destination path {}", dest.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let dest_name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "dir".to_string());
    let tmp_dir = parent.join(format!(".{dest_name}.upgrade-{}", std::process::id()));
    remove_path_if_exists(&tmp_dir)?;
    copy_dir_recursive(src, &tmp_dir)?;
    remove_path_if_exists(dest)?;
    fs::rename(&tmp_dir, dest)
        .with_context(|| format!("rename {} -> {}", tmp_dir.display(), dest.display()))?;
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("metadata {}", path.display()))?;
    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    } else {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let entry = entry.with_context(|| format!("read {}", src.display()))?;
        let source = entry.path();
        let target = dest.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("metadata {}", source.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&source, &target)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        fs::copy(&source, &target)
            .with_context(|| format!("copy {} -> {}", source.display(), target.display()))?;
        let perms = fs::metadata(&source)
            .with_context(|| format!("metadata {}", source.display()))?
            .permissions();
        fs::set_permissions(&target, perms)
            .with_context(|| format!("chmod {}", target.display()))?;
    }
    Ok(())
}

fn write_install_receipt(layout: &InstallLayout, receipt: &InstallReceipt) -> Result<()> {
    let path = layout.share_dir.join("install-receipt.json");
    let tmp = layout.share_dir.join(format!(
        ".install-receipt.json.upgrade-{}",
        std::process::id()
    ));
    let payload = serde_json::to_vec_pretty(receipt).context("serialize install receipt")?;
    fs::create_dir_all(&layout.share_dir)
        .with_context(|| format!("create {}", layout.share_dir.display()))?;
    {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(&payload)
            .with_context(|| format!("write {}", tmp.display()))?;
        file.flush()
            .with_context(|| format!("flush {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", tmp.display()))?;
    }
    fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

async fn shutdown_daemon_best_effort() {
    let socket = default_socket_path();
    if !socket.exists() {
        return;
    }
    let Ok(stream) = UnixStream::connect(&socket).await else {
        return;
    };
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_BYTES)
        .new_codec();
    let framed = Framed::new(stream, codec);
    let mut transport =
        tokio_serde::Framed::new(framed, Json::<ServerMessage, ClientMessage>::default());
    let _ = transport.send(ClientMessage::Shutdown).await;
    let _ = tokio::time::timeout(Duration::from_millis(300), transport.next()).await;
}

fn default_socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: geteuid reads current effective uid.
        let uid = unsafe { libc::geteuid() };
        format!("/tmp/termlm-{uid}")
    });
    PathBuf::from(format!("{runtime}/termlm.sock"))
}

fn expand_tilde(value: &str, home: &Path) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(value)
    }
}

#[cfg(unix)]
fn set_exec_permission(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(path)
        .with_context(|| format!("metadata {}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).with_context(|| format!("chmod {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_exec_permission(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_asset(name: &str) -> GitHubReleaseAsset {
        GitHubReleaseAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.invalid/{name}"),
        }
    }

    #[test]
    fn platform_ids_cover_supported_targets() {
        assert_eq!(platform_id_for("macos", "aarch64"), Some("darwin-arm64"));
        assert_eq!(platform_id_for("macos", "x86_64"), None);
        assert_eq!(platform_id_for("linux", "x86_64"), None);
        assert_eq!(platform_id_for("linux", "aarch64"), None);
        assert_eq!(platform_id_for("windows", "x86_64"), None);
    }

    #[test]
    fn select_upgrade_asset_prefers_no_models_for_platform() {
        let assets = vec![
            sample_asset("termlm-v1.2.0-darwin-arm64-with-models.tar.gz"),
            sample_asset("termlm-v1.2.0-darwin-arm64-no-models.tar.gz"),
            sample_asset("termlm-v1.2.0-linux-arm64-no-models.tar.gz"),
        ];
        let selected = select_upgrade_asset(&assets, "darwin-arm64").expect("select upgrade asset");
        assert!(selected.name.contains("no-models"));
        assert!(selected.name.contains("darwin-arm64"));
    }

    #[test]
    fn parse_sha256sum_file_formats() {
        let root = tempfile::tempdir().expect("tempdir");
        let sums = root.path().join("SHA256SUMS");
        let content = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  termlm-a.tar.gz\n\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa *termlm-b.tar.gz\n\
";
        fs::write(&sums, content).expect("write sums");
        let parsed = parse_sha256_sums(&sums).expect("parse sums");
        assert_eq!(
            parsed.get("termlm-a.tar.gz"),
            Some(&"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string())
        );
        assert_eq!(
            parsed.get("termlm-b.tar.gz"),
            Some(&"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string())
        );
    }

    #[test]
    fn locate_payload_root_supports_nested_top_level_folder() {
        let root = tempfile::tempdir().expect("tempdir");
        let payload = root.path().join("termlm-v9.9.9");
        fs::create_dir_all(payload.join("bin")).expect("create bin");
        fs::create_dir_all(payload.join("plugins/zsh")).expect("create plugin");
        fs::write(payload.join("bin/termlm"), "x").expect("write termlm");
        fs::write(payload.join("bin/termlm-core"), "x").expect("write termlm-core");
        fs::write(payload.join("plugins/zsh/termlm.plugin.zsh"), "x").expect("write plugin");
        let found = locate_payload_root(root.path()).expect("locate payload root");
        assert_eq!(found, payload);
    }

    #[test]
    fn install_payload_copies_binaries_and_plugins() {
        let src = tempfile::tempdir().expect("src tempdir");
        let payload = src.path();
        fs::create_dir_all(payload.join("bin")).expect("create bin");
        fs::create_dir_all(payload.join("plugins/zsh")).expect("create plugin dir");
        fs::write(payload.join("bin/termlm"), "#!/bin/sh\necho termlm\n").expect("write termlm");
        fs::write(payload.join("bin/termlm-core"), "#!/bin/sh\necho core\n")
            .expect("write termlm-core");
        fs::write(payload.join("plugins/zsh/termlm.plugin.zsh"), "# plugin\n")
            .expect("write plugin");

        let dest = tempfile::tempdir().expect("dest tempdir");
        let layout = InstallLayout {
            bin_dir: dest.path().join("bin"),
            share_dir: dest.path().join("share"),
            plugins_zsh_dir: dest.path().join("share/plugins/zsh"),
        };

        install_payload(payload, &layout).expect("install payload");
        assert!(layout.bin_dir.join("termlm").exists());
        assert!(layout.bin_dir.join("termlm-core").exists());
        assert!(layout.bin_dir.join("termlm-client").exists());
        assert!(layout.plugins_zsh_dir.join("termlm.plugin.zsh").exists());
    }

    #[test]
    fn enforce_no_models_bundle_rejects_model_payload() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("models")).expect("create models dir");
        fs::write(root.path().join("models/file.gguf"), "x").expect("write model");
        let err = enforce_no_models_bundle(root.path(), None).expect_err("must reject models");
        assert!(format!("{err:#}").contains("contains model"));
    }

    #[test]
    fn allow_missing_checksums_parser_handles_common_true_values() {
        unsafe { std::env::set_var("TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS", "1") };
        assert!(allow_missing_checksums());
        unsafe { std::env::set_var("TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS", "true") };
        assert!(allow_missing_checksums());
        unsafe { std::env::set_var("TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS", "yes") };
        assert!(allow_missing_checksums());
        unsafe { std::env::set_var("TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS", "0") };
        assert!(!allow_missing_checksums());
        unsafe { std::env::remove_var("TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS") };
    }
}
