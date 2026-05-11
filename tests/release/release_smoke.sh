#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termlm-release-smoke.XXXXXX")"
trap 'rm -rf "${TMP_ROOT}"' EXIT

DIST_DIR="${TMP_ROOT}/dist"
INSTALL_ROOT="${TMP_ROOT}/install-root"
MODEL_SOURCE_DIR="${TMP_ROOT}/model-source"
mkdir -p "${DIST_DIR}" "${INSTALL_ROOT}" "${MODEL_SOURCE_DIR}"

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

echo "release smoke checks passed"
