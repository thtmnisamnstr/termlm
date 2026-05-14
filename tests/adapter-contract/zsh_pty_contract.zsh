#!/usr/bin/env zsh
set -euo pipefail

if ! command -v expect >/dev/null 2>&1; then
  print -r -- "pty-contract failure: expect is required" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termlm-pty-contract.XXXXXX")"
MOCK_CLIENT="${TMP_ROOT}/mock-termlm-client.sh"
MOCK_LOG="${TMP_ROOT}/mock-bridge.log"
EXPECT_LOG="${TMP_ROOT}/expect.log"
ZDOTDIR_PATH="${TMP_ROOT}/zdotdir"
STDOUT_CAPTURE="${TMP_ROOT}/stdout.decoded"
FAKE_BIN_DIR="${TMP_ROOT}/path-bin"

cleanup() {
  rm -rf -- "${TMP_ROOT}"
}
trap cleanup EXIT

fail() {
  print -r -- "pty-contract failure: $*" >&2
  if [[ -f "${MOCK_LOG}" ]]; then
    print -r -- "--- mock bridge log ---" >&2
    cat "${MOCK_LOG}" >&2 || true
  fi
  exit 1
}

mkdir -p -- "${ZDOTDIR_PATH}"
: > "${MOCK_LOG}"
mkdir -p -- "${TMP_ROOT}/xdg-config"
mkdir -p -- "${FAKE_BIN_DIR}"

cat > "${FAKE_BIN_DIR}/termlm" <<'EOF'
#!/usr/bin/env zsh
set -euo pipefail

if [[ "${1:-}" == "upgrade" ]]; then
  print -r -- "fake-upgrade-ok"
  exit 0
fi
exit 0
EOF
chmod +x "${FAKE_BIN_DIR}/termlm"

cat > "${MOCK_CLIENT}" <<'EOF'
#!/usr/bin/env zsh
set -euo pipefail

log_file="${TERMLM_MOCK_LOG:?missing TERMLM_MOCK_LOG}"

json_field() {
  local line="$1"
  local key="$2"
  local pattern="\"${key}\":\"([^\"]*)\""
  if [[ "$line" =~ $pattern ]]; then
    print -r -- "${match[1]}"
  fi
}

encode_b64() {
  local input="$1"
  print -rn -- "$input" | base64 | tr -d '\n'
}

cmd="${1:-}"
case "$cmd" in
  status)
    exit 0
    ;;
  bridge)
    shell_id="00000000-0000-0000-0000-00000000a001"
    print -r -- "{\"event\":\"shell_registered\",\"shell_id\":\"${shell_id}\"}"
    print -r -- "event:shell_registered:${shell_id}" >> "${log_file}"
    pending_task=""
    while IFS= read -r line; do
      print -r -- "recv:${line}" >> "${log_file}"
      if [[ "$line" == *'"op":"start_task"'* ]]; then
        task_id="$(json_field "$line" "task_id")"
        mode="$(json_field "$line" "mode")"
        prompt="$(json_field "$line" "prompt")"
        print -r -- "event:start_task:${task_id}:${mode}" >> "${log_file}"
        print -r -- "{\"event\":\"task_started\",\"task_id\":\"${task_id}\"}"
        if [[ "$prompt" == "cancel me" ]]; then
          pending_task="${task_id}"
        elif [[ "$mode" == "/p" ]]; then
          chunk_b64="$(encode_b64 "session-mode-ok")"
          print -r -- "{\"event\":\"model_text\",\"task_id\":\"${task_id}\",\"chunk_b64\":\"${chunk_b64}\"}"
          print -r -- "{\"event\":\"task_complete\",\"task_id\":\"${task_id}\"}"
        elif [[ "$prompt" == "long command contract" ]]; then
          cmd_text="mkdir -p \\$HOME/Desktop/md && find \\$HOME/Desktop -path \\$HOME/Desktop/md -prune -o -type f \\( -iname '*.md' -o -iname '*.markdown' \\) -print0 | xargs -0 -I{} cp -p {} \\$HOME/Desktop/md/"
          cmd_b64="$(encode_b64 "${cmd_text}")"
          print -r -- "{\"event\":\"proposed_command\",\"task_id\":\"${task_id}\",\"cmd_b64\":\"${cmd_b64}\",\"requires_approval\":true}"
          pending_task="${task_id}"
        else
          cmd_text='echo pty-contract'
          cmd_b64="$(encode_b64 "${cmd_text}")"
          print -r -- "{\"event\":\"proposed_command\",\"task_id\":\"${task_id}\",\"cmd_b64\":\"${cmd_b64}\",\"requires_approval\":true}"
          pending_task="${task_id}"
        fi
      elif [[ "$line" == *'"op":"ack"'* ]]; then
        task_id="$(json_field "$line" "task_id")"
        print -r -- "event:ack:${task_id}" >> "${log_file}"
        if [[ -n "$pending_task" && "$pending_task" == "$task_id" ]]; then
          print -r -- "{\"event\":\"task_complete\",\"task_id\":\"${task_id}\"}"
          pending_task=""
        fi
      elif [[ "$line" == *'"op":"shell_context"'* ]]; then
        print -r -- "event:shell_context" >> "${log_file}"
      elif [[ "$line" == *'"op":"observe_command"'* ]]; then
        print -r -- "event:observe_command" >> "${log_file}"
      elif [[ "$line" == *'"op":"user_response"'* ]]; then
        decision="$(json_field "$line" "decision")"
        task_id="$(json_field "$line" "task_id")"
        print -r -- "event:user_response:${task_id}:${decision}" >> "${log_file}"
      elif [[ "$line" == *'"op":"unregister_shell"'* ]]; then
        print -r -- "event:unregister_shell" >> "${log_file}"
        break
      fi
    done
    ;;
  *)
    exit 0
    ;;
esac
EOF
chmod +x "${MOCK_CLIENT}"

cat > "${ZDOTDIR_PATH}/.zshrc" <<EOF
export TERMLM_CLIENT_BIN="${MOCK_CLIENT}"
export TERMLM_CORE_BIN="/bin/false"
export TERMLM_MOCK_LOG="${MOCK_LOG}"
export XDG_CONFIG_HOME="${TMP_ROOT}/xdg-config"
export TERMLM_CAPTURE_ENABLED=1
export TERMLM_CAPTURE_MAX_BYTES=16384
export TERMLM_PROMPT_INDICATOR='TERMLM_PROMPT> '
export TERMLM_SESSION_INDICATOR='TERMLM_SESSION> '
export PATH="${FAKE_BIN_DIR}:$PATH"
export PS1='TERMLM_NORMAL> '
bindkey -e
source "${ROOT_DIR}/plugins/zsh/termlm.plugin.zsh"
EOF

wait_for_log_pattern() {
  local pattern="$1"
  local timeout_s="${2:-6}"
  local ticks=$(( timeout_s * 20 ))
  local i
  for (( i = 0; i < ticks; i++ )); do
    if [[ -f "${MOCK_LOG}" ]] && rg -q --fixed-strings -- "${pattern}" "${MOCK_LOG}"; then
      return 0
    fi
    sleep 0.05
  done
  return 1
}

decode_b64_to_file() {
  local encoded="$1"
  if [[ -z "$encoded" ]]; then
    return 1
  fi
  if [[ "$(uname -s)" == "Darwin" ]]; then
    print -rn -- "$encoded" | base64 -D > "${STDOUT_CAPTURE}" 2>/dev/null
  else
    print -rn -- "$encoded" | base64 --decode > "${STDOUT_CAPTURE}" 2>/dev/null
  fi
}

export TERMLM_EXPECT_ZDOTDIR="${ZDOTDIR_PATH}"
export TERMLM_EXPECT_TERM="${TERMLM_TEST_TERM:-xterm-256color}"
if ! expect <<'EOF' >"${EXPECT_LOG}" 2>&1
set timeout 30
set zdotdir $env(TERMLM_EXPECT_ZDOTDIR)
set term $env(TERMLM_EXPECT_TERM)
set send_slow {1 0.02}

spawn env TERM=$term ZDOTDIR=$zdotdir zsh -i
expect -re {TERMLM_NORMAL>}
send -s -- "echo before-termlm\r"
expect -re {before-termlm}
expect -re {TERMLM_NORMAL>}
send -s -- "?"
expect -re {TERMLM_PROMPT>}
send -- "\033"
expect -re {TERMLM_NORMAL>}
send -s -- "?"
expect -re {TERMLM_PROMPT>}
send -s -- "cancel me\r"
expect -re {TERMLM_PROMPT>}
send -- "\033"
expect -re {TERMLM_NORMAL>}
send -s -- "?"
expect -re {TERMLM_PROMPT>}
send -s -- "run pty contract\r"
expect -re {proposed command}
expect -re {y accept.*n/Enter reject.*e edit.*a accept all.*Esc cancel}
send -s -- "y"
expect -re {echo pty-contract}
expect -re {pty-contract}
expect -re {TERMLM_NORMAL>}
send -s -- "?"
expect -re {TERMLM_PROMPT>}
send -s -- "long command contract\r"
expect -re {proposed command}
expect -re {cp -p .*Desktop/md/}
send -s -- "n"
expect -re {TERMLM_NORMAL>}
send -s -- "echo normal-command\r"
expect -re {normal-command}
expect -re {TERMLM_NORMAL>}
send -s -- "termlm upgrade\r"
expect -re {fake-upgrade-ok}
expect -re {TERMLM_NORMAL>}
send -s -- "?"
expect -re {TERMLM_PROMPT>}
send -s -- "run pty contract after upgrade\r"
expect -re {proposed command}
send -s -- "n"
expect -re {TERMLM_NORMAL>}
# Trigger a prompt-cycle so precmd emits pending ack deterministically in PTY automation.
send -- "\r"
expect -re {TERMLM_NORMAL>}
send -s -- "/p\r"
expect -re {TERMLM_SESSION>}
send -s -- "session followup\r"
expect -re {TERMLM_SESSION>}
# Escape should leave interactive session mode, then exit shell. Ctrl-D behavior can vary by TERM/keymap.
send -- "\033"
expect -re {TERMLM_NORMAL>}
send -s -- "exit\r"
expect eof
EOF
then
  unset TERMLM_EXPECT_ZDOTDIR
  unset TERMLM_EXPECT_TERM
  [[ -f "${EXPECT_LOG}" ]] && cat "${EXPECT_LOG}" >&2 || true
  fail "expect PTY run failed"
fi
unset TERMLM_EXPECT_ZDOTDIR
unset TERMLM_EXPECT_TERM

wait_for_log_pattern "event:shell_registered" 8 || fail "shell did not register through bridge"
shell_register_count="$(rg -c --fixed-strings -- "event:shell_registered" "${MOCK_LOG}" || true)"
if (( shell_register_count < 2 )); then
  fail "expected bridge to re-register after termlm upgrade in the same zsh session"
fi
wait_for_log_pattern "event:shell_context" 8 || fail "shell context event not observed"
if rg -q --fixed-strings -- "before-termlm" "${MOCK_LOG}"; then
  fail "normal command before first termlm use should not start helper or enter terminal context"
fi

wait_for_log_pattern "event:start_task" 8 || fail "no start_task observed for prompt mode"
if ! rg -q 'event:start_task:[^:]+:\?' "${MOCK_LOG}"; then
  fail "expected start_task in prompt mode (?)"
fi

wait_for_log_pattern "event:ack" 10 || fail "no ack observed after proposed command execution"
ack_line="$(grep 'recv:.*\"op\":\"ack\"' "${MOCK_LOG}" | head -n 1 || true)"
[[ -n "${ack_line}" ]] || fail "ack payload not captured in mock log"
stdout_b64="$(print -r -- "${ack_line}" | sed -n 's/.*"stdout_b64":"\([^"]*\)".*/\1/p')"
if [[ -n "${stdout_b64}" ]]; then
  decode_b64_to_file "${stdout_b64}" || fail "could not decode ack stdout payload"
  if ! rg -q --fixed-strings -- "pty-contract" "${STDOUT_CAPTURE}"; then
    fail "ack stdout payload did not include executed command output"
  fi
fi

if rg -q --fixed-strings -- '> >(tee "' "${EXPECT_LOG}" \
  || rg -q --fixed-strings -- '( { echo pty-contract; }' "${EXPECT_LOG}"; then
  cat "${EXPECT_LOG}" >&2 || true
  fail "approved command leaked internal capture wrapper into the terminal transcript"
fi

wait_for_log_pattern "event:observe_command" 8 || fail "manual command was not observed without crashing the PTY"

if ! rg -q 'event:start_task:[^:]+:/p' "${MOCK_LOG}"; then
  fail "expected start_task in session mode (/p)"
fi

if ! rg -q 'event:user_response:[^:]+:abort' "${MOCK_LOG}"; then
  fail "expected escape to abort a waiting prompt task"
fi

if ! wait_for_log_pattern "event:unregister_shell" 8; then
  # Allow a final direct check in case the bridge writes unregister right as polling ends.
  # Some zsh/PTY combinations can race the mock's final branch logging even though the raw
  # unregister payload is sent and captured.
  if ! rg -q --fixed-strings -- "event:unregister_shell" "${MOCK_LOG}" \
    && ! rg -q --fixed-strings -- 'recv:{"op":"unregister_shell"}' "${MOCK_LOG}"; then
    fail "shell did not send unregister on exit"
  fi
fi

print -r -- "zsh pty adapter contract checks passed."
