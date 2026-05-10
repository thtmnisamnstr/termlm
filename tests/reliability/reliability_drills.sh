#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SOAK_ITERS="${TERMLM_SOAK_ITERS:-40}"
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
DAEMON_PID=""

log() {
  printf '[reliability] %s\n' "$*"
}

fail() {
  printf '[reliability] failure: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    "${CLIENT_BIN}" stop >/dev/null 2>&1 || true
    kill "$DAEMON_PID" >/dev/null 2>&1 || true
    wait "$DAEMON_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "${TMP_ROOT}"
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
  PATH="${BIN_DIR}:${PATH}" "${CORE_BIN}" --config "${CONFIG_PATH}" --sandbox-cwd "${WORK_DIR}" >/dev/null 2>&1 &
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
shutdown_grace_secs = 60
boot_timeout_secs = 20
TOML
}

build_bins() {
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

soak_drill() {
  log "running soak drill (${SOAK_ITERS} iterations)"
  local i
  for (( i=1; i<=SOAK_ITERS; i++ )); do
    local cmd_path="${BIN_DIR}/drill-${i}"
    cat > "${cmd_path}" <<EOF
#!/usr/bin/env bash
echo "drill-${i}"
EOF
    chmod +x "${cmd_path}"

    if (( i > 4 )); then
      rm -f "${BIN_DIR}/drill-$((i - 4))"
    fi

    "${CLIENT_BIN}" reindex --mode delta >/dev/null 2>&1 || fail "delta reindex failed in soak iteration ${i}"
    "${CLIENT_BIN}" ping >/dev/null 2>&1 || fail "ping failed in soak iteration ${i}"
  done
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
  log "all reliability drills passed"
}

main "$@"
