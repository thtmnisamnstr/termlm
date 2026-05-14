#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

ARTIFACTS_DIR="${ARTIFACTS_DIR:-/tmp/termlm-local-ci-$(date +%Y%m%d-%H%M%S)}"
RELEASE_VERSION="${RELEASE_VERSION:-v0.0.0-local}"
SOAK_ITERS="${SOAK_ITERS:-30}"
SOAK_DURATION_SECS="${SOAK_DURATION_SECS:-0}"
PERF_SUITE="${PERF_SUITE:-${REPO_ROOT}/tests/fixtures/termlm-perf-suite.toml}"
PERF_GATES="${PERF_GATES:-${REPO_ROOT}/tests/perf/perf-gates.toml}"
REAL_RUNTIME_GATES="${REAL_RUNTIME_GATES:-${REPO_ROOT}/tests/perf/real-runtime-gates.toml}"
PERF_REINDEX_TIMEOUT_SECS="${PERF_REINDEX_TIMEOUT_SECS:-300}"
RUN_MACOS_LANE=1
RUN_LINUX_SMOKE=1
RUN_SECURITY=1
RUN_RELIABILITY=1
RUN_HARDWARE_MATRIX=1
RUN_OLLAMA_PARITY=1
RUN_ACCURACY=1
RUN_ZSH_USABILITY=1
RUN_RELEASE_PACKAGING=1
RUN_RELEASE_REHEARSAL=1
ZSH_USABILITY_LEVEL="${ZSH_USABILITY_LEVEL:-smoke}"
ACCURACY_LEVEL="${ACCURACY_LEVEL:-commit}"

usage() {
  cat <<'EOF'
Usage: scripts/ci/run_local_ci.sh [options]

Runs local equivalents of GitHub workflows:
  - .github/workflows/ci.yml
  - .github/workflows/extended-validation.yml
  - .github/workflows/release.yml (no signing/notary/upload)
  - .github/workflows/ollama-parity.yml
  - .github/workflows/reliability.yml
  - .github/workflows/security.yml

Options:
  --artifacts-dir <path>      Output directory for local artifacts
  --release-version <version> Version label used for local release bundles
  --soak-iters <n>            Reliability soak iterations (default: 30)
  --soak-duration-secs <n>    Reliability soak duration target in seconds (overrides iteration target when >0)
  --perf-suite <path>         Suite file used by hardware matrix lanes
  --perf-gates <path>         Perf gate file used by hardware matrix lanes
  --real-runtime-gates <path> Perf gate file used by local real-runtime evidence lane
  --perf-reindex-timeout-secs <n>
                              Reindex completion timeout used by hardware matrix lanes
  --quick                     Skip heavy lanes (reliability, security, hardware matrix, ollama parity, release packaging, release rehearsal)
  --skip-macos-lane           Skip macOS CI lane equivalent
  --skip-linux-smoke          Skip linux-smoke lane equivalent
  --skip-security             Skip security lane equivalent
  --skip-reliability          Skip reliability lane equivalent
  --skip-hardware-matrix      Skip local_stub_all hardware matrix case
  --skip-ollama-parity        Skip ollama parity lane
  --skip-accuracy             Skip response accuracy gate
  --accuracy-level <n>        response accuracy level: commit, full, or release
  --skip-zsh-usability        Skip real zsh plugin usability lane
  --zsh-usability-level <n>   zsh usability level: smoke, release, or full
  --skip-release-packaging    Skip release packaging lane
  --skip-release-rehearsal    Skip local release upgrade rehearsal lane
  -h, --help                  Show help
EOF
}

log() {
  printf '\n[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

run_step() {
  local label="$1"
  shift
  log "BEGIN ${label}"
  (cd "${REPO_ROOT}" && "$@")
  log "PASS  ${label}"
}

prepare_tools() {
  run_step "docs-links" python3 scripts/ci/check_docs_links.py
  run_step "shell-lint" bash scripts/ci/lint_shell.sh
  run_step "fmt-check" cargo fmt --check
  run_step "clippy" cargo clippy --workspace --all-targets --locked -- -D warnings
}

macos_lane() {
  prepare_tools
  run_step "tests-release" cargo test --workspace --all-targets --release --locked
  run_step "adapter-contract" bash tests/adapter-contract/zsh_adapter_contract.sh
  run_step "compat-macos-profile" bash tests/compatibility/macos_profile.sh
  run_step "compat-terminal-matrix" bash tests/compatibility/terminal_matrix.sh
  run_step "compat-ssh-env" bash tests/compatibility/ssh_env_smoke.sh
  run_step "compat-plugin-manager" bash tests/compatibility/plugin_manager_matrix.sh
  run_step "release-smoke" bash tests/release/release_smoke.sh
}

zsh_usability_lane() {
  run_step "zsh-usability-${ZSH_USABILITY_LEVEL}" env \
    TERMLM_ZSH_USABILITY_LEVEL="${ZSH_USABILITY_LEVEL}" \
    TERMLM_ZSH_USABILITY_PROFILE=debug \
    bash tests/usability/zsh_user_journey.sh
}

accuracy_lane() {
  run_step "accuracy-${ACCURACY_LEVEL}" env \
    TERMLM_ACCURACY_LEVEL="${ACCURACY_LEVEL}" \
    TERMLM_ACCURACY_ARTIFACTS_DIR="${ARTIFACTS_DIR}/accuracy" \
    bash scripts/ci/run_accuracy_gate.sh \
      --level "${ACCURACY_LEVEL}" \
      --artifacts-dir "${ARTIFACTS_DIR}/accuracy"
}

hardware_matrix_lane() {
  local out_dir="${ARTIFACTS_DIR}/hardware-matrix-local-stub"
  mkdir -p "${out_dir}"
  run_step "hardware-matrix-local_stub_all" env \
    TERMLM_PERF_MATRIX_SUITE="${PERF_SUITE}" \
    TERMLM_PERF_MATRIX_GATES="${PERF_GATES}" \
    TERMLM_PERF_MATRIX_REAL_GATES="${REAL_RUNTIME_GATES}" \
    TERMLM_PERF_MATRIX_REINDEX_TIMEOUT_SECS="${PERF_REINDEX_TIMEOUT_SECS}" \
    TERMLM_PERF_MATRIX_CASES=local_stub_all \
    bash tests/perf/hardware_matrix.sh "${out_dir}"
}

real_runtime_local_lane() {
  local model_path="${HOME}/.local/share/termlm/models/gemma-4-E4B-it-Q4_K_M.gguf"
  local results_out="${ARTIFACTS_DIR}/real-runtime-local.json"
  if [[ -f "${model_path}" ]]; then
    run_step "real-runtime-local-e2e" env \
      TERMLM_E2E_REAL=1 \
      cargo run -p termlm-test --release --locked -- \
      --suite tests/fixtures/termlm-test-suite.toml \
      --mode local-integration \
      --provider local \
      --perf-gates "${REAL_RUNTIME_GATES}" \
      --results-out "${results_out}"
  else
    log "SKIP real-runtime-local-e2e (missing ${model_path})"
  fi
}

linux_smoke_lane() {
  if ! cargo machete --version >/dev/null 2>&1; then
    run_step "install-cargo-machete" cargo install cargo-machete --locked
  fi
  run_step "cargo-machete" cargo machete --with-metadata --skip-target-dir
  prepare_tools
  run_step "tests-dev" cargo test --workspace --all-targets --locked
}

collect_security_sboms() {
  local sbom_dir="${ARTIFACTS_DIR}/security-sbom"
  mkdir -p "${sbom_dir}"
  while IFS= read -r file; do
    local rel="${file#"${REPO_ROOT}"/}"
    local safe_name
    safe_name="$(echo "${rel}" | tr '/' '__')"
    cp "${file}" "${sbom_dir}/${safe_name}"
    rm -f "${file}"
  done < <(find "${REPO_ROOT}/crates" -maxdepth 2 -type f -name 'termlm-sbom*.json' | sort)
}

security_lane() {
  if ! cargo audit --version >/dev/null 2>&1; then
    run_step "install-cargo-audit" cargo install cargo-audit --locked
  fi
  if ! cargo cyclonedx --version >/dev/null 2>&1; then
    run_step "install-cargo-cyclonedx" cargo install cargo-cyclonedx --locked
  fi
  run_step "cargo-audit" cargo audit --deny warnings
  run_step "cargo-cyclonedx" cargo cyclonedx --format json --override-filename termlm-sbom
  collect_security_sboms
}

reliability_lane() {
  local env_args=("TERMLM_SOAK_ITERS=${SOAK_ITERS}")
  if [[ "${SOAK_DURATION_SECS}" =~ ^[0-9]+$ ]] && (( SOAK_DURATION_SECS > 0 )); then
    env_args+=("TERMLM_SOAK_DURATION_SECS=${SOAK_DURATION_SECS}")
  fi
  run_step "reliability-drills" env "${env_args[@]}" \
    bash tests/reliability/reliability_drills.sh
}

ollama_parity_lane() {
  local out_dir="${ARTIFACTS_DIR}/hardware-matrix-ollama-parity"
  mkdir -p "${out_dir}"
  run_step "hardware-matrix-ollama-integration" env \
    TERMLM_PERF_MATRIX_SUITE="${PERF_SUITE}" \
    TERMLM_PERF_MATRIX_GATES="${PERF_GATES}" \
    TERMLM_PERF_MATRIX_REAL_GATES="${REAL_RUNTIME_GATES}" \
    TERMLM_PERF_MATRIX_REINDEX_TIMEOUT_SECS="${PERF_REINDEX_TIMEOUT_SECS}" \
    TERMLM_PERF_MATRIX_CASES=ollama_integration \
    TERMLM_PERF_MATRIX_REQUIRE_OLLAMA=1 \
    TERMLM_TEST_OLLAMA_MODEL=gemma3:1b \
    bash tests/perf/hardware_matrix.sh "${out_dir}"

  local manifest="${out_dir}/manifest.json"
  jq -e '
    .summary.failed == 0
    and .summary.skipped == 0
    and .summary.passed == 1
    and .summary.total == 1
    and (.cases | length) == 1
    and .cases[0].name == "ollama_integration"
    and .cases[0].status == "passed"
  ' "${manifest}" >/dev/null
  log "PASS  ollama-parity-manifest-contract"
}

release_packaging_lane() {
  local dist_dir="${ARTIFACTS_DIR}/release-dist"
  mkdir -p "${dist_dir}"
  run_step "release-build" cargo build -p termlm-client -p termlm-core --release --locked
  if [[ "${RUN_ZSH_USABILITY}" -eq 1 ]]; then
    run_step "zsh-usability-release-binary" env \
      TERMLM_ZSH_USABILITY_LEVEL="${ZSH_USABILITY_LEVEL}" \
      TERMLM_ZSH_USABILITY_PROFILE=release \
      TERMLM_ZSH_USABILITY_SKIP_BUILD=1 \
      bash tests/usability/zsh_user_journey.sh
  fi
  run_step "package-no-models" \
    scripts/release/package_release.sh \
    --mode no-models \
    --version "${RELEASE_VERSION}" \
    --target darwin-arm64 \
    --out "${dist_dir}"
  run_step "package-with-models" \
    scripts/release/package_release.sh \
    --mode with-models \
    --version "${RELEASE_VERSION}" \
    --target darwin-arm64 \
    --out "${dist_dir}"

  (
    cd "${dist_dir}"
    find . -maxdepth 1 -type f -name '*.sha256' -print | sort | while IFS= read -r file; do
      cat "${file}"
    done > SHA256SUMS
  )
  log "PASS  release-checksums (${dist_dir}/SHA256SUMS)"
}

release_rehearsal_lane() {
  local out_dir="${ARTIFACTS_DIR}/release-upgrade-rehearsal"
  mkdir -p "${out_dir}"
  run_step "release-upgrade-rehearsal" \
    env TERMLM_REHEARSAL_ARTIFACT_DIR="${out_dir}" \
    bash tests/release/upgrade_rehearsal.sh
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --artifacts-dir)
      ARTIFACTS_DIR="$2"
      shift 2
      ;;
    --release-version)
      RELEASE_VERSION="$2"
      shift 2
      ;;
    --soak-iters)
      SOAK_ITERS="$2"
      shift 2
      ;;
    --soak-duration-secs)
      SOAK_DURATION_SECS="$2"
      shift 2
      ;;
    --perf-suite)
      PERF_SUITE="$2"
      shift 2
      ;;
    --perf-gates)
      PERF_GATES="$2"
      shift 2
      ;;
    --real-runtime-gates)
      REAL_RUNTIME_GATES="$2"
      shift 2
      ;;
    --perf-reindex-timeout-secs)
      PERF_REINDEX_TIMEOUT_SECS="$2"
      shift 2
      ;;
    --quick)
      RUN_SECURITY=0
      RUN_RELIABILITY=0
      RUN_HARDWARE_MATRIX=0
      RUN_OLLAMA_PARITY=0
      RUN_ZSH_USABILITY=0
      RUN_RELEASE_PACKAGING=0
      RUN_RELEASE_REHEARSAL=0
      shift
      ;;
    --skip-macos-lane)
      RUN_MACOS_LANE=0
      shift
      ;;
    --skip-linux-smoke)
      RUN_LINUX_SMOKE=0
      shift
      ;;
    --skip-security)
      RUN_SECURITY=0
      shift
      ;;
    --skip-reliability)
      RUN_RELIABILITY=0
      shift
      ;;
    --skip-hardware-matrix)
      RUN_HARDWARE_MATRIX=0
      shift
      ;;
    --skip-ollama-parity)
      RUN_OLLAMA_PARITY=0
      shift
      ;;
    --skip-accuracy)
      RUN_ACCURACY=0
      shift
      ;;
    --accuracy-level)
      ACCURACY_LEVEL="$2"
      shift 2
      ;;
    --skip-zsh-usability)
      RUN_ZSH_USABILITY=0
      shift
      ;;
    --zsh-usability-level)
      ZSH_USABILITY_LEVEL="$2"
      shift 2
      ;;
    --skip-release-packaging)
      RUN_RELEASE_PACKAGING=0
      shift
      ;;
    --skip-release-rehearsal)
      RUN_RELEASE_REHEARSAL=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 2
      ;;
  esac
done

mkdir -p "${ARTIFACTS_DIR}"
log "artifact output: ${ARTIFACTS_DIR}"
log "perf suite: ${PERF_SUITE}"
log "perf gates: ${PERF_GATES}"
log "real runtime gates: ${REAL_RUNTIME_GATES}"
log "perf reindex timeout secs: ${PERF_REINDEX_TIMEOUT_SECS}"
log "accuracy level: ${ACCURACY_LEVEL}"
log "zsh usability level: ${ZSH_USABILITY_LEVEL}"

if [[ "${RUN_ACCURACY}" -eq 1 ]]; then
  accuracy_lane
fi
if [[ "${RUN_MACOS_LANE}" -eq 1 ]]; then
  macos_lane
fi
if [[ "${RUN_HARDWARE_MATRIX}" -eq 1 ]]; then
  hardware_matrix_lane
  real_runtime_local_lane
fi
if [[ "${RUN_LINUX_SMOKE}" -eq 1 ]]; then
  linux_smoke_lane
fi
if [[ "${RUN_SECURITY}" -eq 1 ]]; then
  security_lane
fi
if [[ "${RUN_RELIABILITY}" -eq 1 ]]; then
  reliability_lane
fi
if [[ "${RUN_OLLAMA_PARITY}" -eq 1 ]]; then
  ollama_parity_lane
fi
if [[ "${RUN_ZSH_USABILITY}" -eq 1 ]]; then
  zsh_usability_lane
fi
if [[ "${RUN_RELEASE_PACKAGING}" -eq 1 ]]; then
  release_packaging_lane
fi
if [[ "${RUN_RELEASE_REHEARSAL}" -eq 1 ]]; then
  release_rehearsal_lane
fi

log "all requested local CI lanes completed successfully"
log "artifacts: ${ARTIFACTS_DIR}"
