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
API_BASE="https://api.github.com/repos/${REPO}/releases"
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

INSTALL_ARGS=()
if [[ "$SKIP_MODELS" == "1" ]]; then
  INSTALL_ARGS+=(--skip-models)
fi

echo "Installing ${ASSET_NAME} (${RELEASE_TAG})..."
"$PAYLOAD_ROOT/install.sh" "${INSTALL_ARGS[@]}"

echo "Done."

echo "Next steps:"
echo "  1) Ensure ~/.local/bin is in PATH"
echo "  2) Run: termlm init zsh"
echo "  3) Open a new zsh session"
