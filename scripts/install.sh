#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Install termlm from GitHub Releases.

Usage:
  scripts/install.sh [--repo <owner/repo>] [--tag <vX.Y.Z>] [--no-models] [--skip-models]

Options:
  --repo <owner/repo>  Override release repository (default: thtmnisamnstr/termlm)
  --tag <vX.Y.Z>       Install a specific tag instead of latest
  --no-models          Use the no-models bundle (no bundled LLM; embed/index bootstrap still runs)
  --skip-models        Pass --skip-models to bundle install.sh
  -h, --help           Show help

Notes:
  By default, bundle install waits for daemon/model/index readiness before returning.
  no-models install bootstraps embeddings + index only and does not fetch the inference GGUF.
  Use --skip-models (or TERMLM_INSTALL_WAIT_FOR_READY=0) to skip readiness/model bootstrap.

Environment:
  TERMLM_GITHUB_TOKEN  Optional GitHub token for higher API rate limits
  GITHUB_TOKEN         Fallback token env var
  TERMLM_GITHUB_API_BASE
                       Override release API repo base (default: https://api.github.com/repos)

Prerequisites:
  curl, python3, shasum
USAGE
}

require_cmd() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "required command not found: $cmd" >&2
    exit 1
  fi
}

REPO="${TERMLM_GITHUB_REPO:-thtmnisamnstr/termlm}"
TAG=""
ARTIFACT_KIND="with-models"
SKIP_MODELS=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)
      REPO="${2:-}"
      shift 2
      ;;
    --tag)
      TAG="${2:-}"
      shift 2
      ;;
    --no-models)
      ARTIFACT_KIND="no-models"
      shift
      ;;
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

require_cmd curl
require_cmd python3
require_cmd shasum

if [[ ! "$REPO" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]]; then
  echo "invalid --repo value: ${REPO} (expected owner/repo)" >&2
  exit 2
fi

OS="$(uname -s)"
ARCH="$(uname -m)"
TARGET=""
case "${OS}:${ARCH}" in
  Darwin:arm64)
    TARGET="darwin-arm64"
    ;;
  *)
    echo "unsupported platform for installer: ${OS}/${ARCH}" >&2
    echo "currently supported: macOS arm64" >&2
    exit 1
    ;;
esac

TOKEN="${TERMLM_GITHUB_TOKEN:-${GITHUB_TOKEN:-}}"
API_REPOS_BASE="${TERMLM_GITHUB_API_BASE:-https://api.github.com/repos}"
API_REPOS_BASE="${API_REPOS_BASE%/}"
API_BASE="${API_REPOS_BASE}/${REPO}/releases"
if [[ -n "$TAG" ]]; then
  RELEASE_URL="${API_BASE}/tags/${TAG}"
else
  RELEASE_URL="${API_BASE}/latest"
fi

TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termlm-install.XXXXXX")"
trap 'rm -rf "$TMP_ROOT"' EXIT

HEADER_ARGS=(-H "Accept: application/vnd.github+json" -H "User-Agent: termlm-installer")
if [[ -n "$TOKEN" ]]; then
  HEADER_ARGS+=(-H "Authorization: Bearer ${TOKEN}")
fi

RELEASE_JSON="${TMP_ROOT}/release.json"
curl --fail --silent --show-error --location "${HEADER_ARGS[@]}" "$RELEASE_URL" -o "$RELEASE_JSON"

PY_OUT="${TMP_ROOT}/asset-selection.txt"
python3 - "$RELEASE_JSON" "$TARGET" "$ARTIFACT_KIND" > "$PY_OUT" <<'PY'
import json
import sys

release_json, target, kind = sys.argv[1:4]
with open(release_json, "r", encoding="utf-8") as f:
    data = json.load(f)
assets = data.get("assets", [])

bundle = None
checksums = None
for a in assets:
    name = str(a.get("name", ""))
    lower = name.lower()
    if lower in ("sha256sums", "sha256sums.txt"):
        checksums = a.get("browser_download_url", "")
    if target in lower and kind in lower and (lower.endswith(".tar.gz") or lower.endswith(".tgz")):
        bundle = (name, a.get("browser_download_url", ""))

if not bundle:
    raise SystemExit(f"missing {kind} bundle for target {target}")
if not checksums:
    raise SystemExit("release is missing SHA256SUMS")

print(bundle[0])
print(bundle[1])
print(checksums)
print(data.get("tag_name", ""))
PY

ASSET_NAME="$(sed -n '1p' "$PY_OUT")"
ASSET_URL="$(sed -n '2p' "$PY_OUT")"
SHA_URL="$(sed -n '3p' "$PY_OUT")"
RELEASE_TAG="$(sed -n '4p' "$PY_OUT")"
INSTALL_RELEASE_TAG="${RELEASE_TAG:-$TAG}"

ARCHIVE_PATH="${TMP_ROOT}/${ASSET_NAME}"
SHA_PATH="${TMP_ROOT}/SHA256SUMS"

curl --fail --silent --show-error --location "${HEADER_ARGS[@]}" "$ASSET_URL" -o "$ARCHIVE_PATH"
curl --fail --silent --show-error --location "${HEADER_ARGS[@]}" "$SHA_URL" -o "$SHA_PATH"

EXPECTED_SHA="$(awk -v f="$ASSET_NAME" '$2 == f || $2 == "*"f {print $1; exit}' "$SHA_PATH" | tr '[:upper:]' '[:lower:]')"
if [[ -z "$EXPECTED_SHA" ]]; then
  echo "checksum entry missing for $ASSET_NAME" >&2
  exit 1
fi
ACTUAL_SHA="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print tolower($1)}')"
if [[ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]]; then
  echo "checksum mismatch for $ASSET_NAME" >&2
  echo "expected: $EXPECTED_SHA" >&2
  echo "actual:   $ACTUAL_SHA" >&2
  exit 1
fi

EXTRACT_DIR="${TMP_ROOT}/extract"
mkdir -p "$EXTRACT_DIR"
python3 - "$ARCHIVE_PATH" "$EXTRACT_DIR" <<'PY'
import io
import os
import pathlib
import shutil
import sys
import tarfile

archive_path, extract_dir = sys.argv[1:3]
extract_root = pathlib.Path(extract_dir).resolve()
extract_root.mkdir(parents=True, exist_ok=True)

with tarfile.open(archive_path, mode="r:*") as tf:
    members = tf.getmembers()
    for member in members:
        name = member.name or ""
        rel = pathlib.PurePosixPath(name)
        if rel.is_absolute() or any(part in ("", "..") for part in rel.parts):
            raise SystemExit(f"unsafe archive path: {name!r}")
        if member.issym() or member.islnk() or member.isdev():
            raise SystemExit(f"unsupported archive entry type for {name!r}")

    for member in members:
        rel = pathlib.PurePosixPath(member.name)
        target = extract_root.joinpath(*rel.parts)
        if member.isdir():
            target.mkdir(parents=True, exist_ok=True)
            continue
        if not member.isfile():
            raise SystemExit(f"unsupported archive entry type for {member.name!r}")
        target.parent.mkdir(parents=True, exist_ok=True)
        src = tf.extractfile(member)
        if src is None:
            raise SystemExit(f"failed to extract archive entry: {member.name!r}")
        with src, open(target, "wb") as dst:
            shutil.copyfileobj(src, dst, length=1024 * 1024)
        mode = member.mode & 0o777
        os.chmod(target, mode if mode else 0o644)
PY

PAYLOAD_ROOT=""
if [[ -d "$EXTRACT_DIR/termlm" ]]; then
  PAYLOAD_ROOT="$EXTRACT_DIR/termlm"
else
  PAYLOAD_ROOT="$EXTRACT_DIR"
fi

if [[ ! -x "$PAYLOAD_ROOT/install.sh" ]]; then
  echo "release payload missing install.sh" >&2
  exit 1
fi

patch_payload_installer_compat() {
  local installer="$1"
  python3 - "$installer" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
try:
    original = path.read_text(encoding="utf-8")
except UnicodeDecodeError:
    raise SystemExit(0)

patched = original.replace(
    'split($i,a,\\"=\\"); print a[2]; exit',
    'split($i,a,"="); print a[2]; exit',
)
patched = patched.replace(
    'if [[ "$phase_complete" -eq 1 && "$manifest_chunk_count" =~ ^[0-9]+$ && "$manifest_chunk_count" -gt 0 ]]; then',
    'if [[ "$manifest_chunk_count" =~ ^[0-9]+$ && "$manifest_chunk_count" -gt 0 && -s "$(dirname "$index_manifest_path")/vectors.f16" && -s "$(dirname "$index_manifest_path")/lexicon.bin" && -s "$(dirname "$index_manifest_path")/postings.bin" ]]; then',
)
patched = patched.replace(
    '''  local reindex_mode="delta"
  local reindex_requested=0
  if [[ ! -f "$index_manifest_path" ]]; then
    reindex_mode="full"
  fi
''',
    '''  local reindex_requested=0
''',
)
patched = patched.replace(
    'trigger_reindex_with_timeout "$reindex_mode"',
    'trigger_reindex_with_timeout "delta"',
)
patched = patched.replace(
    '''      if [[ "$persisted_index_ready" -eq 1 ]]; then
        index_ready=1
      fi
''',
    '''      local runtime_index_loaded=0
      if [[ "$chunk_count" =~ ^[0-9]+$ && "$chunk_count" -gt 0 ]]; then
        runtime_index_loaded=1
      fi
      if [[ "$persisted_index_ready" -eq 1 && "$runtime_index_loaded" -eq 1 ]]; then
        index_ready=1
      fi
''',
)
patched = patched.replace(
    '''  start_core_instance() {
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
''',
    '''  start_core_instance() {
    local cfg_path="${1:-}"
    if [[ -n "$cfg_path" ]]; then
      "$termlm_core_bin" --detach --config "$cfg_path" >>"$daemon_log_path" 2>&1 < /dev/null || {
        fail_with_logs "failed to start termlm-core for readiness bootstrap"
      }
    else
      "$termlm_core_bin" --detach >>"$daemon_log_path" 2>&1 < /dev/null || {
        fail_with_logs "failed to start termlm-core for readiness bootstrap"
      }
    fi
    core_pid=""
    sleep 1
  }
''',
)

if patched != original:
    path.write_text(patched, encoding="utf-8")
PY
}

verify_installed_payload() {
  local bin_dir="${TERMLM_INSTALL_BIN_DIR:-$HOME/.local/bin}"
  local share_dir="${TERMLM_INSTALL_SHARE_DIR:-$HOME/.local/share/termlm}"
  if [[ ! -x "$bin_dir/termlm" ]]; then
    echo "installed CLI is missing or not executable: $bin_dir/termlm" >&2
    exit 1
  fi
  if [[ ! -x "$bin_dir/termlm-core" ]]; then
    echo "installed daemon is missing or not executable: $bin_dir/termlm-core" >&2
    exit 1
  fi
  if [[ ! -f "$share_dir/plugins/zsh/termlm.plugin.zsh" ]]; then
    echo "installed zsh plugin is missing: $share_dir/plugins/zsh/termlm.plugin.zsh" >&2
    exit 1
  fi
  "$bin_dir/termlm" --help >/dev/null || {
    echo "installed CLI failed to run: $bin_dir/termlm --help" >&2
    exit 1
  }
  "$bin_dir/termlm-core" --help >/dev/null || {
    echo "installed daemon failed to run: $bin_dir/termlm-core --help" >&2
    exit 1
  }
}

patch_payload_installer_compat "$PAYLOAD_ROOT/install.sh"

echo "Installing ${ASSET_NAME} (${RELEASE_TAG})..."
INSTALL_ENV=(TERMLM_GITHUB_REPO="$REPO")
if [[ -n "$INSTALL_RELEASE_TAG" ]]; then
  INSTALL_ENV+=(TERMLM_RELEASE_TAG="$INSTALL_RELEASE_TAG")
fi

if [[ "$SKIP_MODELS" == "1" ]]; then
  env "${INSTALL_ENV[@]}" "$PAYLOAD_ROOT/install.sh" --skip-models
else
  env "${INSTALL_ENV[@]}" "$PAYLOAD_ROOT/install.sh"
fi

verify_installed_payload

echo "Done."

echo "Next steps:"
echo "  1) Ensure ~/.local/bin is in PATH"
echo "  2) Run: termlm init zsh"
echo "  3) Reload zsh with: exec zsh -l"
