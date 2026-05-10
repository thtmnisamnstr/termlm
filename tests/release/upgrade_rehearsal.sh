#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

OLD_VERSION="${TERMLM_REHEARSAL_OLD_VERSION:-v0.0.0-rehearsal-old}"
NEW_VERSION="${TERMLM_REHEARSAL_NEW_VERSION:-v0.0.0-rehearsal-new}"
TARGET="${TERMLM_REHEARSAL_TARGET:-darwin-arm64}"
ARTIFACT_DIR="${TERMLM_REHEARSAL_ARTIFACT_DIR:-}"

if [[ -n "${ARTIFACT_DIR}" ]]; then
  TMP_ROOT="${ARTIFACT_DIR}"
  CLEANUP_TMP_ROOT=0
  mkdir -p "${TMP_ROOT}"
else
  TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termlm-upgrade-rehearsal.XXXXXX")"
  CLEANUP_TMP_ROOT=1
fi

DIST_DIR="${TMP_ROOT}/dist"
SERVER_ROOT="${TMP_ROOT}/server"
MODEL_SOURCE_DIR="${TMP_ROOT}/model-source"
HOME_DIR="${TMP_ROOT}/home"
BIN_DIR="${HOME_DIR}/.local/bin"
SHARE_DIR="${HOME_DIR}/.local/share/termlm"
MODELS_DIR="${SHARE_DIR}/models"

SERVER_PID=""

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
  if [[ "${CLEANUP_TMP_ROOT}" == "1" ]]; then
    rm -rf "${TMP_ROOT}"
  fi
}
trap cleanup EXIT

log() {
  printf '[upgrade-rehearsal] %s\n' "$*"
}

fail() {
  printf '[upgrade-rehearsal] failure: %s\n' "$*" >&2
  exit 1
}

require_file() {
  local path="$1"
  [[ -f "${path}" ]] || fail "missing expected file: ${path}"
}

pick_free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

make_fake_models() {
  mkdir -p "${MODEL_SOURCE_DIR}"
  printf 'fake-e4b-model\n' > "${MODEL_SOURCE_DIR}/gemma-4-E4B-it-Q4_K_M.gguf"
  printf 'fake-embed-model\n' > "${MODEL_SOURCE_DIR}/bge-small-en-v1.5.Q4_K_M.gguf"
}

build_and_package() {
  mkdir -p "${DIST_DIR}"
  local e4b_sha
  local embed_sha
  e4b_sha="$(shasum -a 256 "${MODEL_SOURCE_DIR}/gemma-4-E4B-it-Q4_K_M.gguf" | awk '{print $1}')"
  embed_sha="$(shasum -a 256 "${MODEL_SOURCE_DIR}/bge-small-en-v1.5.Q4_K_M.gguf" | awk '{print $1}')"
  log "building release binaries"
  (
    cd "${ROOT_DIR}" && \
      cargo build -p termlm-client -p termlm-core --release --locked
  )

  log "packaging with-models bundle for initial install (${OLD_VERSION})"
  (
    cd "${ROOT_DIR}" && \
      TERMLM_RELEASE_MODEL_DIR="${MODEL_SOURCE_DIR}" \
      TERMLM_RELEASE_MODEL_E4B_SHA256="${e4b_sha}" \
      TERMLM_RELEASE_EMBED_SHA256="${embed_sha}" \
      scripts/release/package_release.sh \
        --mode with-models \
        --version "${OLD_VERSION}" \
        --target "${TARGET}" \
        --out "${DIST_DIR}"
  )

  log "packaging no-models bundle for upgrade (${NEW_VERSION})"
  (
    cd "${ROOT_DIR}" && \
      scripts/release/package_release.sh \
        --mode no-models \
        --version "${NEW_VERSION}" \
        --target "${TARGET}" \
        --out "${DIST_DIR}"
  )

  (
    cd "${DIST_DIR}" && \
      find . -maxdepth 1 -type f -name '*.sha256' -print | sort | while IFS= read -r file; do
        cat "${file}"
      done > SHA256SUMS
  )
}

install_initial_bundle() {
  local bundle="${DIST_DIR}/termlm-${OLD_VERSION}-${TARGET}-with-models.tar.gz"
  require_file "${bundle}"
  mkdir -p "${TMP_ROOT}/install-old"
  tar -xzf "${bundle}" -C "${TMP_ROOT}/install-old"
  HOME="${HOME_DIR}" \
    TERMLM_INSTALL_BIN_DIR="${BIN_DIR}" \
    TERMLM_INSTALL_SHARE_DIR="${SHARE_DIR}" \
    "${TMP_ROOT}/install-old/termlm/install.sh" --skip-models

  mkdir -p "${MODELS_DIR}"
  printf 'preserve-this-model\n' > "${MODELS_DIR}/sentinel.gguf"
}

write_mock_release_api() {
  local port="$1"
  local api_dir="${SERVER_ROOT}/repos/example/termlm/releases"
  local dl_dir="${SERVER_ROOT}/downloads"
  mkdir -p "${api_dir}" "${dl_dir}"

  local no_models_asset="termlm-${NEW_VERSION}-${TARGET}-no-models.tar.gz"
  require_file "${DIST_DIR}/${no_models_asset}"
  require_file "${DIST_DIR}/SHA256SUMS"
  cp "${DIST_DIR}/${no_models_asset}" "${dl_dir}/${no_models_asset}"
  cp "${DIST_DIR}/SHA256SUMS" "${dl_dir}/SHA256SUMS"

  cat > "${api_dir}/latest" <<EOF
{
  "tag_name": "${NEW_VERSION}",
  "assets": [
    {
      "name": "${no_models_asset}",
      "browser_download_url": "http://127.0.0.1:${port}/downloads/${no_models_asset}"
    },
    {
      "name": "SHA256SUMS",
      "browser_download_url": "http://127.0.0.1:${port}/downloads/SHA256SUMS"
    }
  ]
}
EOF
}

run_mock_server() {
  local port="$1"
  python3 -m http.server "${port}" --bind 127.0.0.1 --directory "${SERVER_ROOT}" >/dev/null 2>&1 &
  SERVER_PID=$!
  sleep 1
  kill -0 "${SERVER_PID}" >/dev/null 2>&1 || fail "mock release server failed to start"
}

verify_receipt() {
  local receipt="${SHARE_DIR}/install-receipt.json"
  require_file "${receipt}"
  python3 - "${receipt}" "${NEW_VERSION}" <<'PY'
import json
import sys
receipt_path, expected_tag = sys.argv[1], sys.argv[2]
with open(receipt_path, "r", encoding="utf-8") as f:
    payload = json.load(f)
assert payload.get("release_tag") == expected_tag, payload
assert payload.get("artifact_kind") == "no-models", payload
assert payload.get("includes_models") is False, payload
PY
}

verify_temp_cleanup() {
  local before_file="$1"
  local after_file="$2"
  python3 - "${before_file}" "${after_file}" <<'PY'
import pathlib
import sys
before = set(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines())
after = set(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8").splitlines())
new = sorted(x for x in (after - before) if x)
if new:
    raise SystemExit(f"unexpected leftover upgrade temp directories: {new}")
PY
}

main() {
  log "working directory: ${TMP_ROOT}"
  make_fake_models
  build_and_package
  install_initial_bundle

  local port
  port="$(pick_free_port)"
  write_mock_release_api "${port}"
  run_mock_server "${port}"

  local pre_tmp="${TMP_ROOT}/tmp-before.txt"
  local post_tmp="${TMP_ROOT}/tmp-after.txt"
  find "${TMPDIR:-/tmp}" -maxdepth 1 -type d -name 'termlm-upgrade-*' -print | sort > "${pre_tmp}"

  log "running upgrade command against local mock release API"
  HOME="${HOME_DIR}" \
    PATH="${BIN_DIR}:${PATH}" \
    TERMLM_GITHUB_API_BASE="http://127.0.0.1:${port}/repos" \
    "${BIN_DIR}/termlm" upgrade --repo example/termlm

  find "${TMPDIR:-/tmp}" -maxdepth 1 -type d -name 'termlm-upgrade-*' -print | sort > "${post_tmp}"

  verify_receipt
  require_file "${BIN_DIR}/termlm"
  require_file "${BIN_DIR}/termlm-core"
  require_file "${SHARE_DIR}/plugins/zsh/termlm.plugin.zsh"
  require_file "${MODELS_DIR}/sentinel.gguf"
  [[ "$(cat "${MODELS_DIR}/sentinel.gguf")" == "preserve-this-model" ]] \
    || fail "sentinel model content changed during upgrade"
  verify_temp_cleanup "${pre_tmp}" "${post_tmp}"

  cat > "${TMP_ROOT}/rehearsal-summary.json" <<EOF
{
  "old_version": "${OLD_VERSION}",
  "new_version": "${NEW_VERSION}",
  "target": "${TARGET}",
  "release_api_repo": "example/termlm",
  "result": "passed",
  "kept_workdir": $([[ "${CLEANUP_TMP_ROOT}" == "0" ]] && echo "true" || echo "false")
}
EOF
  log "upgrade rehearsal passed"
}

main "$@"
