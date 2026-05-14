#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

LEVEL="${TERMLM_ACCURACY_LEVEL:-commit}"
ARTIFACTS_DIR="${TERMLM_ACCURACY_ARTIFACTS_DIR:-/tmp/termlm-accuracy-$(date +%Y%m%d-%H%M%S)}"
SUITE="${TERMLM_ACCURACY_SUITE:-${REPO_ROOT}/tests/fixtures/termlm-test-suite.toml}"
USABILITY_SUITE="${TERMLM_ACCURACY_USABILITY_SUITE:-${REPO_ROOT}/tests/fixtures/termlm-usability-suite.toml}"
TOP_K="${TERMLM_ACCURACY_TOP_K:-8}"
RUN_REAL_ZSH="${TERMLM_ACCURACY_RUN_REAL_ZSH:-auto}"
REQUIRE_REAL="${TERMLM_ACCURACY_REQUIRE_REAL:-0}"
RUN_REAL_HARNESS="${TERMLM_ACCURACY_RUN_REAL_HARNESS:-0}"
STRICT_DIAGNOSTICS="${TERMLM_ACCURACY_STRICT_DIAGNOSTICS:-0}"
TUNER_CMD="${TERMLM_ACCURACY_TUNER_CMD:-}"

usage() {
  cat <<'EOF'
Usage: scripts/ci/run_accuracy_gate.sh [options]

Runs the prompt/retrieval accuracy gate and writes JSON + Markdown artifacts.

Options:
  --level <commit|full|release>
                              Coverage level (default: TERMLM_ACCURACY_LEVEL or commit)
  --artifacts-dir <path>      Output directory for JSON reports and tuning notes
  --suite <path>              Broad behavioral/retrieval suite
  --usability-suite <path>    Focused terminal usability suite
  --top-k <n>                 Retrieval top-k used by harness checks (default: 8)
  --with-real-zsh             Run the real zsh plugin journey when model assets exist
  --skip-real-zsh             Do not run the real zsh plugin journey
  --require-real              Fail if real-model zsh checks cannot run
  --run-real-harness          Also run the focused termlm-test real-model e2e diagnostic lane
  --strict-diagnostics        Treat diagnostic lanes as blocking failures
  -h, --help                  Show help

Environment:
  TERMLM_ACCURACY_TUNER_CMD   Optional command that receives the aggregate Markdown report on stdin
                              and writes LLM tuning notes to artifacts/llm-tuning-notes.md.
EOF
}

log() {
  printf '[accuracy] %s\n' "$*"
}

failures=0
diagnostic_failures=0
json_reports=()
diagnostic_reports=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --level)
      LEVEL="${2:-}"
      shift 2
      ;;
    --artifacts-dir)
      ARTIFACTS_DIR="${2:-}"
      shift 2
      ;;
    --suite)
      SUITE="${2:-}"
      shift 2
      ;;
    --usability-suite)
      USABILITY_SUITE="${2:-}"
      shift 2
      ;;
    --top-k)
      TOP_K="${2:-}"
      shift 2
      ;;
    --with-real-zsh)
      RUN_REAL_ZSH=1
      shift
      ;;
    --skip-real-zsh)
      RUN_REAL_ZSH=0
      shift
      ;;
    --require-real)
      REQUIRE_REAL=1
      RUN_REAL_ZSH=1
      shift
      ;;
    --run-real-harness)
      RUN_REAL_HARNESS=1
      shift
      ;;
    --strict-diagnostics)
      STRICT_DIAGNOSTICS=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "${LEVEL}" in
  commit|full|release) ;;
  *)
    echo "unknown accuracy level: ${LEVEL}" >&2
    usage >&2
    exit 2
    ;;
esac

if ! [[ "${TOP_K}" =~ ^[0-9]+$ ]] || [[ "${TOP_K}" -le 0 ]]; then
  echo "--top-k must be a positive integer" >&2
  exit 2
fi

mkdir -p "${ARTIFACTS_DIR}"

cargo_run_args=(run -p termlm-test --locked)
if [[ "${LEVEL}" == "release" ]]; then
  cargo_run_args=(run -p termlm-test --release --locked)
fi

run_harness() {
  local label="$1"
  local suite="$2"
  local mode="$3"
  local blocking="$4"
  shift 4

  local json_out="${ARTIFACTS_DIR}/${label}.json"
  local md_out="${ARTIFACTS_DIR}/${label}.md"
  log "BEGIN ${label} (${mode})"
  set +e
  (
    cd "${REPO_ROOT}" && \
      cargo "${cargo_run_args[@]}" -- \
        --suite "${suite}" \
        --mode "${mode}" \
        --top-k "${TOP_K}" \
        --skip-benchmarks \
        --results-out "${json_out}" \
        "$@"
  )
  local status=$?
  set -e

  if [[ -f "${json_out}" ]]; then
    json_reports+=("${json_out}")
    if [[ "${blocking}" != "1" ]]; then
      diagnostic_reports+=("${json_out}")
    fi
    python3 "${REPO_ROOT}/scripts/ci/summarize_accuracy_results.py" \
      --input "${json_out}" \
      --out "${md_out}"
    log "REPORT ${md_out}"
  else
    log "WARN ${label} did not produce ${json_out}"
  fi

  if [[ "${status}" -ne 0 ]]; then
    if [[ "${blocking}" == "1" || "${STRICT_DIAGNOSTICS}" == "1" ]]; then
      failures=$((failures + 1))
      log "FAIL ${label}"
    else
      diagnostic_failures=$((diagnostic_failures + 1))
      log "DIAGNOSTIC-FAIL ${label}"
    fi
  else
    log "PASS ${label}"
  fi
}

models_available() {
  local model_dir="${TERMLM_ZSH_USABILITY_MODEL_DIR:-${HOME}/.local/share/termlm/models}"
  [[ -f "${model_dir}/gemma-4-E4B-it-Q4_K_M.gguf" && -f "${model_dir}/bge-small-en-v1.5.Q4_K_M.gguf" ]]
}

should_run_real_zsh() {
  if [[ "${RUN_REAL_ZSH}" == "1" ]]; then
    return 0
  fi
  if [[ "${RUN_REAL_ZSH}" == "0" ]]; then
    return 1
  fi
  [[ "${LEVEL}" == "full" || "${LEVEL}" == "release" ]]
}

run_zsh_journey() {
  local zsh_level="smoke"
  local zsh_profile="debug"
  if [[ "${LEVEL}" == "release" ]]; then
    zsh_level="release"
    zsh_profile="release"
  elif [[ "${LEVEL}" == "full" ]]; then
    zsh_level="release"
  fi

  if ! should_run_real_zsh; then
    log "SKIP real-zsh journey"
    return
  fi

  if ! models_available; then
    if [[ "${REQUIRE_REAL}" == "1" ]]; then
      log "FAIL real-zsh journey (missing local GGUF model assets)"
      failures=$((failures + 1))
      return
    fi
    log "SKIP real-zsh journey (missing local GGUF model assets)"
    return
  fi

  local out_dir="${ARTIFACTS_DIR}/zsh-${zsh_level}"
  log "BEGIN real-zsh journey (${zsh_level})"
  set +e
  (
    cd "${REPO_ROOT}" && \
      TERMLM_ZSH_USABILITY_LEVEL="${zsh_level}" \
      TERMLM_ZSH_USABILITY_PROFILE="${zsh_profile}" \
      TERMLM_ZSH_USABILITY_REQUIRE_MODEL="${REQUIRE_REAL}" \
      TERMLM_ZSH_USABILITY_ARTIFACT_DIR="${out_dir}" \
      bash tests/usability/zsh_user_journey.sh
  )
  local status=$?
  set -e
  if [[ "${status}" -ne 0 ]]; then
    failures=$((failures + 1))
    log "FAIL real-zsh journey"
  else
    log "PASS real-zsh journey"
  fi
}

run_tuner() {
  local aggregate="$1"
  if [[ -z "${TUNER_CMD}" ]]; then
    return
  fi
  local notes="${ARTIFACTS_DIR}/llm-tuning-notes.md"
  log "BEGIN llm tuning notes"
  set +e
  bash -lc "${TUNER_CMD}" < "${aggregate}" > "${notes}"
  local status=$?
  set -e
  if [[ "${status}" -ne 0 ]]; then
    failures=$((failures + 1))
    log "FAIL llm tuning notes (${notes})"
  else
    log "REPORT ${notes}"
  fi
}

log "artifact output: ${ARTIFACTS_DIR}"
log "level: ${LEVEL}"
log "suite: ${SUITE}"
log "usability suite: ${USABILITY_SUITE}"
log "top-k: ${TOP_K}"

run_harness "behavioral-retrieval" "${SUITE}" "retrieval" 1
run_harness "behavioral-safety" "${SUITE}" "safety" 1
run_harness "usability-retrieval" "${USABILITY_SUITE}" "retrieval" 1

if [[ "${LEVEL}" == "full" || "${LEVEL}" == "release" ]]; then
  run_harness "behavioral-all-diagnostic" "${SUITE}" "all" 0
fi

if [[ "${RUN_REAL_HARNESS}" == "1" ]]; then
  run_harness "usability-real-e2e-diagnostic" "${USABILITY_SUITE}" "e2e" 0
fi

run_zsh_journey

if [[ "${#json_reports[@]}" -gt 0 ]]; then
  summary_args=()
  for report in "${json_reports[@]}"; do
    summary_args+=(--input "${report}")
  done
  aggregate_report="${ARTIFACTS_DIR}/accuracy-report.md"
  python3 "${REPO_ROOT}/scripts/ci/summarize_accuracy_results.py" \
    "${summary_args[@]}" \
    --out "${aggregate_report}"
  log "REPORT ${aggregate_report}"
  run_tuner "${aggregate_report}"
fi

if [[ "${diagnostic_failures}" -gt 0 && "${STRICT_DIAGNOSTICS}" != "1" ]]; then
  log "diagnostic lanes reported ${diagnostic_failures} non-blocking failure(s); inspect reports before release tuning"
fi

if [[ "${failures}" -gt 0 ]]; then
  log "accuracy gate failed (${failures} blocking failure(s)); artifacts: ${ARTIFACTS_DIR}"
  exit 1
fi

log "accuracy gate passed; artifacts: ${ARTIFACTS_DIR}"
