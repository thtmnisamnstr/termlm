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

echo "Installed termlm binaries to: $BIN_DIR"
echo "Installed zsh plugin to:      $SHARE_DIR/plugins/zsh"
if [[ $SKIP_MODELS -eq 0 && -d "$ROOT_DIR/models" ]]; then
  echo "Installed models to:          $MODELS_DIR"
fi
