#!/usr/bin/env zsh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
zmodload zsh/datetime >/dev/null 2>&1 || true

source "${ROOT_DIR}/plugins/zsh/widgets/prompt-mode.zsh"
source "${ROOT_DIR}/plugins/zsh/widgets/self-insert.zsh"
source "${ROOT_DIR}/plugins/zsh/widgets/accept-line.zsh"
source "${ROOT_DIR}/plugins/zsh/widgets/escape.zsh"
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

assert_not_contains() {
  local haystack="$1"
  local needle="$2"
  local msg="$3"
  [[ "$haystack" != *"$needle"* ]] || fail "$msg (unexpected '$needle' in '$haystack')"
}

encode_b64() {
  print -rn -- "$1" | base64 | tr -d '\n'
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
  _TERMLM_CLARIFICATION_TASK_ID=""
  _TERMLM_APPROVAL_TASK_ID=""
  _TERMLM_APPROVAL_CMD=""
  _TERMLM_EDITING_APPROVAL_TASK_ID=""
  _TERMLM_CLOSED_TASK_ID=""
  _TERMLM_PENDING_TASK_ID=""
  _TERMLM_PENDING_CMD=""
  _TERMLM_PENDING_CWD_BEFORE=""
  _TERMLM_PENDING_STARTED_AT=0
  _TERMLM_PENDING_STDOUT_FILE=""
  _TERMLM_PENDING_STDERR_FILE=""
  _TERMLM_PENDING_SEQ=0
  _TERMLM_ACKED_PENDING_TASK_ID=""
  _TERMLM_CAPTURE_ACTIVE=0
  _TERMLM_CAPTURE_SAVE_STDOUT_FD=-1
  _TERMLM_CAPTURE_SAVE_STDERR_FD=-1
  _TERMLM_OBS_CAPTURE_ACTIVE=0
  _TERMLM_OBS_SAVE_STDOUT_FD=-1
  _TERMLM_OBS_SAVE_STDERR_FD=-1
  _TERMLM_OUTPUT_STARTED=0
  _TERMLM_OUTPUT_NEEDS_NEWLINE=0
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

typeset -g _TEST_DECISION_ARGS=""
typeset -g _TEST_DECISION_FAIL=0
termlm-send-decision() {
  _TEST_DECISION_ARGS="$*"
  if (( _TEST_DECISION_FAIL != 0 )); then
    termlm-reset-after-connection-lost
  fi
  return $_TEST_DECISION_FAIL
}

termlm-capture-enabled() {
  return 1
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
_TERMLM_MODE="prompt"
KEYMAP="termlm-prompt"
BUFFER="cancel this"
LBUFFER="cancel this"
CURSOR=${#BUFFER}
_TEST_DECISION_ARGS=""
termlm-cancel-prompt
assert_eq "$_TERMLM_MODE" "normal" "escape should exit prompt mode before submission"
assert_eq "$KEYMAP" "main" "escape should restore main keymap"
assert_eq "$BUFFER" "" "escape should clear prompt buffer"
assert_eq "$_TEST_DECISION_ARGS" "" "escape without an active task should not send a decision"

reset_state
_TERMLM_MODE="prompt"
KEYMAP="termlm-prompt"
_TERMLM_TASK_ID="task-waiting"
_TERMLM_WAITING_MODEL=1
_TEST_DECISION_ARGS=""
termlm-cancel-prompt
assert_contains "$_TEST_DECISION_ARGS" "task-waiting" "escape should address the active task"
assert_contains "$_TEST_DECISION_ARGS" "--decision abort" "escape should abort the active task"
assert_eq "$_TERMLM_WAITING_MODEL" "0" "escape should stop model wait"
assert_eq "$_TERMLM_TASK_ID" "" "escape should clear task id"
assert_eq "$_TERMLM_MODE" "normal" "escape should leave prompt mode after abort"
late_b64="$(encode_b64 "late output")"
termlm-handle-run-task-line "{\"event\":\"model_text\",\"task_id\":\"task-waiting\",\"chunk_b64\":\"${late_b64}\"}"
assert_eq "$_TERMLM_TASK_ID" "" "late model output for escaped task should be ignored"
assert_eq "$_TERMLM_OUTPUT_STARTED" "0" "late model output for escaped task should not redraw"

reset_state
_TERMLM_MODE="prompt"
KEYMAP="termlm-prompt"
_TERMLM_TASK_ID="task-provider-recover"
_TERMLM_WAITING_MODEL=1
provider_msg_b64="$(encode_b64 "provider request failed")"
termlm-handle-run-task-line "{\"event\":\"error\",\"task_id\":\"task-provider-recover\",\"kind\":\"inference_provider_unavailable\",\"message_b64\":\"${provider_msg_b64}\"}"
assert_eq "$_TERMLM_TASK_ID" "task-provider-recover" "provider failure should not close task before fallback messages"
assert_eq "$_TERMLM_CLOSED_TASK_ID" "" "provider failure should leave follow-up events eligible"
provider_question_b64="$(encode_b64 "What exact behavior should run?")"
termlm-handle-run-task-line "{\"event\":\"needs_clarification\",\"task_id\":\"task-provider-recover\",\"question_b64\":\"${provider_question_b64}\"}"
assert_eq "$_TERMLM_CLARIFICATION_TASK_ID" "task-provider-recover" "provider fallback clarification should still be accepted"

reset_state
_TERMLM_MODE="prompt"
KEYMAP="termlm-prompt"
_TERMLM_TASK_ID="task-bad-protocol"
_TERMLM_WAITING_MODEL=1
bad_protocol_b64="$(encode_b64 "task_id not active")"
termlm-handle-run-task-line "{\"event\":\"error\",\"task_id\":\"task-bad-protocol\",\"kind\":\"bad_protocol\",\"message_b64\":\"${bad_protocol_b64}\"}"
assert_eq "$_TERMLM_TASK_ID" "" "terminal protocol error should clear active task id"
assert_eq "$_TERMLM_MODE" "normal" "terminal protocol error should leave prompt mode"

typeset -g _STOP_HELPER_COUNT=0
typeset -g _REPORT_DIED_COUNT=0
termlm-stop-helper() {
  _STOP_HELPER_COUNT=$(( _STOP_HELPER_COUNT + 1 ))
}
termlm-report-daemon-died() {
  _REPORT_DIED_COUNT=$(( _REPORT_DIED_COUNT + 1 ))
}

empty_stream="$(mktemp "${TMPDIR:-/tmp}/termlm-empty-stream.XXXXXX")"
exec {empty_fd}<"$empty_stream"
reset_state
_TERMLM_MODE="prompt"
KEYMAP="termlm-prompt"
_TERMLM_TASK_ID="task-helper-eof"
_TERMLM_WAITING_MODEL=1
_ABANDON_COUNT=0
_TEST_DECISION_ARGS=""
_STOP_HELPER_COUNT=0
_REPORT_DIED_COUNT=0
termlm-handle-run-task-stream "$empty_fd"
exec {empty_fd}<&-
rm -f -- "$empty_stream"
assert_eq "$_STOP_HELPER_COUNT" "1" "helper EOF should stop helper state"
assert_eq "$_REPORT_DIED_COUNT" "1" "helper EOF should report daemon death"
assert_eq "$_ABANDON_COUNT" "0" "helper EOF should not send abort through a dead helper"
assert_eq "$_TEST_DECISION_ARGS" "" "helper EOF should not send any decision"
assert_eq "$_TERMLM_TASK_ID" "" "helper EOF should clear active task id"
assert_eq "$_TERMLM_CLOSED_TASK_ID" "task-helper-eof" "helper EOF should ignore late events for the failed task"
assert_eq "$_TERMLM_MODE" "normal" "helper EOF should leave prompt mode"

empty_stream="$(mktemp "${TMPDIR:-/tmp}/termlm-empty-stream.XXXXXX")"
exec {empty_fd}<"$empty_stream"
reset_state
_TERMLM_MODE="session"
_TERMLM_SESSION_MODE=1
KEYMAP="termlm-prompt"
_TERMLM_TASK_ID="task-session-eof"
_TERMLM_WAITING_MODEL=1
_ABANDON_COUNT=0
_STOP_HELPER_COUNT=0
_REPORT_DIED_COUNT=0
termlm-handle-run-task-stream "$empty_fd"
exec {empty_fd}<&-
rm -f -- "$empty_stream"
assert_eq "$_ABANDON_COUNT" "0" "session helper EOF should not send abort through a dead helper"
assert_eq "$_REPORT_DIED_COUNT" "1" "session helper EOF should report daemon death"
assert_eq "$_TERMLM_SESSION_MODE" "1" "session helper EOF should preserve interactive session mode"
assert_eq "$_TERMLM_MODE" "session" "session helper EOF should keep session state"

reset_state
_TERMLM_MODE="prompt"
KEYMAP="termlm-prompt"
_TERMLM_APPROVAL_TASK_ID="task-approval-cancel"
_TEST_DECISION_ARGS=""
termlm-cancel-prompt
assert_contains "$_TEST_DECISION_ARGS" "task-approval-cancel" "escape should abort pending approval"
assert_eq "$_TERMLM_APPROVAL_TASK_ID" "" "escape should clear approval state"
assert_eq "$_TERMLM_MODE" "normal" "escape should exit after approval cancel"

reset_state
_TERMLM_MODE="session"
_TERMLM_SESSION_MODE=1
KEYMAP="termlm-prompt"
_TERMLM_TASK_ID="task-session-cancel"
_TEST_DECISION_ARGS=""
termlm-cancel-prompt
assert_contains "$_TEST_DECISION_ARGS" "task-session-cancel" "escape should abort active session task"
assert_eq "$_TERMLM_SESSION_MODE" "0" "escape should exit session mode"
assert_eq "$_TERMLM_MODE" "normal" "escape should return to normal mode from session"

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
assert_contains "${(j:|:)_ZLE_CALLS}" ".accept-line" "typed command should execute immediately after implicit abort"

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
assert_contains "${(j:|:)_ZLE_CALLS}" ".accept-line" "session implicit command should execute immediately after implicit abort"

reset_state
_TERMLM_MODE="prompt"
_TEST_APPROVAL_DECISION="approved"
termlm-handle-proposed-event "task-approved" "echo hi" "true"
assert_eq "$_TERMLM_APPROVAL_TASK_ID" "task-approved" "proposed command should enter approval state"
termlm-handle-approval-key "y"
assert_eq "$_TERMLM_PENDING_TASK_ID" "task-approved" "approved command should set pending task id"
assert_eq "$_TERMLM_PENDING_CMD" "echo hi" "approved command should set pending command"
assert_eq "$BUFFER" "echo hi" "approved command should execute the visible command via BUFFER"
assert_contains "${(j:|:)_ZLE_CALLS}" ".accept-line" "approved command should execute with zle .accept-line"
_ZLE_CALLS=()
_TERMLM_ACKED_PENDING_TASK_ID="task-approved"
_TERMLM_PENDING_TASK_ID=""
_TERMLM_TASK_ID="task-approved"
termlm-handle-run-task-line '{"event":"task_complete","task_id":"task-approved"}'
assert_not_contains "${(j:|:)_ZLE_CALLS}" "reset-prompt" "task_complete after approved command should not redraw an extra prompt"
assert_eq "$_TERMLM_ACKED_PENDING_TASK_ID" "" "task_complete should clear acked pending task marker"

reset_state
_TERMLM_MODE="prompt"
_TEST_APPROVAL_DECISION="rejected"
termlm-handle-proposed-event "task-rejected" "echo hi" "true"
assert_eq "$_TERMLM_APPROVAL_TASK_ID" "task-rejected" "rejected command should start in approval state"
termlm-handle-approval-key "n"
assert_eq "$_TERMLM_WAITING_MODEL" "0" "rejected command should stop waiting for model"
assert_eq "$_TERMLM_APPROVAL_TASK_ID" "" "rejected command should clear approval state"
assert_eq "$_TERMLM_MODE" "normal" "rejected command should leave prompt mode"

reset_state
_TERMLM_MODE="prompt"
KEYMAP="termlm-prompt"
_TEST_APPROVAL_DECISION="rejected"
termlm-handle-proposed-event "task-reject-send-fail" "echo hi" "true"
_TEST_DECISION_FAIL=1
termlm-handle-approval-key "n"
_TEST_DECISION_FAIL=0
assert_eq "$_TERMLM_WAITING_MODEL" "0" "failed rejection send should not keep waiting for model"
assert_eq "$_TERMLM_APPROVAL_TASK_ID" "" "failed rejection send should clear approval state"
assert_eq "$_TERMLM_MODE" "normal" "failed rejection send should leave prompt mode cleanly"

reset_state
_TERMLM_MODE="prompt"
_TERMLM_TASK_ID="task-clarify"
_TERMLM_CLARIFICATION_TASK_ID="task-clarify"
BUFFER="use mkdir -p archive"
_TEST_DECISION_ARGS=""
termlm-accept-line
assert_contains "$_TEST_DECISION_ARGS" "--decision clarification" "clarification reply should be sent as user_response"
assert_contains "$_TEST_DECISION_ARGS" "--text use mkdir -p archive" "clarification reply should include prompt text"
assert_eq "$_TERMLM_CLARIFICATION_TASK_ID" "" "clarification state should clear after reply"
assert_eq "$_TERMLM_WAITING_MODEL" "1" "clarification reply should resume model wait"

reset_state
model_b64="$(encode_b64 "Note: indexing")"
termlm-handle-run-task-line "{\"event\":\"model_text\",\"task_id\":\"task-output\",\"chunk_b64\":\"${model_b64}\"}"
assert_eq "$_TERMLM_OUTPUT_STARTED" "1" "model text should mark async output as started"
assert_eq "$_TERMLM_OUTPUT_NEEDS_NEWLINE" "1" "model text without trailing newline should request newline before next event"
question_b64="$(encode_b64 "Clarify this")"
termlm-handle-run-task-line "{\"event\":\"needs_clarification\",\"task_id\":\"task-output\",\"question_b64\":\"${question_b64}\"}"
assert_eq "$_TERMLM_OUTPUT_NEEDS_NEWLINE" "0" "clarification should finish the previous model text line"
assert_eq "$_TERMLM_CLARIFICATION_TASK_ID" "task-output" "clarification should remain editable after output formatting"

reset_state
_TERMLM_MODE="prompt"
_TEST_APPROVAL_DECISION="abort"
termlm-handle-proposed-event "task-abort" "echo hi" "true"
termlm-handle-approval-key $'\x1b'
assert_eq "$_TERMLM_WAITING_MODEL" "0" "aborted command should stop waiting"
assert_eq "$_TERMLM_TASK_ID" "" "aborted command should clear active task id"

tmp_cfg_dir="$(mktemp -d "${TMPDIR:-/tmp}/termlm-runtime-ollama.XXXXXX")"
tmp_cfg="${tmp_cfg_dir}/config.toml"
mock_client="${tmp_cfg_dir}/mock-client.zsh"
cat > "$tmp_cfg" <<'CFG'
[inference]
provider = "ollama"

[ollama]
endpoint = "http://127.0.0.1:11434"
CFG
cat > "$mock_client" <<'EOF'
#!/usr/bin/env zsh
set -euo pipefail
if [[ "${1:-}" == "status" ]]; then
  mode="${TERMLM_MOCK_STATUS_MODE:-healthy}"
  case "$mode" in
    healthy)
      print -r -- "provider: ollama"
      print -r -- "provider_healthy: true"
      exit 0
      ;;
    unhealthy)
      print -r -- "provider: ollama"
      print -r -- "provider_healthy: false"
      exit 0
      ;;
    fail)
      exit 1
      ;;
  esac
fi
exit 0
EOF
chmod +x "$mock_client"

TERMLM_CONFIG_PATH="$tmp_cfg"
termlm-client-bin() {
  print -r -- "$mock_client"
}

_TERMLM_NO_LLM_WARNING_SHOWN=0
export TERMLM_MOCK_STATUS_MODE="unhealthy"
warn_file="${tmp_cfg_dir}/warn.out"
termlm-maybe-warn-no-llm-provider > "$warn_file"
warn_out="$(<"$warn_file")"
assert_contains "$warn_out" "no configured LLM provider is available" "unhealthy ollama status should emit startup warning"
assert_contains "$warn_out" "https://github.com/thtmnisamnstr/termlm/blob/main/docs/configuration.md#use-ollama-for-generation-local-embeddings-still-default" "startup warning should include full docs URL"
termlm-maybe-warn-no-llm-provider > "$warn_file"
warn_again="$(<"$warn_file")"
assert_eq "$warn_again" "" "startup warning should only print once per shell session"

_TERMLM_NO_LLM_WARNING_SHOWN=0
export TERMLM_MOCK_STATUS_MODE="healthy"
termlm-maybe-warn-no-llm-provider > "$warn_file"
healthy_warn="$(<"$warn_file")"
assert_eq "$healthy_warn" "" "healthy ollama provider should not emit startup warning"

_TERMLM_NO_LLM_WARNING_SHOWN=0
export TERMLM_MOCK_STATUS_MODE="fail"
termlm-maybe-warn-no-llm-provider > "$warn_file"
failed_warn="$(<"$warn_file")"
assert_contains "$failed_warn" "configured provider=ollama could not be reached" "status failure should emit ollama unavailable warning"

missing_models_dir="${tmp_cfg_dir}/missing-models"
mkdir -p "$missing_models_dir"
cat > "$tmp_cfg" <<CFG
[inference]
provider = "local"

[model]
variant = "E4B"
models_dir = "${missing_models_dir}"
e4b_filename = "gemma-4-E4B-it-Q4_K_M.gguf"
CFG

_TERMLM_NO_LLM_WARNING_SHOWN=0
termlm-maybe-warn-no-llm-provider > "$warn_file"
missing_warn="$(<"$warn_file")"
assert_contains "$missing_warn" "bundled local model is missing" "missing local bundled model should emit startup warning"
rm -rf -- "$tmp_cfg_dir"

print -r -- "zsh runtime behavior checks passed."
