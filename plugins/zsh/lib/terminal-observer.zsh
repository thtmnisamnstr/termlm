zmodload zsh/datetime 2>/dev/null || true

typeset -g _TERMLM_LAST_PREEXEC_CMD=""
typeset -g _TERMLM_LAST_PREEXEC_EXPANDED=""
typeset -g _TERMLM_LAST_PREEXEC_TS=0
typeset -g _TERMLM_LAST_PREEXEC_CWD=""
typeset -g _TERMLM_OBS_SEQ=0
typeset -g _TERMLM_OBS_CURRENT_SEQ=0
typeset -g _TERMLM_OBS_EXCLUDE_TUI=1
typeset -g _TERMLM_OBS_CAPTURE_ALL=1
typeset -g _TERMLM_OBS_MAX_BYTES=32768
typeset -g _TERMLM_OBS_CAPTURE_ACTIVE=0
typeset -g _TERMLM_OBS_STDOUT_FILE=""
typeset -g _TERMLM_OBS_STDERR_FILE=""
typeset -g _TERMLM_OBS_SAVE_STDOUT_FD=-1
typeset -g _TERMLM_OBS_SAVE_STDERR_FD=-1
typeset -ga _TERMLM_OBS_EXCLUDE_PATTERNS=(
  '^\s*(env|printenv)(\s|$)'
  '^\s*security\s+find-.*password'
  '^\s*(op|pass)\s+.*(show|get)'
  '^\s*gcloud\s+auth\s+print-access-token'
  '^\s*aws\s+configure\s+get'
)

typeset -ga _TERMLM_OBS_EXCLUDE_COMMANDS=(
  vim nvim vi emacs nano
  less more man
  ssh sftp scp mosh
  top htop btop watch
  fzf sk tmux screen
  node python python3 ruby irb lua julia
  mysql psql sqlite3 redis-cli mongosh
)

termlm-load-observer-settings() {
  local cfg="${XDG_CONFIG_HOME:-$HOME/.config}/termlm/config.toml"

  _TERMLM_OBS_EXCLUDE_TUI=1
  _TERMLM_OBS_CAPTURE_ALL=1
  _TERMLM_OBS_MAX_BYTES=32768
  _TERMLM_OBS_EXCLUDE_PATTERNS=(
    '^\s*(env|printenv)(\s|$)'
    '^\s*security\s+find-.*password'
    '^\s*(op|pass)\s+.*(show|get)'
    '^\s*gcloud\s+auth\s+print-access-token'
    '^\s*aws\s+configure\s+get'
  )
  _TERMLM_OBS_EXCLUDE_COMMANDS=(
    vim nvim vi emacs nano
    less more man
    ssh sftp scp mosh
    top htop btop watch
    fzf sk tmux screen
    node python python3 ruby irb lua julia
    mysql psql sqlite3 redis-cli mongosh
  )

  [[ ! -f "$cfg" ]] && return 0

  local in_terminal=0
  local line
  local parsed_patterns=()
  while IFS= read -r line; do
    if [[ "$line" =~ '^[[:space:]]*\[[^]]+\][[:space:]]*$' ]]; then
      if [[ "$line" == *"[terminal_context]"* ]]; then
        in_terminal=1
      else
        in_terminal=0
      fi
      continue
    fi

    (( in_terminal )) || continue

    if [[ "$line" =~ '^[[:space:]]*capture_all_interactive_commands[[:space:]]*=[[:space:]]*(true|false)' ]]; then
      if [[ "${match[1]}" == "true" ]]; then
        _TERMLM_OBS_CAPTURE_ALL=1
      else
        _TERMLM_OBS_CAPTURE_ALL=0
      fi
      continue
    fi

    if [[ "$line" =~ '^[[:space:]]*max_output_bytes_per_command[[:space:]]*=[[:space:]]*([0-9]+)' ]]; then
      if (( match[1] > 0 )); then
        _TERMLM_OBS_MAX_BYTES="${match[1]}"
      fi
      continue
    fi

    if [[ "$line" =~ '^[[:space:]]*exclude_tui_commands[[:space:]]*=[[:space:]]*(true|false)' ]]; then
      if [[ "${match[1]}" == "true" ]]; then
        _TERMLM_OBS_EXCLUDE_TUI=1
      else
        _TERMLM_OBS_EXCLUDE_TUI=0
      fi
      continue
    fi

    if [[ "$line" =~ "^[[:space:]]*'[^\"]*" ]]; then
      if [[ "$line" =~ "^[[:space:]]*'(.+)'" ]]; then
        parsed_patterns+=("${match[1]}")
      fi
      continue
    fi
  done < "$cfg"

  if (( ${#parsed_patterns[@]} > 0 )); then
    _TERMLM_OBS_EXCLUDE_PATTERNS=("${parsed_patterns[@]}")
  fi
}

termlm-should-observe-command() {
  local cmd="$1"
  if (( _TERMLM_OBS_EXCLUDE_TUI == 1 )); then
    local first="${${(z)cmd}[1]}"
    case " ${_TERMLM_OBS_EXCLUDE_COMMANDS[*]} " in
      *" ${first} "*)
        return 1
        ;;
      *)
        ;;
    esac
  fi

  local pat
  for pat in "${_TERMLM_OBS_EXCLUDE_PATTERNS[@]}"; do
    if [[ "$cmd" =~ "$pat" ]]; then
      return 1
    fi
  done

  return 0
}

termlm-epoch-to-ms() {
  local ts="$1"
  [[ -z "$ts" || "$ts" == "0" ]] && return 1
  if [[ ! "$ts" =~ '^[0-9]+(\.[0-9]+)?$' ]]; then
    return 1
  fi
  local -i ms=0
  (( ms = ts * 1000.0 ))
  if (( ms < 0 )); then
    ms=0
  fi
  print -r -- "$ms"
}

termlm-epoch-delta-ms() {
  local start="$1"
  local end="$2"
  [[ -z "$start" || -z "$end" || "$start" == "0" || "$end" == "0" ]] && return 1
  if [[ ! "$start" =~ '^[0-9]+(\.[0-9]+)?$' || ! "$end" =~ '^[0-9]+(\.[0-9]+)?$' ]]; then
    return 1
  fi
  local -i ms=0
  (( ms = (end - start) * 1000.0 ))
  if (( ms < 0 )); then
    ms=0
  fi
  print -r -- "$ms"
}

termlm-observer-start-capture() {
  local seq="$1"
  [[ -z "$seq" || "$seq" == "0" ]] && return 1

  if (( _TERMLM_OBS_CAPTURE_ACTIVE == 1 )); then
    termlm-observer-stop-capture
  fi

  _TERMLM_OBS_STDOUT_FILE="${_TERMLM_RUN_DIR}/obs.stdout.${seq}"
  _TERMLM_OBS_STDERR_FILE="${_TERMLM_RUN_DIR}/obs.stderr.${seq}"

  : >| "$_TERMLM_OBS_STDOUT_FILE" 2>/dev/null || return 1
  : >| "$_TERMLM_OBS_STDERR_FILE" 2>/dev/null || return 1

  local save_out save_err
  exec {save_out}>&1 || return 1
  exec {save_err}>&2 || {
    eval "exec ${save_out}>&-" 2>/dev/null || true
    return 1
  }
  _TERMLM_OBS_SAVE_STDOUT_FD="$save_out"
  _TERMLM_OBS_SAVE_STDERR_FD="$save_err"

  exec > >(tee "$_TERMLM_OBS_STDOUT_FILE" >&${_TERMLM_OBS_SAVE_STDOUT_FD}) || {
    termlm-observer-stop-capture
    return 1
  }
  exec 2> >(tee "$_TERMLM_OBS_STDERR_FILE" >&${_TERMLM_OBS_SAVE_STDERR_FD}) || {
    termlm-observer-stop-capture
    return 1
  }

  _TERMLM_OBS_CAPTURE_ACTIVE=1
  return 0
}

termlm-observer-stop-capture() {
  if [[ "$_TERMLM_OBS_SAVE_STDOUT_FD" != "-1" ]]; then
    eval "exec 1>&${_TERMLM_OBS_SAVE_STDOUT_FD}" 2>/dev/null || true
    eval "exec ${_TERMLM_OBS_SAVE_STDOUT_FD}>&-" 2>/dev/null || true
  fi
  if [[ "$_TERMLM_OBS_SAVE_STDERR_FD" != "-1" ]]; then
    eval "exec 2>&${_TERMLM_OBS_SAVE_STDERR_FD}" 2>/dev/null || true
    eval "exec ${_TERMLM_OBS_SAVE_STDERR_FD}>&-" 2>/dev/null || true
  fi
  _TERMLM_OBS_CAPTURE_ACTIVE=0
  _TERMLM_OBS_SAVE_STDOUT_FD=-1
  _TERMLM_OBS_SAVE_STDERR_FD=-1
}

termlm-preexec() {
  _TERMLM_LAST_PREEXEC_CMD="${1:-}"
  if [[ -n "${2:-}" ]]; then
    _TERMLM_LAST_PREEXEC_EXPANDED="$2"
  elif [[ -n "${3:-}" ]]; then
    _TERMLM_LAST_PREEXEC_EXPANDED="$3"
  else
    _TERMLM_LAST_PREEXEC_EXPANDED="$_TERMLM_LAST_PREEXEC_CMD"
  fi
  _TERMLM_LAST_PREEXEC_TS=$EPOCHREALTIME
  _TERMLM_LAST_PREEXEC_CWD="$PWD"
  _TERMLM_OBS_CURRENT_SEQ=$(( _TERMLM_OBS_SEQ + 1 ))

  if [[ -z "$_TERMLM_SHELL_ID" || -n "$_TERMLM_PENDING_TASK_ID" ]]; then
    return
  fi
  if (( _TERMLM_OBS_CAPTURE_ALL != 1 )); then
    return
  fi
  if ! termlm-should-observe-command "$_TERMLM_LAST_PREEXEC_CMD"; then
    return
  fi

  termlm-observer-start-capture "$_TERMLM_OBS_CURRENT_SEQ" >/dev/null 2>&1 || true
}

termlm-precmd() {
  local last_status=$?
  termlm-send-shell-context
  termlm-load-capture-settings
  termlm-load-observer-settings

  local was_pending=0
  if [[ -n "$_TERMLM_PENDING_TASK_ID" ]]; then
    was_pending=1
    local now="$EPOCHREALTIME"
    local elapsed_ms=0

    if [[ -n "$_TERMLM_PENDING_STARTED_AT" && "$_TERMLM_PENDING_STARTED_AT" != "0" ]]; then
      elapsed_ms="$(termlm-epoch-delta-ms "$_TERMLM_PENDING_STARTED_AT" "$now" 2>/dev/null || print -r -- 0)"
    fi

    local stdout_b64=""
    local stderr_b64=""
    local stdout_truncated=0
    local stderr_truncated=0
    if termlm-capture-enabled; then
      if [[ -n "$_TERMLM_PENDING_STDOUT_FILE" && -f "$_TERMLM_PENDING_STDOUT_FILE" ]]; then
        local -a stdout_payload
        stdout_payload=("${(@f)$(termlm-read-captured-file-b64 "$_TERMLM_PENDING_STDOUT_FILE" "$_TERMLM_CAPTURE_MAX_BYTES")}")
        stdout_b64="${stdout_payload[1]}"
        stdout_truncated="${stdout_payload[2]:-0}"
      fi
      if [[ -n "$_TERMLM_PENDING_STDERR_FILE" && -f "$_TERMLM_PENDING_STDERR_FILE" ]]; then
        local -a stderr_payload
        stderr_payload=("${(@f)$(termlm-read-captured-file-b64 "$_TERMLM_PENDING_STDERR_FILE" "$_TERMLM_CAPTURE_MAX_BYTES")}")
        stderr_b64="${stderr_payload[1]}"
        stderr_truncated="${stderr_payload[2]:-0}"
      fi
    fi
    [[ -n "$_TERMLM_PENDING_STDOUT_FILE" ]] && rm -f -- "$_TERMLM_PENDING_STDOUT_FILE"
    [[ -n "$_TERMLM_PENDING_STDERR_FILE" ]] && rm -f -- "$_TERMLM_PENDING_STDERR_FILE"

    local started_at_ms=""
    started_at_ms="$(termlm-epoch-to-ms "$_TERMLM_PENDING_STARTED_AT" 2>/dev/null || true)"
    local ack_json
    ack_json="{\"op\":\"ack\",\"task_id\":\"$(termlm-json-escape "$_TERMLM_PENDING_TASK_ID")\",\"command_seq\":${_TERMLM_PENDING_SEQ},\"command\":\"$(termlm-json-escape "$_TERMLM_PENDING_CMD")\",\"cwd_before\":\"$(termlm-json-escape "$_TERMLM_PENDING_CWD_BEFORE")\",\"cwd_after\":\"$(termlm-json-escape "$PWD")\",\"exit_status\":${last_status},\"elapsed_ms\":${elapsed_ms},\"stdout_truncated\":$([[ $stdout_truncated -eq 1 ]] && echo true || echo false),\"stderr_truncated\":$([[ $stderr_truncated -eq 1 ]] && echo true || echo false)"
    if [[ -n "$started_at_ms" ]]; then
      ack_json+=",\"started_at_ms\":${started_at_ms}"
    fi
    if [[ -n "$stdout_b64" ]]; then
      ack_json+=",\"stdout_b64\":\"${stdout_b64}\""
    fi
    if [[ -n "$stderr_b64" ]]; then
      ack_json+=",\"stderr_b64\":\"${stderr_b64}\""
    fi
    ack_json+="}"
    local ack_ok=0
    if termlm-helper-send "$ack_json" >/dev/null 2>&1; then
      ack_ok=1
      _TERMLM_WAITING_MODEL=1
    else
      _TERMLM_WAITING_MODEL=0
      zle -M "termlm: connection lost" 2>/dev/null || print -r -- "termlm: connection lost"
      if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
        termlm-exit-prompt-mode
      else
        zle reset-prompt
      fi
    fi
    _TERMLM_PENDING_TASK_ID=""
    _TERMLM_PENDING_CMD=""
    _TERMLM_PENDING_CWD_BEFORE=""
    _TERMLM_PENDING_STARTED_AT=0
    _TERMLM_PENDING_STDOUT_FILE=""
    _TERMLM_PENDING_STDERR_FILE=""
    if (( ack_ok == 0 )); then
      _TERMLM_TASK_ID=""
    fi
    _TERMLM_OBS_CURRENT_SEQ=0
  fi

  if [[ -n "$_TERMLM_LAST_PREEXEC_CMD" && -n "$_TERMLM_SHELL_ID" && $was_pending -eq 0 ]]; then
    local now="$EPOCHREALTIME"
    local dur_ms=0
    if [[ -n "$_TERMLM_LAST_PREEXEC_TS" && "$_TERMLM_LAST_PREEXEC_TS" != "0" ]]; then
      dur_ms="$(termlm-epoch-delta-ms "$_TERMLM_LAST_PREEXEC_TS" "$now" 2>/dev/null || print -r -- 0)"
    fi

    local seq="${_TERMLM_OBS_CURRENT_SEQ:-0}"
    if [[ "$seq" == "0" ]]; then
      seq=$(( _TERMLM_OBS_SEQ + 1 ))
    fi
    _TERMLM_OBS_SEQ="$seq"

    local should_capture=1
    local capture_status="skipped_interactive_tty"
    if ! termlm-should-observe-command "$_TERMLM_LAST_PREEXEC_CMD"; then
      should_capture=0
      capture_status="excluded_interactive"
    fi

    local had_capture_active=0
    if (( _TERMLM_OBS_CAPTURE_ACTIVE == 1 )); then
      had_capture_active=1
      termlm-observer-stop-capture
    fi

    local stdout_b64=""
    local stderr_b64=""
    local stdout_truncated=0
    local stderr_truncated=0
    if (( should_capture == 1 && had_capture_active == 1 )); then
      capture_status="captured"
      if [[ -n "$_TERMLM_OBS_STDOUT_FILE" && -f "$_TERMLM_OBS_STDOUT_FILE" ]]; then
        local -a stdout_payload
        stdout_payload=("${(@f)$(termlm-read-captured-file-b64 "$_TERMLM_OBS_STDOUT_FILE" "$_TERMLM_OBS_MAX_BYTES")}")
        stdout_b64="${stdout_payload[1]}"
        stdout_truncated="${stdout_payload[2]:-0}"
      fi
      if [[ -n "$_TERMLM_OBS_STDERR_FILE" && -f "$_TERMLM_OBS_STDERR_FILE" ]]; then
        local -a stderr_payload
        stderr_payload=("${(@f)$(termlm-read-captured-file-b64 "$_TERMLM_OBS_STDERR_FILE" "$_TERMLM_OBS_MAX_BYTES")}")
        stderr_b64="${stderr_payload[1]}"
        stderr_truncated="${stderr_payload[2]:-0}"
      fi
    fi

    [[ -n "$_TERMLM_OBS_STDOUT_FILE" ]] && rm -f -- "$_TERMLM_OBS_STDOUT_FILE"
    [[ -n "$_TERMLM_OBS_STDERR_FILE" ]] && rm -f -- "$_TERMLM_OBS_STDERR_FILE"
    _TERMLM_OBS_STDOUT_FILE=""
    _TERMLM_OBS_STDERR_FILE=""

    local started_at_ms=""
    started_at_ms="$(termlm-epoch-to-ms "$_TERMLM_LAST_PREEXEC_TS" 2>/dev/null || true)"
    local observe_json
    observe_json="{\"op\":\"observe_command\",\"command_seq\":${_TERMLM_OBS_SEQ},\"raw_command\":\"$(termlm-json-escape "$_TERMLM_LAST_PREEXEC_CMD")\",\"expanded_command\":\"$(termlm-json-escape "${_TERMLM_LAST_PREEXEC_EXPANDED:-$_TERMLM_LAST_PREEXEC_CMD}")\",\"cwd_before\":\"$(termlm-json-escape "${_TERMLM_LAST_PREEXEC_CWD:-$PWD}")\",\"cwd_after\":\"$(termlm-json-escape "$PWD")\",\"exit_status\":${last_status},\"duration_ms\":${dur_ms},\"output_capture_status\":\"$(termlm-json-escape "$capture_status")\",\"stdout_truncated\":$([[ $stdout_truncated -eq 1 ]] && echo true || echo false),\"stderr_truncated\":$([[ $stderr_truncated -eq 1 ]] && echo true || echo false)"
    if [[ -n "$started_at_ms" ]]; then
      observe_json+=",\"started_at_ms\":${started_at_ms}"
    fi
    if [[ -n "$stdout_b64" ]]; then
      observe_json+=",\"stdout_b64\":\"${stdout_b64}\""
    fi
    if [[ -n "$stderr_b64" ]]; then
      observe_json+=",\"stderr_b64\":\"${stderr_b64}\""
    fi
    observe_json+="}"
    termlm-helper-send "$observe_json" >/dev/null 2>&1 || true
  fi

  _TERMLM_LAST_PREEXEC_CMD=""
  _TERMLM_LAST_PREEXEC_EXPANDED=""
  _TERMLM_LAST_PREEXEC_TS=0
  _TERMLM_LAST_PREEXEC_CWD=""
  _TERMLM_OBS_CURRENT_SEQ=0
}

termlm-zshexit() {
  if [[ -n "${_TERMLM_TASK_ID:-}" ]]; then
    termlm-abort-task "$_TERMLM_TASK_ID" >/dev/null 2>&1 || true
  fi
  termlm-observer-stop-capture
  _TERMLM_WAITING_MODEL=0
  _TERMLM_TASK_ID=""
  zle -K main 2>/dev/null || true
  PS1="${_TERMLM_SAVED_PS1:-$PS1}"
  _TERMLM_MODE="normal"
  _TERMLM_SESSION_MODE=0

  if [[ -n "${_TERMLM_PENDING_TASK_ID:-}" ]]; then
    termlm-abort-task "$_TERMLM_PENDING_TASK_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${_TERMLM_SHELL_ID:-}" ]]; then
    termlm-helper-send '{"op":"unregister_shell"}' >/dev/null 2>&1 || true
  fi
  termlm-stop-helper
  _TERMLM_SHELL_ID=""
  rm -rf -- "$_TERMLM_RUN_DIR"
}
