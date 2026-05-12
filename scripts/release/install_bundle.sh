#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Install a termlm release bundle into user-local paths.

Usage:
  ./install.sh [--skip-models]

Environment overrides:
  TERMLM_INSTALL_BIN_DIR      (default: ~/.local/bin)
  TERMLM_INSTALL_SHARE_DIR    (default: ~/.local/share/termlm)
  TERMLM_MODELS_DIR           (default: ~/.local/share/termlm/models)
  TERMLM_GITHUB_REPO          (default: thtmnisamnstr/termlm)
  TERMLM_GITHUB_DOWNLOAD_BASE (default: https://github.com)
  TERMLM_RELEASE_TAG          (override release tag when using chunked model assets)
  TERMLM_GITHUB_TOKEN/GITHUB_TOKEN
                              Optional token for private releases or rate limiting
  TERMLM_MODEL_DOWNLOAD_RETRIES (default: 3)
  TERMLM_MODEL_DOWNLOAD_TIMEOUT_SECS (default: 300)
  TERMLM_INSTALL_WAIT_FOR_READY (default: 1; set to 0 to skip daemon/index readiness wait)
  TERMLM_INSTALL_READY_TIMEOUT_SECS (default: 900)
  TERMLM_INSTALL_READY_POLL_SECS (default: 2)

Install behavior:
  Chunked model downloads and runtime readiness both emit periodic progress.
USAGE
}

SKIP_MODELS=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-models)
      SKIP_MODELS=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${TERMLM_INSTALL_BIN_DIR:-$HOME/.local/bin}"
SHARE_DIR="${TERMLM_INSTALL_SHARE_DIR:-$HOME/.local/share/termlm}"
MODELS_DIR="${TERMLM_MODELS_DIR:-$HOME/.local/share/termlm/models}"
GITHUB_REPO="${TERMLM_GITHUB_REPO:-thtmnisamnstr/termlm}"
GITHUB_DOWNLOAD_BASE="${TERMLM_GITHUB_DOWNLOAD_BASE:-https://github.com}"

mkdir -p "$BIN_DIR" "$SHARE_DIR/plugins"

if [[ ! -f "$ROOT_DIR/bin/termlm-core" ]]; then
  echo "bundle is missing bin/termlm-core" >&2
  exit 1
fi
if [[ ! -f "$ROOT_DIR/bin/termlm" && ! -f "$ROOT_DIR/bin/termlm-client" ]]; then
  echo "bundle is missing bin/termlm or bin/termlm-client" >&2
  exit 1
fi

if [[ -f "$ROOT_DIR/bin/termlm" ]]; then
  install -m 0755 "$ROOT_DIR/bin/termlm" "$BIN_DIR/termlm"
else
  install -m 0755 "$ROOT_DIR/bin/termlm-client" "$BIN_DIR/termlm"
fi

install -m 0755 "$ROOT_DIR/bin/termlm-core" "$BIN_DIR/termlm-core"
if [[ -f "$ROOT_DIR/bin/termlm-client" ]]; then
  install -m 0755 "$ROOT_DIR/bin/termlm-client" "$BIN_DIR/termlm-client"
else
  install -m 0755 "$BIN_DIR/termlm" "$BIN_DIR/termlm-client"
fi

rm -rf "$SHARE_DIR/plugins/zsh"
cp -R "$ROOT_DIR/plugins/zsh" "$SHARE_DIR/plugins/zsh"

verify_installed_payload() {
  if [[ ! -x "$BIN_DIR/termlm" ]]; then
    echo "installed CLI is missing or not executable: $BIN_DIR/termlm" >&2
    exit 1
  fi
  if [[ ! -x "$BIN_DIR/termlm-core" ]]; then
    echo "installed daemon is missing or not executable: $BIN_DIR/termlm-core" >&2
    exit 1
  fi
  if [[ ! -f "$SHARE_DIR/plugins/zsh/termlm.plugin.zsh" ]]; then
    echo "installed zsh plugin is missing: $SHARE_DIR/plugins/zsh/termlm.plugin.zsh" >&2
    exit 1
  fi
  "$BIN_DIR/termlm" --help >/dev/null || {
    echo "installed CLI failed to run: $BIN_DIR/termlm --help" >&2
    exit 1
  }
  "$BIN_DIR/termlm-core" --help >/dev/null || {
    echo "installed daemon failed to run: $BIN_DIR/termlm-core --help" >&2
    exit 1
  }
}

verify_installed_payload

resolve_release_tag() {
  if [[ -n "${TERMLM_RELEASE_TAG:-}" ]]; then
    echo "$TERMLM_RELEASE_TAG"
    return
  fi
  local manifest_path="$ROOT_DIR/bundle-manifest.json"
  if [[ -f "$manifest_path" ]] && command -v python3 >/dev/null 2>&1; then
    local version
    version="$(python3 - "$manifest_path" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    payload = json.load(f)
version = str(payload.get("version", "")).strip()
if not version:
    print("")
elif version.startswith("v"):
    print(version)
else:
    print(f"v{version}")
PY
)"
    if [[ -n "$version" ]]; then
      echo "$version"
      return
    fi
  fi
  echo ""
}

detect_bundle_artifact_kind() {
  local manifest_path="$ROOT_DIR/bundle-manifest.json"
  if [[ -f "$manifest_path" ]] && command -v python3 >/dev/null 2>&1; then
    local kind
    kind="$(python3 - "$manifest_path" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    payload = json.load(f)
kind = str(payload.get("artifact_kind", "")).strip().lower()
if kind in {"with-models", "no-models"}:
    print(kind)
else:
    print("")
PY
)"
    if [[ -n "$kind" ]]; then
      echo "$kind"
      return
    fi
  fi
  if [[ -d "$ROOT_DIR/models" ]]; then
    echo "with-models"
  else
    echo "no-models"
  fi
}

download_chunked_models() {
  local models_manifest="$1"
  local release_tag="$2"
  if ! command -v python3 >/dev/null 2>&1; then
    echo "python3 is required to install chunked model assets" >&2
    exit 1
  fi

  python3 - "$models_manifest" "$MODELS_DIR" "$GITHUB_REPO" "$release_tag" "$ROOT_DIR" "$GITHUB_DOWNLOAD_BASE" <<'PY'
import hashlib
import json
import os
import shutil
import sys
import tempfile
import time
import urllib.error
import urllib.request

manifest_path, models_dir, repo, release_tag, bundle_root, download_base = sys.argv[1:7]
with open(manifest_path, "r", encoding="utf-8") as f:
    manifest = json.load(f)
models = manifest.get("models", [])
if not models:
    raise SystemExit("models manifest is empty")

os.makedirs(models_dir, exist_ok=True)
temp_root = tempfile.mkdtemp(prefix="termlm-model-install-")

def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            chunk = f.read(1024 * 1024)
            if not chunk:
                break
            h.update(chunk)
    return h.hexdigest()

def human_bytes(value: int) -> str:
    size = float(value)
    units = ["B", "KiB", "MiB", "GiB", "TiB"]
    for unit in units:
        if size < 1024.0 or unit == units[-1]:
            if unit == "B":
                return f"{int(size)} {unit}"
            return f"{size:.1f} {unit}"
        size /= 1024.0
    return f"{int(value)} B"

def int_env(name: str, default: int) -> int:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        parsed = int(raw)
    except ValueError:
        print(f"  ignoring invalid {name}={raw!r}; using {default}", file=sys.stderr)
        return default
    return max(1, parsed)

def download_with_progress(url: str, chunk_path: str, asset_name: str) -> None:
    token = os.environ.get("TERMLM_GITHUB_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""
    headers = {
        "Accept": "application/octet-stream",
        "User-Agent": f"termlm-installer/{release_tag or 'unknown'}",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    timeout = int_env("TERMLM_MODEL_DOWNLOAD_TIMEOUT_SECS", 300)
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=timeout) as response, open(chunk_path, "wb") as out:
        total_header = response.headers.get("Content-Length")
        total = int(total_header) if total_header and total_header.isdigit() else 0
        downloaded = 0
        next_report = time.monotonic()
        report_interval_secs = 5.0

        while True:
            block = response.read(1024 * 1024)
            if not block:
                break
            out.write(block)
            downloaded += len(block)
            now = time.monotonic()
            if now >= next_report:
                if total > 0:
                    pct = (downloaded / total) * 100.0
                    print(
                        f"  {asset_name}: {human_bytes(downloaded)} / {human_bytes(total)} ({pct:.1f}%)",
                        file=sys.stderr,
                    )
                else:
                    print(f"  {asset_name}: {human_bytes(downloaded)} downloaded", file=sys.stderr)
                next_report = now + report_interval_secs

        if total > 0:
            print(
                f"  {asset_name}: download complete ({human_bytes(downloaded)} / {human_bytes(total)})",
                file=sys.stderr,
            )
        else:
            print(f"  {asset_name}: download complete ({human_bytes(downloaded)})", file=sys.stderr)

def download_with_retries(url: str, chunk_path: str, asset_name: str) -> None:
    retries = int_env("TERMLM_MODEL_DOWNLOAD_RETRIES", 3)
    last_error = None
    for attempt in range(1, retries + 1):
        try:
            if os.path.exists(chunk_path):
                os.remove(chunk_path)
            download_with_progress(url, chunk_path, asset_name)
            return
        except urllib.error.HTTPError as exc:
            if exc.code == 404:
                raise SystemExit(
                    f"model chunk not found: {asset_name}\n"
                    f"  url: {url}\n"
                    f"  release tag: {release_tag or '(empty)'}\n"
                    "Check that every models-manifest.json chunk is uploaded to the GitHub release, "
                    "or set TERMLM_RELEASE_TAG to the tag containing the chunk assets."
                ) from None
            last_error = f"HTTP {exc.code} {exc.reason}"
        except Exception as exc:
            last_error = str(exc)

        if attempt < retries:
            print(
                f"  {asset_name}: download failed ({last_error}); retrying {attempt + 1}/{retries}",
                file=sys.stderr,
            )
            time.sleep(min(2 * attempt, 10))

    raise SystemExit(f"failed to download model chunk {asset_name} after {retries} attempt(s): {last_error}")

base_url = f"{download_base.rstrip('/')}/{repo}/releases/download/{release_tag}".rstrip("/")

try:
    for model in models:
        filename = model["filename"]
        expected_model_sha = model["sha256"].lower()
        final_path = os.path.join(models_dir, filename)
        if os.path.exists(final_path):
            existing_sha = sha256_file(final_path).lower()
            if existing_sha == expected_model_sha:
                print(f"model already present: {filename}", file=sys.stderr)
                continue

        assembled_path = os.path.join(temp_root, f"{filename}.assembled")
        with open(assembled_path, "wb") as assembled:
            for chunk in model.get("chunks", []):
                asset_name = chunk["asset_name"]
                expected_chunk_sha = chunk["sha256"].lower()
                local_candidate = os.path.join(bundle_root, "models", "chunks", asset_name)
                chunk_path = os.path.join(temp_root, asset_name)
                if os.path.exists(local_candidate):
                    print(f"using bundled model chunk: {asset_name}", file=sys.stderr)
                    shutil.copyfile(local_candidate, chunk_path)
                else:
                    if not release_tag:
                        raise SystemExit(
                            "release tag is required to download missing model chunks; set TERMLM_RELEASE_TAG"
                        )
                    url = f"{base_url}/{asset_name}"
                    print(f"downloading model chunk: {asset_name}", file=sys.stderr)
                    download_with_retries(url, chunk_path, asset_name)

                actual_chunk_sha = sha256_file(chunk_path).lower()
                if actual_chunk_sha != expected_chunk_sha:
                    raise SystemExit(
                        f"chunk checksum mismatch for {asset_name}: expected {expected_chunk_sha} got {actual_chunk_sha}"
                    )
                with open(chunk_path, "rb") as chunk_file:
                    shutil.copyfileobj(chunk_file, assembled)
                os.remove(chunk_path)

        actual_model_sha = sha256_file(assembled_path).lower()
        if actual_model_sha != expected_model_sha:
            raise SystemExit(
                f"model checksum mismatch for {filename}: expected {expected_model_sha} got {actual_model_sha}"
            )
        os.makedirs(os.path.dirname(final_path), exist_ok=True)
        os.replace(assembled_path, final_path)
        print(f"installed model: {filename}", file=sys.stderr)
finally:
    shutil.rmtree(temp_root, ignore_errors=True)
PY
}

is_truthy() {
  local raw="${1:-}"
  raw="$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]')"
  case "$raw" in
    1|true|yes|on)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

validate_positive_int_or_default() {
  local value="$1"
  local fallback="$2"
  if [[ "$value" =~ ^[0-9]+$ ]] && [[ "$value" -ge 1 ]]; then
    echo "$value"
  else
    echo "$fallback"
  fi
}

write_embed_only_bootstrap_config() {
  local out_path="$1"
  local source_config="$HOME/.config/termlm/config.toml"
  if ! command -v python3 >/dev/null 2>&1; then
    echo "python3 is required for embed-only bootstrap config generation" >&2
    exit 1
  fi
  python3 - "$source_config" "$out_path" "$MODELS_DIR" <<'PY'
import json
import os
import pathlib
import sys

try:
    import tomllib
except Exception:  # pragma: no cover
    tomllib = None

source_config, out_path, models_dir = sys.argv[1:4]

cfg = {}
if tomllib is not None and os.path.exists(source_config):
    try:
        with open(source_config, "rb") as f:
            cfg = tomllib.load(f)
    except Exception:
        cfg = {}

def get(path, default):
    cur = cfg
    for part in path:
        if not isinstance(cur, dict) or part not in cur:
            return default
        cur = cur[part]
    return cur

def as_bool(value, default):
    if isinstance(value, bool):
        return value
    return default

def as_int(value, default):
    if isinstance(value, int):
        return value
    return default

def as_string(value, default):
    if isinstance(value, str) and value.strip():
        return value
    return default

socket_path = as_string(get(("daemon", "socket_path"), ""), "$XDG_RUNTIME_DIR/termlm.sock")
pid_file = as_string(get(("daemon", "pid_file"), ""), "$XDG_RUNTIME_DIR/termlm.pid")
log_file = as_string(get(("daemon", "log_file"), ""), "~/.local/state/termlm/termlm.log")

ollama_endpoint = as_string(get(("ollama", "endpoint"), ""), "http://127.0.0.1:11434")
ollama_model = as_string(get(("ollama", "model"), ""), "gemma4:e4b")
ollama_keep_alive = as_string(get(("ollama", "keep_alive"), ""), "5m")
ollama_request_timeout = as_int(get(("ollama", "request_timeout_secs"), 300), 300)
ollama_connect_timeout = as_int(get(("ollama", "connect_timeout_secs"), 3), 3)
ollama_allow_remote = as_bool(get(("ollama", "allow_remote"), False), False)
ollama_allow_plain_http_remote = as_bool(
    get(("ollama", "allow_plain_http_remote"), False), False
)

embed_filename = as_string(
    get(("indexer", "embed_filename"), ""),
    "bge-small-en-v1.5.Q4_K_M.gguf",
)

lines = [
    "[daemon]",
    f"socket_path = {json.dumps(socket_path)}",
    f"pid_file = {json.dumps(pid_file)}",
    f"log_file = {json.dumps(log_file)}",
    "",
    "[model]",
    f"models_dir = {json.dumps(models_dir)}",
    "auto_download = true",
    "",
    "[inference]",
    'provider = "ollama"',
    "startup_failure_behavior = \"fail\"",
    "",
    "[ollama]",
    f"endpoint = {json.dumps(ollama_endpoint)}",
    f"model = {json.dumps(ollama_model)}",
    f"keep_alive = {json.dumps(ollama_keep_alive)}",
    f"request_timeout_secs = {ollama_request_timeout}",
    f"connect_timeout_secs = {ollama_connect_timeout}",
    f"allow_remote = {'true' if ollama_allow_remote else 'false'}",
    f"allow_plain_http_remote = {'true' if ollama_allow_plain_http_remote else 'false'}",
    "healthcheck_on_start = false",
    "",
    "[web]",
    "enabled = true",
    "expose_tools = true",
    'provider = "duckduckgo_html"',
    "",
    "[indexer]",
    "enabled = true",
    'embedding_provider = "local"',
    f"embed_filename = {json.dumps(embed_filename)}",
    "",
]

out_parent = pathlib.Path(out_path).parent
out_parent.mkdir(parents=True, exist_ok=True)
with open(out_path, "w", encoding="utf-8") as f:
    f.write("\n".join(lines))

print(embed_filename)
PY
}

wait_for_runtime_ready() {
  local termlm_bin="$BIN_DIR/termlm"
  local termlm_core_bin="$BIN_DIR/termlm-core"
  local runtime_dir
  local socket_path
  local pid_path
  local daemon_log_path="${XDG_STATE_HOME:-$HOME/.local/state}/termlm/termlm.log"
  local timeout_secs
  local poll_secs
  local embed_only_bootstrap=0
  local embed_filename=""
  local bootstrap_config=""
  local core_pid=""
  if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
    runtime_dir="${XDG_RUNTIME_DIR}"
  else
    runtime_dir="/tmp/termlm-$(id -u)"
  fi
  socket_path="${runtime_dir}/termlm.sock"
  pid_path="${runtime_dir}/termlm.pid"
  timeout_secs="$(validate_positive_int_or_default "${TERMLM_INSTALL_READY_TIMEOUT_SECS:-900}" "900")"
  poll_secs="$(validate_positive_int_or_default "${TERMLM_INSTALL_READY_POLL_SECS:-2}" "2")"
  mkdir -p "$(dirname "$daemon_log_path")"

  if [[ ! -x "$termlm_bin" || ! -x "$termlm_core_bin" ]]; then
    echo "cannot wait for runtime readiness: installed binaries missing under $BIN_DIR" >&2
    exit 1
  fi

  if [[ "$BUNDLE_ARTIFACT_KIND" == "no-models" ]]; then
    embed_only_bootstrap=1
    local bootstrap_tmp
    bootstrap_tmp="$(mktemp "${TMPDIR:-/tmp}/termlm-install-bootstrap.XXXXXX")"
    bootstrap_config="${bootstrap_tmp}.toml"
    mv "$bootstrap_tmp" "$bootstrap_config"
    embed_filename="$(write_embed_only_bootstrap_config "$bootstrap_config")"
    if [[ -z "$embed_filename" ]]; then
      echo "failed to determine embedding filename for no-models bootstrap" >&2
      exit 1
    fi
  fi

  fail_with_logs() {
    local message="$1"
    echo "$message" >&2
    echo "socket path: $socket_path" >&2
    if [[ -e "$socket_path" ]]; then
      ls -l "$socket_path" >&2 || true
    else
      echo "socket missing: $socket_path" >&2
    fi
    if [[ -f "$daemon_log_path" ]]; then
      echo "recent daemon log tail:" >&2
      tail -n 80 "$daemon_log_path" >&2 || true
    fi
    if [[ -n "${core_pid:-}" ]] && declare -F stop_core_instance >/dev/null 2>&1; then
      stop_core_instance "$core_pid" >/dev/null 2>&1 || true
    fi
    if [[ -n "${bootstrap_config:-}" ]]; then
      rm -f "$bootstrap_config" >/dev/null 2>&1 || true
    fi
    exit 1
  }

  stop_core_instance() {
    local pid="${1:-}"
    if [[ -n "$pid" ]] && kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1 || true
      sleep 1
      if kill -0 "$pid" >/dev/null 2>&1; then
        kill -9 "$pid" >/dev/null 2>&1 || true
      fi
    fi
    "$termlm_bin" stop >/dev/null 2>&1 || true
    rm -f "$socket_path" "$pid_path" >/dev/null 2>&1 || true
  }

  start_core_instance() {
    local cfg_path="${1:-}"
    if [[ -n "$cfg_path" ]]; then
      nohup "$termlm_core_bin" --config "$cfg_path" >>"$daemon_log_path" 2>&1 < /dev/null &
    else
      nohup "$termlm_core_bin" >>"$daemon_log_path" 2>&1 < /dev/null &
    fi
    core_pid=$!
    disown "$core_pid" 2>/dev/null || true
    sleep 1
    if [[ -z "$core_pid" ]] || ! kill -0 "$core_pid" >/dev/null 2>&1; then
      fail_with_logs "failed to start termlm-core for readiness bootstrap"
    fi
  }

  status_with_timeout() {
    python3 - "$termlm_bin" "status" <<'PY'
import subprocess
import sys

bin_path, subcommand = sys.argv[1:3]
try:
    proc = subprocess.run(
        [bin_path, subcommand],
        capture_output=True,
        text=True,
        timeout=3,
    )
except subprocess.TimeoutExpired:
    sys.exit(124)

sys.stdout.write(proc.stdout)
sys.stderr.write(proc.stderr)
sys.exit(proc.returncode)
PY
  }
  status_verbose_with_timeout() {
    python3 - "$termlm_bin" <<'PY'
import subprocess
import sys

bin_path = sys.argv[1]
try:
    proc = subprocess.run(
        [bin_path, "status", "--verbose"],
        capture_output=True,
        text=True,
        timeout=4,
    )
except subprocess.TimeoutExpired:
    sys.exit(124)

sys.stdout.write(proc.stdout)
sys.stderr.write(proc.stderr)
sys.exit(proc.returncode)
PY
  }
  trigger_reindex_with_timeout() {
    local mode="$1"
    python3 - "$termlm_bin" "$mode" <<'PY'
import subprocess
import sys

bin_path, mode = sys.argv[1:3]
try:
    proc = subprocess.run(
        [bin_path, "reindex", "--mode", mode],
        capture_output=True,
        text=True,
        timeout=6,
    )
except subprocess.TimeoutExpired:
    # Treat timeout as accepted trigger; daemon may already be indexing.
    sys.exit(0)

if proc.returncode == 0:
    sys.exit(0)

if proc.stderr:
    sys.stderr.write(proc.stderr)
elif proc.stdout:
    sys.stderr.write(proc.stdout)
sys.exit(proc.returncode if proc.returncode != 0 else 1)
PY
  }
  stop_core_instance ""
  if [[ $embed_only_bootstrap -eq 1 ]]; then
    start_core_instance "$bootstrap_config"
  else
    start_core_instance ""
  fi

  local startup_deadline=$((SECONDS + 45))
  local startup_ready=0
  while (( SECONDS < startup_deadline )); do
    if status_with_timeout >/dev/null 2>&1; then
      startup_ready=1
      break
    fi
    # If termlm-core is already gone and no socket exists, fail quickly.
    if [[ -n "$core_pid" ]] && ! kill -0 "$core_pid" >/dev/null 2>&1 && [[ ! -S "$socket_path" ]]; then
      break
    fi
    sleep 1
  done
  if [[ "$startup_ready" -ne 1 ]]; then
    fail_with_logs "termlm-core did not become reachable after startup."
  fi

  local index_manifest_path="$HOME/.local/share/termlm/index/manifest.json"
  local reindex_requested=0

  request_initial_reindex() {
    local attempts="${1:-10}"
    local i=0
    while (( i < attempts )); do
      if trigger_reindex_with_timeout "delta" >/dev/null 2>&1; then
        reindex_requested=1
        return 0
      fi
      (( i += 1 ))
      sleep 1
    done
    return 1
  }

  # If status is already reachable, kick indexing immediately; otherwise defer
  # until the readiness loop can reach `termlm status`.
  if [[ "$startup_ready" -eq 1 ]]; then
    request_initial_reindex 10 >/dev/null 2>&1 || true
  fi

  echo "Waiting for termlm runtime/model/index readiness (timeout ${timeout_secs}s)..."
  local deadline=$((SECONDS + timeout_secs))
  local last_status=""
  local last_reported_progress=""
  local last_reported_provider=""
  local last_report_at=0
  local accepted_empty_index=0
  local status_timeout_streak=0
  while (( SECONDS < deadline )); do
    if [[ -n "$core_pid" ]] && ! kill -0 "$core_pid" >/dev/null 2>&1; then
      fail_with_logs "termlm-core is not running during readiness wait."
    fi

    local status_rc=0
    if last_status="$(status_verbose_with_timeout 2>/dev/null)"; then
      status_rc=0
    else
      status_rc=$?
      last_status=""
    fi

    if [[ "$status_rc" -eq 124 ]]; then
      status_timeout_streak=$((status_timeout_streak + 1))
    else
      status_timeout_streak=0
    fi
    if [[ "$status_timeout_streak" -ge 6 ]]; then
      fail_with_logs "status command timed out repeatedly while waiting for readiness."
    fi

    local progress_line="phase=starting"
    local progress_phase="starting"
    local progress_percent="0"
    local provider_health="unknown"
    local chunk_count="0"
    if [[ $status_rc -eq 0 ]]; then
      if [[ "$reindex_requested" -eq 0 ]]; then
        request_initial_reindex 1 >/dev/null 2>&1 || true
      fi
      progress_line="$(printf '%s\n' "$last_status" | awk -F': ' '/^index_progress:/ {print $2; exit}')"
      progress_phase="$(printf '%s\n' "$progress_line" | sed -E 's/^phase=([^[:space:]]+).*/\1/')"
      progress_percent="$(printf '%s\n' "$progress_line" | awk '{for(i=1;i<=NF;i++){if($i ~ /^percent=/){split($i,a,"="); print a[2]; exit}}}')"
      provider_health="$(printf '%s\n' "$last_status" | awk -F': ' '/^provider_healthy:/ {print $2; exit}')"
      chunk_count="$(printf '%s\n' "$last_status" | awk -F': ' '/^index_chunk_count:/ {print $2; exit}')"
      [[ -z "$progress_line" ]] && progress_line="phase=unknown"
      [[ -z "$progress_phase" ]] && progress_phase="unknown"
      [[ -z "$progress_percent" ]] && progress_percent="0"
      [[ -z "$provider_health" ]] && provider_health="unknown"
      [[ -z "$chunk_count" ]] && chunk_count="0"
    fi

    if (( SECONDS - last_report_at >= 10 )) || [[ "$progress_line" != "$last_reported_progress" ]] || [[ "$provider_health" != "$last_reported_provider" ]]; then
      echo "  progress: ${progress_line}; chunks=${chunk_count}; provider_healthy=${provider_health}"
      last_reported_progress="$progress_line"
      last_reported_provider="$provider_health"
      last_report_at=$SECONDS
    fi

    if [[ $status_rc -eq 0 ]]; then
      local manifest_chunk_count="-1"
      if [[ -f "$index_manifest_path" ]]; then
        manifest_chunk_count="$(python3 - "$index_manifest_path" <<'PY'
import json
import sys

path = sys.argv[1]
try:
    with open(path, "r", encoding="utf-8") as f:
        payload = json.load(f)
    chunk_count = int(payload.get("chunk_count", 0) or 0)
    print(chunk_count)
except Exception:
    print(-1)
PY
)"
      fi

      local phase_complete=0
      if [[ "$progress_phase" == "complete" || "$progress_phase" == "idle" ]]; then
        phase_complete=1
      fi
      local percent_complete=0
      if awk -v pct="$progress_percent" 'BEGIN { exit !(pct+0 >= 100.0) }'; then
        percent_complete=1
      fi
      local index_ready=0
      local index_dir
      index_dir="$(dirname "$index_manifest_path")"
      local persisted_index_ready=0
      if [[ "$manifest_chunk_count" =~ ^[0-9]+$ && "$manifest_chunk_count" -gt 0 \
        && -s "$index_dir/vectors.f16" \
        && -s "$index_dir/lexicon.bin" \
        && -s "$index_dir/postings.bin" ]]; then
        persisted_index_ready=1
      fi
      if [[ "$persisted_index_ready" -eq 1 ]]; then
        index_ready=1
      fi
      if [[ "$phase_complete" -eq 1 && "$percent_complete" -eq 1 && "$manifest_chunk_count" == "0" ]]; then
        index_ready=1
        if [[ "$accepted_empty_index" -eq 0 ]]; then
          echo "  progress: index phase is complete with zero chunks; proceeding."
          accepted_empty_index=1
        fi
      fi

      if [[ "$index_ready" -eq 1 ]]; then
        if [[ $embed_only_bootstrap -eq 0 && "$provider_health" != "true" ]]; then
          sleep "$poll_secs"
          continue
        fi
        if [[ $embed_only_bootstrap -eq 1 ]]; then
          local embed_path="$MODELS_DIR/$embed_filename"
          if [[ ! -s "$embed_path" ]]; then
            fail_with_logs "embedding bootstrap incomplete: expected ${embed_path} to exist"
          fi
          echo "termlm embedding/index bootstrap is ready."
          stop_core_instance "$core_pid"
          rm -f "$bootstrap_config" >/dev/null 2>&1 || true
          return 0
        fi
        echo "termlm runtime is ready."
        return 0
      fi
    fi

    sleep "$poll_secs"
  done

  echo "timed out waiting for termlm readiness after ${timeout_secs}s" >&2
  if [[ -n "$last_status" ]]; then
    echo "last observed status:" >&2
    printf '%s\n' "$last_status" >&2
  fi
  fail_with_logs "readiness timeout reached."
}

BUNDLE_ARTIFACT_KIND="$(detect_bundle_artifact_kind)"

if [[ $SKIP_MODELS -eq 0 && -d "$ROOT_DIR/models" ]]; then
  mkdir -p "$MODELS_DIR"
  if compgen -G "$ROOT_DIR/models/*.gguf" >/dev/null 2>&1; then
    cp -R "$ROOT_DIR/models/." "$MODELS_DIR/"
  elif [[ -f "$ROOT_DIR/models/models-manifest.json" ]]; then
    tag="$(resolve_release_tag)"
    if [[ "$tag" =~ [Xx]\.[Yy]\.[Zz] || "$tag" == *"<"* || "$tag" == *">"* ]]; then
      echo "bundle manifest contains placeholder release tag '${tag}'; set TERMLM_RELEASE_TAG to the actual GitHub release tag" >&2
      exit 1
    fi
    download_chunked_models "$ROOT_DIR/models/models-manifest.json" "$tag"
  fi
fi

if [[ $SKIP_MODELS -eq 0 ]]; then
  if is_truthy "${TERMLM_INSTALL_WAIT_FOR_READY:-1}"; then
    wait_for_runtime_ready
  else
    echo "Skipping runtime/index readiness wait (TERMLM_INSTALL_WAIT_FOR_READY=0)"
  fi
fi

echo "Installed termlm binaries to: $BIN_DIR"
echo "Installed zsh plugin to:      $SHARE_DIR/plugins/zsh"
if [[ $SKIP_MODELS -eq 0 ]]; then
  echo "Installed model assets to:    $MODELS_DIR"
fi
