#!/usr/bin/env zsh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
zmodload zsh/datetime >/dev/null 2>&1 || true

source "${ROOT_DIR}/plugins/zsh/widgets/prompt-mode.zsh"
source "${ROOT_DIR}/plugins/zsh/widgets/self-insert.zsh"
source "${ROOT_DIR}/plugins/zsh/widgets/accept-line.zsh"
source "${ROOT_DIR}/plugins/zsh/widgets/safety-floor.zsh"
source "${ROOT_DIR}/plugins/zsh/lib/capture.zsh"
source "${ROOT_DIR}/plugins/zsh/lib/ipc.zsh"

fail() {
  print -r -- "runtime-contract failure: $*" >&2
  exit 1
}

assert_eq() {
  local got="$1"
  local expected="$2"
  local msg="$3"
  [[ "$got" == "$expected" ]] || fail "$msg (got='$got' expected='$expected')"
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local msg="$3"
  [[ "$haystack" == *"$needle"* ]] || fail "$msg (missing '$needle' in '$haystack')"
}

typeset -ga _ZLE_CALLS=()
zle() {
  _ZLE_CALLS+=("$*")
  case "${1:-}" in
    .self-insert)
      LBUFFER+="${KEYS:-}"
      BUFFER="${LBUFFER}${RBUFFER:-}"
      CURSOR=${#LBUFFER}
      ;;
    -K)
      KEYMAP="${2:-main}"
      ;;
    *)
      ;;
  esac
}

reset_state() {
  _ZLE_CALLS=()
  KEYMAP="main"
  PS1='$ '
  BUFFER=""
  LBUFFER=""
  RBUFFER=""
  CURSOR=0
  KEYS=""
  _TERMLM_MODE="normal"
  _TERMLM_SESSION_MODE=0
  _TERMLM_SAVED_PS1='$ '
  _TERMLM_WAITING_MODEL=0
  _TERMLM_TASK_ID=""
  _TERMLM_PENDING_TASK_ID=""
  _TERMLM_PENDING_CMD=""
  _TERMLM_PENDING_CWD_BEFORE=""
  _TERMLM_PENDING_STARTED_AT=0
  _TERMLM_PENDING_STDOUT_FILE=""
  _TERMLM_PENDING_STDERR_FILE=""
  _TERMLM_PENDING_SEQ=0
}

typeset -g _ABANDON_COUNT=0
typeset -g _ABANDON_LAST_ARG=""
termlm-abandon-active-task() {
  _ABANDON_COUNT=$(( _ABANDON_COUNT + 1 ))
  _ABANDON_LAST_ARG="${1:-0}"
  _TERMLM_WAITING_MODEL=0
}

typeset -g _TEST_APPROVAL_DECISION="approved"
termlm-approval-prompt() {
  print -r -- "$_TEST_APPROVAL_DECISION"
}

termlm-send-decision() {
  return 0
}

termlm-capture-enabled() {
  return 1
}

termlm-wrap-command-for-capture() {
  local cmd="$1"
  local seq="$2"
  print -r -- "wrapped:${cmd}:${seq}"
}

reset_state
KEYS='?'
termlm-self-insert
assert_eq "$_TERMLM_MODE" "prompt" "self-insert should enter prompt mode"
assert_eq "$BUFFER" "" "self-insert prompt entry should clear BUFFER"
assert_eq "$KEYMAP" "termlm-prompt" "prompt mode should switch keymap"

reset_state
KEYS='?'
LBUFFER=$'\\'
BUFFER=$'\\'
CURSOR=1
termlm-self-insert
assert_eq "$BUFFER" "\\?" "escaped '?' should insert literal question mark"
assert_eq "$_TERMLM_MODE" "normal" "escaped '?' should not enter prompt mode"
assert_contains "${(j:|:)_ZLE_CALLS}" ".self-insert" "escaped '?' path should use native self-insert"

reset_state
BUFFER="/p"
termlm-accept-line
assert_eq "$_TERMLM_SESSION_MODE" "1" "/p should enter session mode"
assert_eq "$_TERMLM_MODE" "session" "/p should set session mode state"
assert_eq "$PS1" "$_TERMLM_SESSION_INDICATOR" "/p should set session indicator"

BUFFER="/q"
termlm-accept-line
assert_eq "$_TERMLM_SESSION_MODE" "0" "/q should exit session mode"
assert_eq "$_TERMLM_MODE" "normal" "/q should restore normal mode"
assert_eq "$KEYMAP" "main" "/q should restore main keymap"

reset_state
_TERMLM_MODE="prompt"
_TERMLM_WAITING_MODEL=1
BUFFER="ls -la"
_ABANDON_COUNT=0
_ABANDON_LAST_ARG=""
termlm-accept-line
assert_eq "$_ABANDON_COUNT" "1" "typed command while waiting should abandon active task"
assert_eq "$_ABANDON_LAST_ARG" "0" "non-session implicit abort should not preserve session"
assert_eq "$BUFFER" "ls -la" "typed command should remain in BUFFER after implicit abort"
assert_eq "$CURSOR" "${#BUFFER}" "cursor should move to end after implicit abort preservation"

reset_state
_TERMLM_MODE="session"
_TERMLM_SESSION_MODE=1
_TERMLM_WAITING_MODEL=1
BUFFER="pwd"
_ABANDON_COUNT=0
_ABANDON_LAST_ARG=""
termlm-accept-line
assert_eq "$_ABANDON_COUNT" "1" "session implicit command should abandon active task"
assert_eq "$_ABANDON_LAST_ARG" "1" "session implicit abort should preserve session"

reset_state
_TERMLM_MODE="prompt"
_TEST_APPROVAL_DECISION="approved"
termlm-handle-proposed-event "task-approved" "echo hi" "true"
assert_eq "$_TERMLM_PENDING_TASK_ID" "task-approved" "approved command should set pending task id"
assert_eq "$_TERMLM_PENDING_CMD" "echo hi" "approved command should set pending command"
assert_eq "$BUFFER" "wrapped:echo hi:1" "approved command should execute wrapped command via BUFFER"
assert_contains "${(j:|:)_ZLE_CALLS}" ".accept-line" "approved command should execute with zle .accept-line"

reset_state
_TERMLM_MODE="prompt"
_TEST_APPROVAL_DECISION="rejected"
termlm-handle-proposed-event "task-rejected" "echo hi" "true"
assert_eq "$_TERMLM_WAITING_MODEL" "1" "rejected command should continue waiting for model"
assert_eq "$_TERMLM_MODE" "prompt" "rejected command should remain in prompt mode"

reset_state
_TERMLM_MODE="prompt"
_TEST_APPROVAL_DECISION="abort"
termlm-handle-proposed-event "task-abort" "echo hi" "true"
assert_eq "$_TERMLM_WAITING_MODEL" "0" "aborted command should stop waiting"
assert_eq "$_TERMLM_TASK_ID" "" "aborted command should clear active task id"

print -r -- "zsh runtime behavior checks passed."
