#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OUT_DIR="${1:-/tmp/termlm-perf-matrix-$(date +%Y%m%d-%H%M%S)}"
SUITE="${REPO_ROOT}/tests/fixtures/termlm-test-suite.toml"
GATES="${REPO_ROOT}/tests/perf/perf-gates.toml"
MODEL_PATH="${TERMLM_PERF_MATRIX_LOCAL_MODEL_PATH:-${HOME}/.local/share/termlm/models/gemma-4-E4B-it-Q4_K_M.gguf}"
OLLAMA_MODEL="${TERMLM_TEST_OLLAMA_MODEL:-gemma3:1b}"
CASES_RAW="${TERMLM_PERF_MATRIX_CASES:-local_stub_all,local_real_e2e,ollama_integration}"
REQUIRE_REAL_LOCAL="${TERMLM_PERF_MATRIX_REQUIRE_REAL_LOCAL:-0}"
REQUIRE_OLLAMA="${TERMLM_PERF_MATRIX_REQUIRE_OLLAMA:-0}"
RECORDS_FILE=""
RUN_FAILED=0

mkdir -p "${OUT_DIR}"
RECORDS_FILE="${OUT_DIR}/case-records.tsv"
: > "${RECORDS_FILE}"

if ! command -v jq >/dev/null 2>&1; then
  echo "hardware matrix requires jq in PATH" >&2
  exit 2
fi

has_case() {
  local needle="$1"
  local list=",${CASES_RAW},"
  [[ "${list}" == *",${needle},"* ]]
}

validate_case_selection() {
  local raw="${CASES_RAW}"
  local known_count=0
  local case_name=""
  IFS=',' read -r -a requested <<< "${raw}"
  for case_name in "${requested[@]}"; do
    case "${case_name}" in
      local_stub_all|local_real_e2e|ollama_integration)
        known_count=$((known_count + 1))
        ;;
      *)
        echo "unknown case in TERMLM_PERF_MATRIX_CASES: ${case_name}" >&2
        exit 2
        ;;
    esac
  done
  if [[ "${known_count}" -eq 0 ]]; then
    echo "no valid cases selected via TERMLM_PERF_MATRIX_CASES=${raw}" >&2
    exit 2
  fi
}

record_case() {
  local name="$1"
  local status="$2"
  local out_file="${3:-}"
  local reason="${4:-}"
  local command="${5:-}"
  printf '%s\t%s\t%s\t%s\t%s\n' "${name}" "${status}" "${out_file}" "${reason}" "${command}" >> "${RECORDS_FILE}"
}

validate_case_selection

run_case() {
  local name="$1"
  local command="$2"
  shift
  shift
  local out_file="${OUT_DIR}/${name}.json"
  local started_at_utc
  started_at_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "==> running ${name}"
  if (cd "${REPO_ROOT}" && "$@" --results-out "${out_file}"); then
    if [[ ! -s "${out_file}" ]]; then
      echo "    failed (${name}): missing results file ${out_file}" >&2
      record_case "${name}" "failed" "${out_file}" "missing-results-file" "${command}"
      RUN_FAILED=1
      return 0
    fi
    if ! jq -e '.summary.total >= 1 and (.summary.failed | type == "number") and (.benchmark_environment.provider | type == "string")' "${out_file}" >/dev/null; then
      echo "    failed (${name}): invalid results schema in ${out_file}" >&2
      record_case "${name}" "failed" "${out_file}" "invalid-results-schema" "${command}"
      RUN_FAILED=1
      return 0
    fi
    echo "    wrote ${out_file}"
    record_case "${name}" "passed" "${out_file}" "" "${command}"
  else
    local exit_code=$?
    echo "    failed (${name}): command exit ${exit_code}" >&2
    record_case "${name}" "failed" "${out_file}" "command-exit-${exit_code}" "${command}"
    RUN_FAILED=1
  fi
  local ended_at_utc
  ended_at_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "    window ${started_at_utc} -> ${ended_at_utc}"
}

if has_case "local_stub_all"; then
  run_case "local_stub_all" \
  "cargo run -p termlm-test --release --locked -- --suite ${SUITE} --mode all --provider local --perf-gates ${GATES}" \
  cargo run -p termlm-test --release --locked -- \
  --suite "${SUITE}" --mode all --provider local --perf-gates "${GATES}"
fi

if has_case "local_real_e2e"; then
  if [[ -f "${MODEL_PATH}" ]]; then
    run_case "local_real_e2e" \
      "TERMLM_E2E_REAL=1 cargo run -p termlm-test --release --locked -- --suite ${SUITE} --mode local-integration --provider local" \
      env TERMLM_E2E_REAL=1 cargo run -p termlm-test --release --locked -- \
      --suite "${SUITE}" --mode local-integration --provider local
  else
    local_reason="missing-model-${MODEL_PATH}"
    if [[ "${REQUIRE_REAL_LOCAL}" == "1" ]]; then
      echo "==> required case local_real_e2e missing prerequisite (${MODEL_PATH})" >&2
      record_case "local_real_e2e" "failed" "" "${local_reason}" "TERMLM_E2E_REAL=1 cargo run -p termlm-test --release --locked -- --suite ${SUITE} --mode local-integration --provider local"
      RUN_FAILED=1
    else
      echo "==> skipping local_real_e2e (missing model: ${MODEL_PATH})"
      record_case "local_real_e2e" "skipped" "" "${local_reason}" "TERMLM_E2E_REAL=1 cargo run -p termlm-test --release --locked -- --suite ${SUITE} --mode local-integration --provider local"
    fi
  fi
fi

if has_case "ollama_integration"; then
  if command -v ollama >/dev/null 2>&1; then
    run_case "ollama_integration" \
      "TERMLM_TEST_OLLAMA=1 TERMLM_TEST_OLLAMA_MODEL=${OLLAMA_MODEL} cargo run -p termlm-test --release --locked -- --suite ${SUITE} --mode ollama-integration --provider ollama" \
      env TERMLM_TEST_OLLAMA=1 TERMLM_TEST_OLLAMA_MODEL="${OLLAMA_MODEL}" cargo run -p termlm-test --release --locked -- \
      --suite "${SUITE}" --mode ollama-integration --provider ollama
  else
    ollama_reason="missing-ollama-binary"
    if [[ "${REQUIRE_OLLAMA}" == "1" ]]; then
      echo "==> required case ollama_integration missing prerequisite (ollama binary not found)" >&2
      record_case "ollama_integration" "failed" "" "${ollama_reason}" "TERMLM_TEST_OLLAMA=1 TERMLM_TEST_OLLAMA_MODEL=${OLLAMA_MODEL} cargo run -p termlm-test --release --locked -- --suite ${SUITE} --mode ollama-integration --provider ollama"
      RUN_FAILED=1
    else
      echo "==> skipping ollama_integration (ollama binary not found)"
      record_case "ollama_integration" "skipped" "" "${ollama_reason}" "TERMLM_TEST_OLLAMA=1 TERMLM_TEST_OLLAMA_MODEL=${OLLAMA_MODEL} cargo run -p termlm-test --release --locked -- --suite ${SUITE} --mode ollama-integration --provider ollama"
    fi
  fi
fi

MANIFEST_PATH="${OUT_DIR}/manifest.json"
jq -Rn \
  --arg generated_at_utc "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg repo_root "${REPO_ROOT}" \
  --arg git_commit "$(cd "${REPO_ROOT}" && git rev-parse --short=12 HEAD 2>/dev/null || echo unknown)" \
  --arg host_os "$(uname -s)" \
  --arg host_arch "$(uname -m)" \
  --arg host_name "$(hostname -s 2>/dev/null || hostname)" \
  --arg suite "${SUITE}" \
  --arg gates "${GATES}" \
  --arg model_path "${MODEL_PATH}" \
  --arg ollama_model "${OLLAMA_MODEL}" \
  --arg selected_cases "${CASES_RAW}" \
  '
  [inputs | select(length > 0) | split("\t")] as $rows
  | {
      generated_at_utc: $generated_at_utc,
      repo_root: $repo_root,
      git_commit: $git_commit,
      host: {
        os: $host_os,
        arch: $host_arch,
        name: $host_name
      },
      suite: $suite,
      perf_gates: $gates,
      selected_cases: $selected_cases,
      local_model_path: $model_path,
      ollama_model: $ollama_model,
      cases: [
        $rows[] | {
          name: .[0],
          status: .[1],
          results_file: (if .[2] == "" then null else .[2] end),
          reason: (if .[3] == "" then null else .[3] end),
          command: (if .[4] == "" then null else .[4] end)
        }
      ]
    }
  | .summary = {
      total: (.cases | length),
      passed: (.cases | map(select(.status == "passed")) | length),
      failed: (.cases | map(select(.status == "failed")) | length),
      skipped: (.cases | map(select(.status == "skipped")) | length)
    }
  ' "${RECORDS_FILE}" > "${MANIFEST_PATH}"

echo "==> per-case summaries"
for f in "${OUT_DIR}"/*.json; do
  [[ -e "${f}" ]] || continue
  [[ "${f}" == "${MANIFEST_PATH}" ]] && continue
  echo "-- ${f##*/}"
  jq -r '"summary: passed=\(.summary.passed) failed=\(.summary.failed) total=\(.summary.total) duration_secs=\(.duration_secs) env=\(.benchmark_environment.os)/\(.benchmark_environment.arch) provider=\(.benchmark_environment.provider) model=\(.benchmark_environment.model) profile=\(.benchmark_environment.performance_profile)"' "${f}"
done

(
  cd "${OUT_DIR}"
  shasum -a 256 ./*.json > SHA256SUMS
)

echo "==> manifest summary"
jq -r '"summary: passed=\(.summary.passed) failed=\(.summary.failed) skipped=\(.summary.skipped) total=\(.summary.total)"' "${MANIFEST_PATH}"

if jq -e '.summary.failed > 0' "${MANIFEST_PATH}" >/dev/null; then
  RUN_FAILED=1
fi

echo "hardware matrix complete: ${OUT_DIR}"

if [[ "${RUN_FAILED}" -ne 0 ]]; then
  exit 1
fi
