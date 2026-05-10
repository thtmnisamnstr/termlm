#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termlm-release-smoke.XXXXXX")"
trap 'rm -rf "${TMP_ROOT}"' EXIT

DIST_DIR="${TMP_ROOT}/dist"
INSTALL_ROOT="${TMP_ROOT}/install-root"
mkdir -p "${DIST_DIR}" "${INSTALL_ROOT}"

pushd "${ROOT_DIR}" >/dev/null
cargo build -p termlm-client -p termlm-core --release --locked >/dev/null
scripts/release/package_release.sh \
  --mode no-models \
  --version v0.0.0-smoke \
  --target darwin-arm64 \
  --out "${DIST_DIR}" >/dev/null
popd >/dev/null

ARCHIVE="${DIST_DIR}/termlm-v0.0.0-smoke-darwin-arm64-no-models.tar.gz"
[[ -f "${ARCHIVE}" ]]
ORIG_SHA="$(shasum -a 256 "${ARCHIVE}" | awk '{print tolower($1)}')"

pushd "${ROOT_DIR}" >/dev/null
scripts/release/sign_and_notarize.sh --dist "${DIST_DIR}" --identity "-" >/dev/null
popd >/dev/null

NEW_SHA="$(shasum -a 256 "${ARCHIVE}" | awk '{print tolower($1)}')"
[[ -n "${NEW_SHA}" ]]
[[ "${ORIG_SHA}" != "${NEW_SHA}" ]]
[[ -f "${ARCHIVE}.sha256" ]]
grep -qi "^${NEW_SHA}[[:space:]]" "${ARCHIVE}.sha256"

EXTRACT_DIR="${TMP_ROOT}/extract"
mkdir -p "${EXTRACT_DIR}"
tar -xzf "${ARCHIVE}" -C "${EXTRACT_DIR}"

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
