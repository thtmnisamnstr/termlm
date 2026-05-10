#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SOAK_ITERS="${TERMLM_SOAK_ITERS:-40}"
SOAK_DURATION_SECS="${TERMLM_SOAK_DURATION_SECS:-0}"
SOAK_PARALLEL_CLIENTS="${TERMLM_SOAK_PARALLEL_CLIENTS:-1}"
SOAK_PATH_CHURN_WINDOW="${TERMLM_SOAK_PATH_CHURN_WINDOW:-4}"
SOAK_METRICS_PATH="${TERMLM_SOAK_METRICS_PATH:-}"
SOAK_LOOP_SLEEP_SECS="${TERMLM_SOAK_LOOP_SLEEP_SECS:-0.02}"
RELIABILITY_RETRIES="${TERMLM_RELIABILITY_RETRIES:-3}"
RELIABILITY_RETRY_DELAY_SECS="${TERMLM_RELIABILITY_RETRY_DELAY_SECS:-0.05}"
SKIP_BUILD="${TERMLM_RELIABILITY_SKIP_BUILD:-0}"
ORIG_HOME="${HOME}"
ORIG_CARGO_HOME="${CARGO_HOME:-${ORIG_HOME}/.cargo}"
ORIG_RUSTUP_HOME="${RUSTUP_HOME:-${ORIG_HOME}/.rustup}"

TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termlm-reliability.XXXXXX")"
HOME_DIR="${TMP_ROOT}/home"
RUNTIME_DIR="${TMP_ROOT}/runtime"
WORK_DIR="${TMP_ROOT}/workspace"
BIN_DIR="${WORK_DIR}/bin"
CONFIG_DIR="${HOME_DIR}/.config/termlm"
CONFIG_PATH="${CONFIG_DIR}/config.toml"
INDEX_ROOT="${HOME_DIR}/.local/share/termlm/index"

CORE_BIN="${ROOT_DIR}/target/debug/termlm-core"
CLIENT_BIN="${ROOT_DIR}/target/debug/termlm-client"
DAEMON_LOG="${TMP_ROOT}/daemon.log"
DAEMON_PID=""
SOAK_ITERATIONS_COMPLETED=0
SOAK_ELAPSED_SECS=0
MAX_DAEMON_RSS_KB=0
MAX_DAEMON_CPU_PCT=0.0
KEEP_TMP_ROOT=0

log() {
  printf '[reliability] %s\n' "$*"
}

fail() {
  KEEP_TMP_ROOT=1
  printf '[reliability] failure: %s\n' "$*" >&2
  if [[ -f "${DAEMON_LOG}" ]]; then
    printf '[reliability] daemon log tail:\n' >&2
    tail -n 120 "${DAEMON_LOG}" >&2 || true
  fi
  exit 1
}

client_status_debug() {
  local status_log="${TMP_ROOT}/status-debug.log"
  "${CLIENT_BIN}" status --verbose >"${status_log}" 2>&1 || true
  if [[ -s "${status_log}" ]]; then
    log "status --verbose output:"
    sed -n '1,120p' "${status_log}"
  fi
}

cleanup() {
  if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    "${CLIENT_BIN}" stop >/dev/null 2>&1 || true
    kill "$DAEMON_PID" >/dev/null 2>&1 || true
    wait "$DAEMON_PID" >/dev/null 2>&1 || true
  fi
  if [[ "${KEEP_TMP_ROOT}" == "1" ]]; then
    log "preserving temp directory for debugging: ${TMP_ROOT}"
  else
    rm -rf "${TMP_ROOT}"
  fi
}
trap cleanup EXIT

wait_for_daemon() {
  local timeout_s="${1:-20}"
  local elapsed=0
  while (( elapsed < timeout_s * 10 )); do
    if "${CLIENT_BIN}" status >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
    (( elapsed += 1 ))
  done
  return 1
}

start_daemon() {
  if [[ -n "${DAEMON_PID:-}" ]]; then
    kill "$DAEMON_PID" >/dev/null 2>&1 || true
    wait "$DAEMON_PID" >/dev/null 2>&1 || true
  fi
  DAEMON_PID=""

  log "starting daemon"
  PATH="${BIN_DIR}:${PATH}" "${CORE_BIN}" --config "${CONFIG_PATH}" --sandbox-cwd "${WORK_DIR}" >>"${DAEMON_LOG}" 2>&1 &
  DAEMON_PID=$!
  wait_for_daemon 20 || fail "daemon failed to become ready"
}

prepare_env() {
  export HOME="${HOME_DIR}"
  export XDG_RUNTIME_DIR="${RUNTIME_DIR}"
  mkdir -p "${RUNTIME_DIR}" "${CONFIG_DIR}" "${WORK_DIR}" "${BIN_DIR}" "${INDEX_ROOT}"
  cat > "${CONFIG_PATH}" <<'TOML'
[inference]
provider = "local"
tool_calling_required = true
stream = true
token_idle_timeout_secs = 15
startup_failure_behavior = "fail"

[performance]
warm_core_on_start = false
keep_embedding_warm = false
prewarm_common_docs = false

[model]
auto_download = false
models_dir = "~/.local/share/termlm/models"

[indexer]
enabled = true
embedding_provider = "local"

[daemon]
shutdown_grace_secs = 7200
boot_timeout_secs = 20
TOML
}

build_bins() {
  if [[ "${SKIP_BUILD}" == "1" ]]; then
    log "skipping binary build (TERMLM_RELIABILITY_SKIP_BUILD=1)"
    return 0
  fi
  log "building termlm-core (runtime-stub) and termlm-client"
  (
    cd "${ROOT_DIR}" && \
      HOME="${ORIG_HOME}" \
      CARGO_HOME="${ORIG_CARGO_HOME}" \
      RUSTUP_HOME="${ORIG_RUSTUP_HOME}" \
      cargo build -p termlm-core --features runtime-stub --locked >/dev/null
  )
  (
    cd "${ROOT_DIR}" && \
      HOME="${ORIG_HOME}" \
      CARGO_HOME="${ORIG_CARGO_HOME}" \
      RUSTUP_HOME="${ORIG_RUSTUP_HOME}" \
      cargo build -p termlm-client --locked >/dev/null
  )
}

record_daemon_metrics() {
  [[ -n "${DAEMON_PID:-}" ]] || return 0
  if ! kill -0 "${DAEMON_PID}" 2>/dev/null; then
    return 0
  fi

  local rss_raw cpu_raw rss_kb cpu_pct
  rss_raw="$(ps -o rss= -p "${DAEMON_PID}" 2>/dev/null | tr -d '[:space:]' || true)"
  cpu_raw="$(ps -o %cpu= -p "${DAEMON_PID}" 2>/dev/null | tr -d '[:space:]' || true)"
  rss_kb="${rss_raw:-0}"
  cpu_pct="${cpu_raw:-0}"

  if [[ "${rss_kb}" =~ ^[0-9]+$ ]] && (( rss_kb > MAX_DAEMON_RSS_KB )); then
    MAX_DAEMON_RSS_KB="${rss_kb}"
  fi

  MAX_DAEMON_CPU_PCT="$(awk -v current="${MAX_DAEMON_CPU_PCT}" -v sample="${cpu_pct}" 'BEGIN { c=current+0; s=sample+0; if (s>c) printf "%.2f", s; else printf "%.2f", c }')"
}

run_parallel_ping_fanout() {
  local retries="${RELIABILITY_RETRIES}"
  local delay="${RELIABILITY_RETRY_DELAY_SECS}"
  local attempt
  for (( attempt=1; attempt<=retries; attempt++ )); do
    if run_parallel_ping_fanout_once "$1"; then
      return 0
    fi
    if (( attempt < retries )); then
      log "parallel ping fanout failed (attempt ${attempt}/${retries}); retrying"
      sleep "${delay}"
    fi
  done
  return 1
}

run_parallel_ping_fanout_once() {
  local fanout="$1"
  local pids=()
  local idx
  for (( idx=1; idx<=fanout; idx++ )); do
    "${CLIENT_BIN}" ping >/dev/null 2>&1 &
    pids+=("$!")
  done
  local pid
  for pid in "${pids[@]}"; do
    if ! wait "${pid}"; then
      return 1
    fi
  done
  return 0
}

run_client_with_retry() {
  local label="$1"
  shift
  local retries="${RELIABILITY_RETRIES}"
  local delay="${RELIABILITY_RETRY_DELAY_SECS}"
  local attempt
  local output=""
  local exit_code=0

  for (( attempt=1; attempt<=retries; attempt++ )); do
    set +e
    output="$("$@" 2>&1)"
    exit_code=$?
    set -e
    if (( exit_code == 0 )); then
      return 0
    fi
    if (( attempt < retries )); then
      log "${label} failed (attempt ${attempt}/${retries}, exit ${exit_code}); retrying"
      sleep "${delay}"
      continue
    fi
  done

  log "${label} failed after ${retries} attempts (exit ${exit_code})"
  if [[ -n "${output:-}" ]]; then
    log "${label} stderr/stdout:"
    printf '%s\n' "${output}" | sed -n '1,120p'
  fi
  client_status_debug
  return "${exit_code}"
}

soak_drill() {
  local mode_desc
  if (( SOAK_DURATION_SECS > 0 )); then
    mode_desc="${SOAK_DURATION_SECS}s duration target"
  else
    mode_desc="${SOAK_ITERS} iterations"
  fi
  log "running soak drill (${mode_desc}; fanout=${SOAK_PARALLEL_CLIENTS}; path_window=${SOAK_PATH_CHURN_WINDOW})"

  local start_secs now elapsed i
  start_secs="$(date +%s)"
  i=0
  while true; do
    i=$((i + 1))
    local cmd_path="${BIN_DIR}/drill-${i}"
    cat > "${cmd_path}" <<EOF
#!/usr/bin/env bash
echo "drill-${i}"
EOF
    chmod +x "${cmd_path}"

    if (( i > SOAK_PATH_CHURN_WINDOW )); then
      rm -f "${BIN_DIR}/drill-$((i - SOAK_PATH_CHURN_WINDOW))"
    fi

    run_client_with_retry "delta reindex failed in soak iteration ${i}" \
      "${CLIENT_BIN}" reindex --mode delta || fail "delta reindex failed in soak iteration ${i}"
    if (( SOAK_PARALLEL_CLIENTS > 1 )); then
      run_parallel_ping_fanout "${SOAK_PARALLEL_CLIENTS}" || fail "ping fanout failed in soak iteration ${i}"
    else
      run_client_with_retry "ping failed in soak iteration ${i}" \
        "${CLIENT_BIN}" ping || fail "ping failed in soak iteration ${i}"
    fi
    record_daemon_metrics

    now="$(date +%s)"
    elapsed=$((now - start_secs))

    if [[ "${SOAK_LOOP_SLEEP_SECS}" != "0" ]]; then
      sleep "${SOAK_LOOP_SLEEP_SECS}"
    fi

    if (( SOAK_DURATION_SECS > 0 )); then
      if (( elapsed >= SOAK_DURATION_SECS )); then
        break
      fi
    elif (( i >= SOAK_ITERS )); then
      break
    fi
  done

  SOAK_ITERATIONS_COMPLETED="${i}"
  SOAK_ELAPSED_SECS="${elapsed}"
}

write_metrics_file() {
  [[ -n "${SOAK_METRICS_PATH}" ]] || return 0
  mkdir -p "$(dirname "${SOAK_METRICS_PATH}")"
  cat > "${SOAK_METRICS_PATH}" <<EOF
{
  "soak_iterations_completed": ${SOAK_ITERATIONS_COMPLETED},
  "soak_elapsed_secs": ${SOAK_ELAPSED_SECS},
  "soak_duration_target_secs": ${SOAK_DURATION_SECS},
  "soak_parallel_clients": ${SOAK_PARALLEL_CLIENTS},
  "soak_path_churn_window": ${SOAK_PATH_CHURN_WINDOW},
  "max_daemon_rss_kb": ${MAX_DAEMON_RSS_KB},
  "max_daemon_cpu_pct": ${MAX_DAEMON_CPU_PCT}
}
EOF
  log "wrote soak metrics: ${SOAK_METRICS_PATH}"
}

crash_recovery_drill() {
  log "running crash-recovery drill"
  [[ -n "${DAEMON_PID:-}" ]] || fail "daemon pid missing before crash drill"
  kill -9 "${DAEMON_PID}" >/dev/null 2>&1 || fail "failed to SIGKILL daemon"
  wait "${DAEMON_PID}" >/dev/null 2>&1 || true
  DAEMON_PID=""

  if "${CLIENT_BIN}" status >/dev/null 2>&1; then
    fail "status unexpectedly succeeded after daemon crash"
  fi

  start_daemon
  "${CLIENT_BIN}" ping >/dev/null 2>&1 || fail "ping failed after daemon restart"
}

index_write_failure_drill() {
  log "running index write-failure recovery drill"
  chmod -R a-w "${INDEX_ROOT}"
  "${CLIENT_BIN}" reindex --mode full >/dev/null 2>&1 || true
  if ! wait_for_daemon 5; then
    log "daemon exited during write-failure phase; restarting for recovery verification"
    start_daemon
  fi

  chmod -R u+rwX "${INDEX_ROOT}" || true
  rm -rf "${INDEX_ROOT}"
  mkdir -p "${INDEX_ROOT}"
  start_daemon
  if ! "${CLIENT_BIN}" reindex --mode delta; then
    "${CLIENT_BIN}" status --verbose || true
    fail "delta reindex failed after restoring index permissions"
  fi
  "${CLIENT_BIN}" ping >/dev/null 2>&1 || fail "daemon unresponsive after write-failure recovery"
}

main() {
  prepare_env
  build_bins
  start_daemon
  soak_drill
  crash_recovery_drill
  index_write_failure_drill
  write_metrics_file
  log "all reliability drills passed"
}

main "$@"
