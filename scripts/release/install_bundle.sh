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
  TERMLM_RELEASE_TAG          (override release tag when using chunked model assets)
  TERMLM_INSTALL_WAIT_FOR_READY (default: 1; set to 0 to skip daemon/index readiness wait)
  TERMLM_INSTALL_READY_TIMEOUT_SECS (default: 1800)
  TERMLM_INSTALL_READY_POLL_SECS (default: 2)
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

  python3 - "$models_manifest" "$MODELS_DIR" "$GITHUB_REPO" "$release_tag" "$ROOT_DIR" <<'PY'
import hashlib
import json
import os
import shutil
import sys
import tempfile
import urllib.request

manifest_path, models_dir, repo, release_tag, bundle_root = sys.argv[1:6]
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

base_url = f"https://github.com/{repo}/releases/download/{release_tag}".rstrip("/")

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
                    shutil.copyfile(local_candidate, chunk_path)
                else:
                    url = f"{base_url}/{asset_name}"
                    print(f"downloading model chunk: {asset_name}", file=sys.stderr)
                    urllib.request.urlretrieve(url, chunk_path)

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
  local timeout_secs
  local poll_secs
  local embed_only_bootstrap=0
  local embed_filename=""
  local bootstrap_config=""
  timeout_secs="$(validate_positive_int_or_default "${TERMLM_INSTALL_READY_TIMEOUT_SECS:-1800}" "1800")"
  poll_secs="$(validate_positive_int_or_default "${TERMLM_INSTALL_READY_POLL_SECS:-2}" "2")"

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

  cleanup_bootstrap() {
    if [[ $embed_only_bootstrap -eq 1 ]]; then
      "$termlm_bin" stop >/dev/null 2>&1 || true
      if [[ -n "$bootstrap_config" ]]; then
        rm -f "$bootstrap_config"
      fi
    fi
  }
  trap cleanup_bootstrap RETURN

  "$termlm_bin" stop >/dev/null 2>&1 || true
  if [[ $embed_only_bootstrap -eq 1 ]]; then
    if ! "$termlm_core_bin" --config "$bootstrap_config" --detach >/dev/null 2>&1; then
      echo "failed to start termlm-core for no-models embed/index bootstrap" >&2
      exit 1
    fi
  else
    if ! "$termlm_core_bin" --detach >/dev/null 2>&1; then
      echo "failed to start termlm-core for readiness bootstrap" >&2
      exit 1
    fi
  fi

  # Trigger a delta refresh explicitly; harmless if one is already running.
  "$termlm_bin" reindex --mode delta >/dev/null 2>&1 || true

  echo "Waiting for termlm runtime/model/index readiness (timeout ${timeout_secs}s)..."
  local deadline=$((SECONDS + timeout_secs))
  local last_status=""
  while (( SECONDS < deadline )); do
    local status_out=""
    if status_out="$("$termlm_bin" status 2>/dev/null)"; then
      last_status="$status_out"
      local provider
      local provider_healthy
      local phase
      local percent
      provider="$(printf '%s\n' "$status_out" | awk -F': ' '/^provider:/ {print $2; exit}')"
      provider_healthy="$(printf '%s\n' "$status_out" | awk -F': ' '/^provider_healthy:/ {print $2; exit}')"
      phase="$(printf '%s\n' "$status_out" | sed -n 's/^index_progress: phase=\([^ ]*\) percent=.*/\1/p' | head -n 1)"
      percent="$(printf '%s\n' "$status_out" | sed -n 's/^index_progress: phase=[^ ]* percent=\([0-9.][0-9.]*\).*/\1/p' | head -n 1)"

      local index_ready=0
      local provider_ready=1

      if [[ "$phase" == "complete" || "$phase" == "idle" ]]; then
        if [[ -n "$percent" ]] && awk "BEGIN { exit !($percent >= 100.0) }"; then
          index_ready=1
        fi
      fi

      if [[ "$provider" == "local" && "$provider_healthy" != "true" ]]; then
        provider_ready=0
      fi

      if [[ "$index_ready" -eq 1 && "$provider_ready" -eq 1 ]]; then
        if [[ $embed_only_bootstrap -eq 1 ]]; then
          local embed_path="$MODELS_DIR/$embed_filename"
          if [[ ! -s "$embed_path" ]]; then
            echo "embedding bootstrap incomplete: expected ${embed_path} to exist" >&2
            exit 1
          fi
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
  exit 1
}

BUNDLE_ARTIFACT_KIND="$(detect_bundle_artifact_kind)"

if [[ $SKIP_MODELS -eq 0 && -d "$ROOT_DIR/models" ]]; then
  mkdir -p "$MODELS_DIR"
  if compgen -G "$ROOT_DIR/models/*.gguf" >/dev/null 2>&1; then
    cp -R "$ROOT_DIR/models/." "$MODELS_DIR/"
  elif [[ -f "$ROOT_DIR/models/models-manifest.json" ]]; then
    tag="$(resolve_release_tag)"
    if [[ -z "$tag" ]]; then
      echo "unable to resolve release tag for chunked model download; set TERMLM_RELEASE_TAG" >&2
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
