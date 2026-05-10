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
  --no-models          Use the no-models bundle (default bundle is with-models)
  --skip-models        Pass --skip-models to bundle install.sh
  -h, --help           Show help

Environment:
  TERMLM_GITHUB_TOKEN  Optional GitHub token for higher API rate limits
  GITHUB_TOKEN         Fallback token env var
USAGE
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
tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR"

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
if [[ "$ARTIFACT_KIND" == "no-models" ]]; then
  INSTALL_ARGS+=(--skip-models)
fi

echo "Installing ${ASSET_NAME} (${RELEASE_TAG})..."
"$PAYLOAD_ROOT/install.sh" "${INSTALL_ARGS[@]}"

echo "Done."

echo "Next steps:"
echo "  1) Ensure ~/.local/bin is in PATH"
echo "  2) Run: termlm init zsh"
echo "  3) Open a new zsh session"
