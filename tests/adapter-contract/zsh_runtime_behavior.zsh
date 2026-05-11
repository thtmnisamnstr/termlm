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
  _TERMLM_CLARIFICATION_TASK_ID=""
  _TERMLM_APPROVAL_TASK_ID=""
  _TERMLM_APPROVAL_CMD=""
  _TERMLM_EDITING_APPROVAL_TASK_ID=""
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

typeset -g _TEST_DECISION_ARGS=""
termlm-send-decision() {
  _TEST_DECISION_ARGS="$*"
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
assert_eq "$_TERMLM_APPROVAL_TASK_ID" "task-approved" "proposed command should enter approval state"
termlm-handle-approval-key "y"
assert_eq "$_TERMLM_PENDING_TASK_ID" "task-approved" "approved command should set pending task id"
assert_eq "$_TERMLM_PENDING_CMD" "echo hi" "approved command should set pending command"
assert_eq "$BUFFER" "wrapped:echo hi:1" "approved command should execute wrapped command via BUFFER"
assert_contains "${(j:|:)_ZLE_CALLS}" ".accept-line" "approved command should execute with zle .accept-line"

reset_state
_TERMLM_MODE="prompt"
_TEST_APPROVAL_DECISION="rejected"
termlm-handle-proposed-event "task-rejected" "echo hi" "true"
assert_eq "$_TERMLM_APPROVAL_TASK_ID" "task-rejected" "rejected command should start in approval state"
termlm-handle-approval-key "n"
assert_eq "$_TERMLM_WAITING_MODEL" "1" "rejected command should continue waiting for model"
assert_eq "$_TERMLM_MODE" "prompt" "rejected command should remain in prompt mode"

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
