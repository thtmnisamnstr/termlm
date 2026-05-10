#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

PLUGIN_MAIN="${ROOT_DIR}/plugins/zsh/termlm.plugin.zsh"
WIDGET_SELF_INSERT="${ROOT_DIR}/plugins/zsh/widgets/self-insert.zsh"
WIDGET_ACCEPT_LINE="${ROOT_DIR}/plugins/zsh/widgets/accept-line.zsh"
WIDGET_APPROVAL="${ROOT_DIR}/plugins/zsh/widgets/approval.zsh"
WIDGET_SAFETY="${ROOT_DIR}/plugins/zsh/widgets/safety-floor.zsh"
LIB_CAPTURE="${ROOT_DIR}/plugins/zsh/lib/capture.zsh"
LIB_IPC="${ROOT_DIR}/plugins/zsh/lib/ipc.zsh"
LIB_OBSERVER="${ROOT_DIR}/plugins/zsh/lib/terminal-observer.zsh"
RUNTIME_BEHAVIOR="${ROOT_DIR}/tests/adapter-contract/zsh_runtime_behavior.zsh"
PTY_BEHAVIOR="${ROOT_DIR}/tests/adapter-contract/zsh_pty_contract.zsh"

fail() {
  echo "adapter-contract failure: $*" >&2
  exit 1
}

require_file() {
  local file="$1"
  [[ -f "$file" ]] || fail "missing required file: $file"
}

require_pattern() {
  local file="$1"
  local pattern="$2"
  local desc="$3"
  if ! rg -q --pcre2 "$pattern" "$file"; then
    fail "${desc} (pattern '$pattern' not found in ${file})"
  fi
}

require_literal() {
  local file="$1"
  local literal="$2"
  local desc="$3"
  if ! rg -F -q -- "$literal" "$file"; then
    fail "${desc} (literal '$literal' not found in ${file})"
  fi
}

run_zsh_check() {
  local code="$1"
  if ! zsh -fc "$code" >/dev/null 2>&1; then
    fail "zsh runtime check failed: $code"
  fi
}

echo "checking required adapter files..."
require_file "$PLUGIN_MAIN"
require_file "$WIDGET_SELF_INSERT"
require_file "$WIDGET_ACCEPT_LINE"
require_file "$WIDGET_APPROVAL"
require_file "$WIDGET_SAFETY"
require_file "$LIB_CAPTURE"
require_file "$LIB_IPC"
require_file "$LIB_OBSERVER"
require_file "$RUNTIME_BEHAVIOR"
require_file "$PTY_BEHAVIOR"

echo "checking widget registration contract..."
require_pattern "$PLUGIN_MAIN" 'zle -N self-insert termlm-self-insert' "self-insert wrapper must be installed"
require_pattern "$PLUGIN_MAIN" 'zle -N accept-line termlm-accept-line' "accept-line wrapper must be installed"
require_pattern "$PLUGIN_MAIN" 'zle -N zle-line-pre-redraw termlm-line-pre-redraw' "prompt redraw hook must be installed"
require_pattern "$PLUGIN_MAIN" 'autoload -Uz add-zsh-hook' "plugin must autoload add-zsh-hook for clean zsh environments"
require_pattern "$PLUGIN_MAIN" 'add-zsh-hook preexec termlm-preexec' "preexec observer hook must be installed"
require_pattern "$PLUGIN_MAIN" 'add-zsh-hook precmd termlm-precmd' "precmd capture/ack hook must be installed"

echo "checking prompt entry/exit behavior..."
# shellcheck disable=SC2016
require_pattern "$WIDGET_SELF_INSERT" '\[\[ "\$KEYS" == "\?" \]\]' "self-insert must inspect ? trigger"
# shellcheck disable=SC2016
require_pattern "$WIDGET_SELF_INSERT" '\[\[ "\$LBUFFER" == \*"\\\\" \]\]' "self-insert must support escaped literal ?"
require_pattern "$WIDGET_SELF_INSERT" 'termlm-enter-prompt-mode' "self-insert must enter prompt mode"
require_literal "$WIDGET_ACCEPT_LINE" "\"\$BUFFER\" =~ '^/p[[:space:]]*$'" "accept-line must intercept /p"
require_literal "$WIDGET_ACCEPT_LINE" "\"\$BUFFER\" =~ '^/q[[:space:]]*$'" "accept-line must intercept /q"
require_pattern "$WIDGET_ACCEPT_LINE" 'termlm-abandon-active-task' "accept-line must abort pending task on implicit command"

echo "checking approval contract..."
require_pattern "$WIDGET_APPROVAL" 'read -k 1 -s key' "approval must use single-key silent input"
require_pattern "$WIDGET_APPROVAL" '\[y\]es.*\[n\]o\(default\).*\[e\]dit.*\[a\]ll-in-this-task' "approval UI must expose y/n/e/a controls"
require_literal "$WIDGET_APPROVAL" "\$'\\r'|\$'\\n'|\"\") echo \"rejected\"" "Return/default must reject"
require_literal "$WIDGET_APPROVAL" "echo \"edited:\$edited\"" "approval edit flow must return edited command payload"

echo "checking execution/capture contract..."
require_literal "$LIB_CAPTURE" "echo \"( { \$cmd; } > >(tee \\\"\$out\\\") 2> >(tee \\\"\$err\\\" >&2) )\"" "capture wrapper must use subshell tee process substitution"
# shellcheck disable=SC2016
require_pattern "$LIB_IPC" 'BUFFER="\$wrapped"' "approved command must be written into BUFFER"
require_pattern "$LIB_IPC" 'zle \.accept-line' "approved command must execute via zle .accept-line"
require_pattern "$LIB_OBSERVER" '\\\"op\\\":\\\"ack\\\"' "adapter must send Ack messages"
require_pattern "$LIB_OBSERVER" '\\\"op\\\":\\\"observe_command\\\"' "adapter must send observed interactive command events"
require_pattern "$LIB_OBSERVER" 'started_at_ms' "adapter must forward absolute command start timestamps"
# shellcheck disable=SC2016
require_pattern "$LIB_OBSERVER" '_TERMLM_LAST_PREEXEC_EXPANDED:-\$_TERMLM_LAST_PREEXEC_CMD' "observer payload must preserve expanded-command field"
require_pattern "$LIB_OBSERVER" 'termlm-epoch-to-ms' "observer must convert preexec timestamps for IPC"
require_pattern "$LIB_IPC" 'no configured LLM provider is available; agentic features are disabled' "adapter must surface startup warning when no ollama LLM provider is reachable"
require_literal "$LIB_IPC" "https://github.com/thtmnisamnstr/termlm/blob/main/docs/configuration.md#use-ollama-for-generation-local-embeddings-still-default" "startup warning must include full ollama configuration docs URL"
# shellcheck disable=SC2016
require_pattern "$LIB_IPC" 'bridge <"\$fifo_in" >"\$fifo_out"' "adapter must run persistent helper bridge over stdio"
if rg -q --pcre2 '\$client_bin[[:space:]]+(run-task|respond-task|ack-task|send-shell-context|observe-command)\b' "$LIB_IPC" "$LIB_OBSERVER"; then
  fail "adapter must not use one-shot helper subcommands for task/ack/context/observe flow"
fi

echo "checking immutable adapter safety floor..."
require_literal "$WIDGET_SAFETY" "'^[[:space:]]*rm[[:space:]]+-[[:alpha:]]*r[[:alpha:]]*[[:space:]]+/([[:space:]]|$)'" "adapter floor must include rm -rf / guard"
require_literal "$WIDGET_SAFETY" "'^[[:space:]]*:\\(\\)[[:space:]]*\\{[[:space:]]*:[[:space:]]*\\|[[:space:]]*:[[:space:]]*&[[:space:]]*\\}[[:space:]]*;[[:space:]]*:'" "adapter floor must include fork bomb guard"

echo "checking runtime helper behavior..."
run_zsh_check "source \"$WIDGET_SAFETY\"; termlm-safety-floor-match 'rm -rf /' >/dev/null"
run_zsh_check "source \"$LIB_CAPTURE\"; _TERMLM_RUN_DIR='/tmp/termlm-contract'; wrapped=\$(termlm-wrap-command-for-capture 'echo ok' 1); [[ \"\$wrapped\" == *'> >(tee \"'* ]]"
run_zsh_check "source \"$LIB_CAPTURE\"; _TERMLM_CAPTURE_ENABLED=0; wrapped=\$(termlm-wrap-command-for-capture 'echo ok' 1); [[ \"\$wrapped\" == 'echo ok' ]]"
run_zsh_check "source \"$LIB_OBSERVER\"; [[ \"\$(termlm-epoch-to-ms 1.234)\" == '1234' ]]"
run_zsh_check "source \"$LIB_OBSERVER\"; _TERMLM_OBS_CAPTURE_ALL=0; _TERMLM_SHELL_ID='shell'; _TERMLM_PENDING_TASK_ID=''; _TERMLM_RUN_DIR='/tmp/termlm-contract'; mkdir -p \"\$_TERMLM_RUN_DIR\"; termlm-preexec 'll /tmp' 'ls -l /tmp'; [[ \"\$_TERMLM_LAST_PREEXEC_CMD\" == 'll /tmp' && \"\$_TERMLM_LAST_PREEXEC_EXPANDED\" == 'ls -l /tmp' ]]"

echo "checking automated runtime behavior flows..."
runtime_log="$(mktemp "${TMPDIR:-/tmp}/termlm-runtime-contract.XXXXXX")"
pty_log="$(mktemp "${TMPDIR:-/tmp}/termlm-pty-contract.XXXXXX")"
trap 'rm -f -- "$runtime_log" "$pty_log"' EXIT
if ! zsh "$RUNTIME_BEHAVIOR" >"$runtime_log" 2>&1; then
  cat "$runtime_log" >&2 || true
  fail "zsh runtime behavior contract failed"
fi
if ! zsh "$PTY_BEHAVIOR" >"$pty_log" 2>&1; then
  cat "$pty_log" >&2 || true
  fail "zsh PTY behavior contract failed"
fi

echo "zsh adapter contract checks passed."
