#!/usr/bin/env zsh

emulate -L zsh
setopt pipe_fail no_unset
zmodload zsh/datetime 2>/dev/null || true

SCRIPT_DIR="${0:A:h}"
REPO_ROOT="${SCRIPT_DIR:h:h}"

source "${REPO_ROOT}/plugins/zsh/lib/capture.zsh"
source "${REPO_ROOT}/plugins/zsh/lib/terminal-observer.zsh"

termlm-send-shell-context() { return 0; }
termlm-helper-send() { return 0; }
termlm-load-capture-settings() { :; }
termlm-load-observer-settings() { :; }
termlm-observer-start-capture() {
  _TERMLM_OBS_CAPTURE_ACTIVE=1
  return 0
}
termlm-observer-stop-capture() {
  _TERMLM_OBS_CAPTURE_ACTIVE=0
  return 0
}

termlm-json-escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/\\n}"
  s="${s//$'\r'/\\r}"
  s="${s//$'\t'/\\t}"
  print -rn -- "$s"
}

typeset -g _TERMLM_RUN_DIR="${TMPDIR:-/tmp}/termlm-observer-bench-$$"
mkdir -p -- "$_TERMLM_RUN_DIR"
typeset -g _TERMLM_SHELL_ID="bench-shell"
typeset -g _TERMLM_PENDING_TASK_ID=""
typeset -g _TERMLM_SESSION_MODE=0
typeset -g _TERMLM_WAITING_MODEL=0
typeset -g _TERMLM_LAST_CONTEXT_HASH=""
typeset -g _TERMLM_CAPTURE_ENABLED=1
typeset -g _TERMLM_CAPTURE_MAX_BYTES=16384
typeset -g _TERMLM_OBS_EXCLUDE_TUI=0
typeset -ga _TERMLM_OBS_EXCLUDE_PATTERNS=()
typeset -ga _TERMLM_OBS_EXCLUDE_COMMANDS=()

measure_mean_overhead_ms() {
  local capture_all="$1"
  local loops="$2"
  local -a samples=()
  local i start end delta
  _TERMLM_OBS_CAPTURE_ALL="$capture_all"
  for (( i = 0; i < loops; i++ )); do
    start="$EPOCHREALTIME"
    termlm-preexec "echo bench" "echo bench"
    true
    termlm-precmd
    end="$EPOCHREALTIME"
    delta=$(( (end - start) * 1000.0 ))
    if (( delta < 0 )); then
      delta=0
    fi
    samples+=("${delta}")
  done
  printf '%s\n' "${samples[@]}" | awk '{s+=$1; n++} END { if (n==0) { print "0.000000" } else { printf("%.6f", s/n) } }'
}

measure_capture_wrapper_overhead_ms() {
  local loops="$1"
  local -a samples=()
  local i start end delta
  for (( i = 1; i <= loops; i++ )); do
    start="$EPOCHREALTIME"
    termlm-observer-start-capture "$i" >/dev/null 2>&1 || true
    termlm-observer-stop-capture
    end="$EPOCHREALTIME"
    delta=$(( (end - start) * 1000.0 ))
    if (( delta < 0 )); then
      delta=0
    fi
    samples+=("${delta}")
  done
  printf '%s\n' "${samples[@]}" | awk '{s+=$1; n++} END { if (n==0) { print "0.000000" } else { printf("%.6f", s/n) } }'
}

loops=200
no_capture_ms="$(measure_mean_overhead_ms 0 "$loops")"
capture_ms="$(measure_capture_wrapper_overhead_ms "$loops")"

printf '{"observed_command_overhead_ms": %s, "observed_command_capture_overhead_ms": %s}\n' \
  "$no_capture_ms" \
  "$capture_ms"

rm -rf -- "$_TERMLM_RUN_DIR"
