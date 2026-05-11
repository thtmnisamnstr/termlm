#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termlm-release-smoke.XXXXXX")"
SERVER_PID=""

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
  rm -rf "${TMP_ROOT}"
}
trap cleanup EXIT

DIST_DIR="${TMP_ROOT}/dist"
INSTALL_ROOT="${TMP_ROOT}/install-root"
MODEL_SOURCE_DIR="${TMP_ROOT}/model-source"
mkdir -p "${DIST_DIR}" "${INSTALL_ROOT}" "${MODEL_SOURCE_DIR}"

fail() {
  echo "release smoke failure: $*" >&2
  exit 1
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

assemble_sha256sums() {
  (
    cd "${DIST_DIR}" && \
      find . -maxdepth 1 -type f -name '*.sha256' -print | sort | while IFS= read -r checksum_file; do
        cat "${checksum_file}"
      done > SHA256SUMS
  )
}

run_mock_server() {
  local port="$1"
  local server_root="$2"
  python3 -m http.server "${port}" --bind 127.0.0.1 --directory "${server_root}" >/dev/null 2>&1 &
  SERVER_PID=$!
  sleep 1
  kill -0 "${SERVER_PID}" >/dev/null 2>&1 || fail "mock release server failed to start"
}

run_top_level_install_rehearsal() {
  local release_tag="v0.0.0-installer-live"
  local server_root="${TMP_ROOT}/installer-server"
  local port
  port="$(pick_free_port)"

  local api_dir="${server_root}/repos/example/termlm/releases"
  local download_dir="${server_root}/downloads/${release_tag}"
  local github_download_dir="${server_root}/github/example/termlm/releases/download/${release_tag}"
  mkdir -p "${api_dir}" "${download_dir}" "${github_download_dir}"
  cp "${DIST_DIR}/"* "${download_dir}/"
  cp "${DIST_DIR}/"* "${github_download_dir}/"

  local with_models_asset
  with_models_asset="$(basename "${WITH_MODELS_ARCHIVE}")"
  cat > "${api_dir}/latest" <<EOF
{
  "tag_name": "${release_tag}",
  "assets": [
    {
      "name": "${with_models_asset}",
      "browser_download_url": "http://127.0.0.1:${port}/downloads/${release_tag}/${with_models_asset}"
    },
    {
      "name": "SHA256SUMS",
      "browser_download_url": "http://127.0.0.1:${port}/downloads/${release_tag}/SHA256SUMS"
    }
  ]
}
EOF

  run_mock_server "${port}" "${server_root}"

  local top_install_root="${TMP_ROOT}/top-level-install-root"
  local top_home="${top_install_root}/home"
  local top_bin="${top_home}/.local/bin"
  local top_share="${top_home}/.local/share/termlm"
  local top_models="${top_share}/models"
  mkdir -p "${top_home}"

  HOME="${top_home}" \
    TERMLM_INSTALL_BIN_DIR="${top_bin}" \
    TERMLM_INSTALL_SHARE_DIR="${top_share}" \
    TERMLM_MODELS_DIR="${top_models}" \
    TERMLM_GITHUB_API_BASE="http://127.0.0.1:${port}/repos" \
    TERMLM_GITHUB_DOWNLOAD_BASE="http://127.0.0.1:${port}/github" \
    TERMLM_INSTALL_WAIT_FOR_READY=0 \
    "${ROOT_DIR}/scripts/install.sh" --repo example/termlm >/dev/null

  [[ -x "${top_bin}/termlm" ]] || fail "top-level installer did not install termlm"
  [[ -x "${top_bin}/termlm-core" ]] || fail "top-level installer did not install termlm-core"
  [[ -f "${top_share}/plugins/zsh/termlm.plugin.zsh" ]] || fail "top-level installer did not install zsh plugin"
  [[ -f "${top_models}/gemma-4-E4B-it-Q4_K_M.gguf" ]] || fail "top-level installer did not assemble E4B model"
  [[ -f "${top_models}/bge-small-en-v1.5.Q4_K_M.gguf" ]] || fail "top-level installer did not assemble embedding model"
  [[ "$(<"${top_models}/gemma-4-E4B-it-Q4_K_M.gguf")" == "fake-e4b-model" ]] || fail "E4B model content mismatch"
  [[ "$(<"${top_models}/bge-small-en-v1.5.Q4_K_M.gguf")" == "fake-embed-model" ]] || fail "embedding model content mismatch"
}

printf 'fake-e4b-model\n' > "${MODEL_SOURCE_DIR}/gemma-4-E4B-it-Q4_K_M.gguf"
printf 'fake-embed-model\n' > "${MODEL_SOURCE_DIR}/bge-small-en-v1.5.Q4_K_M.gguf"
E4B_SHA="$(shasum -a 256 "${MODEL_SOURCE_DIR}/gemma-4-E4B-it-Q4_K_M.gguf" | awk '{print $1}')"
EMBED_SHA="$(shasum -a 256 "${MODEL_SOURCE_DIR}/bge-small-en-v1.5.Q4_K_M.gguf" | awk '{print $1}')"

pushd "${ROOT_DIR}" >/dev/null
cargo build -p termlm-client -p termlm-core --release --locked >/dev/null
scripts/release/package_release.sh \
  --mode no-models \
  --version v0.0.0-smoke \
  --target darwin-arm64 \
  --out "${DIST_DIR}" >/dev/null
TERMLM_RELEASE_MODEL_DIR="${MODEL_SOURCE_DIR}" \
TERMLM_RELEASE_MODEL_E4B_SHA256="${E4B_SHA}" \
TERMLM_RELEASE_EMBED_SHA256="${EMBED_SHA}" \
scripts/release/package_release.sh \
  --mode with-models \
  --version v0.0.0-smoke \
  --target darwin-arm64 \
  --out "${DIST_DIR}" >/dev/null
popd >/dev/null

NO_MODELS_ARCHIVE="${DIST_DIR}/termlm-v0.0.0-smoke-darwin-arm64-no-models.tar.gz"
WITH_MODELS_ARCHIVE="${DIST_DIR}/termlm-v0.0.0-smoke-darwin-arm64-with-models.tar.gz"
[[ -f "${NO_MODELS_ARCHIVE}" ]]
[[ -f "${WITH_MODELS_ARCHIVE}" ]]

ORIG_SHA_NO_MODELS="$(shasum -a 256 "${NO_MODELS_ARCHIVE}" | awk '{print tolower($1)}')"
ORIG_SHA_WITH_MODELS="$(shasum -a 256 "${WITH_MODELS_ARCHIVE}" | awk '{print tolower($1)}')"

pushd "${ROOT_DIR}" >/dev/null
scripts/release/sign_and_notarize.sh --dist "${DIST_DIR}" --identity "-" >/dev/null
popd >/dev/null

NEW_SHA_NO_MODELS="$(shasum -a 256 "${NO_MODELS_ARCHIVE}" | awk '{print tolower($1)}')"
NEW_SHA_WITH_MODELS="$(shasum -a 256 "${WITH_MODELS_ARCHIVE}" | awk '{print tolower($1)}')"
[[ -n "${NEW_SHA_NO_MODELS}" ]]
[[ -n "${NEW_SHA_WITH_MODELS}" ]]
[[ "${ORIG_SHA_NO_MODELS}" != "${NEW_SHA_NO_MODELS}" ]]
[[ "${ORIG_SHA_WITH_MODELS}" != "${NEW_SHA_WITH_MODELS}" ]]
[[ -f "${NO_MODELS_ARCHIVE}.sha256" ]]
[[ -f "${WITH_MODELS_ARCHIVE}.sha256" ]]
grep -qi "^${NEW_SHA_NO_MODELS}[[:space:]]" "${NO_MODELS_ARCHIVE}.sha256"
grep -qi "^${NEW_SHA_WITH_MODELS}[[:space:]]" "${WITH_MODELS_ARCHIVE}.sha256"
assemble_sha256sums

python3 - "${NO_MODELS_ARCHIVE}" "no-models" "false" <<'PY'
import json
import sys
import tarfile

archive, expected_kind, expected_includes = sys.argv[1:4]
with tarfile.open(archive, "r:*") as tf:
    payload = json.load(tf.extractfile("termlm/bundle-manifest.json"))
assert payload["artifact_kind"] == expected_kind, payload
assert bool(payload["includes_models"]) == (expected_includes == "true"), payload
PY

python3 - "${WITH_MODELS_ARCHIVE}" "with-models" "true" <<'PY'
import json
import sys
import tarfile

archive, expected_kind, expected_includes = sys.argv[1:4]
with tarfile.open(archive, "r:*") as tf:
    payload = json.load(tf.extractfile("termlm/bundle-manifest.json"))
    models_manifest = json.load(tf.extractfile("termlm/models/models-manifest.json"))
assert payload["artifact_kind"] == expected_kind, payload
assert bool(payload["includes_models"]) == (expected_includes == "true"), payload
assert len(models_manifest.get("models", [])) >= 2, models_manifest
PY

if tar -tzf "${NO_MODELS_ARCHIVE}" | grep -q '^termlm/models/models-manifest.json$'; then
  echo "no-models archive unexpectedly includes model manifest" >&2
  exit 1
fi
tar -tzf "${WITH_MODELS_ARCHIVE}" | grep -q '^termlm/models/models-manifest.json$'

chunk_assets_count="$(find "${DIST_DIR}" -maxdepth 1 -type f -name 'termlm-v0.0.0-smoke-darwin-arm64-model-*.part-*' | wc -l | tr -d '[:space:]')"
if [[ "${chunk_assets_count}" -lt 2 ]]; then
  echo "expected split model chunk assets for with-models release" >&2
  exit 1
fi

EXTRACT_DIR="${TMP_ROOT}/extract"
mkdir -p "${EXTRACT_DIR}"
tar -xzf "${NO_MODELS_ARCHIVE}" -C "${EXTRACT_DIR}"

PAYLOAD_ROOT="${EXTRACT_DIR}/termlm"
[[ -x "${PAYLOAD_ROOT}/install.sh" ]]

TERMLM_INSTALL_BIN_DIR="${INSTALL_ROOT}/bin" \
TERMLM_INSTALL_SHARE_DIR="${INSTALL_ROOT}/share" \
"${PAYLOAD_ROOT}/install.sh" --skip-models >/dev/null

[[ -x "${INSTALL_ROOT}/bin/termlm" ]]
[[ -x "${INSTALL_ROOT}/bin/termlm-core" ]]
[[ -x "${INSTALL_ROOT}/bin/termlm-client" ]]
[[ -f "${INSTALL_ROOT}/share/plugins/zsh/termlm.plugin.zsh" ]]

"${INSTALL_ROOT}/bin/termlm" --help >/dev/null

run_top_level_install_rehearsal

echo "release smoke checks passed"
