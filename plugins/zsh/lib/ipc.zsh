termlm-json-field() {
  local line="$1"
  local key="$2"
  local pattern
  pattern="\"${key}\":\"([^\"]*)\""
  if [[ "$line" =~ $pattern ]]; then
    echo "${match[1]}"
  fi
}

termlm-json-bool-field() {
  local line="$1"
  local key="$2"
  local pattern
  pattern="\"${key}\":(true|false)"
  if [[ "$line" =~ $pattern ]]; then
    echo "${match[1]}"
  fi
}

termlm-json-event-kind() {
  termlm-json-field "$1" "event"
}

termlm-base64-decode() {
  local encoded="$1"
  [[ -z "$encoded" ]] && return 0
  if [[ "$(uname -s)" == "Darwin" ]]; then
    print -rn -- "$encoded" | base64 -D 2>/dev/null
  else
    print -rn -- "$encoded" | base64 --decode 2>/dev/null
  fi
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

termlm-helper-is-alive() {
  local pid="${_TERMLM_HELPER_PID:-}"
  [[ -n "$pid" ]] && [[ -n "${_TERMLM_HELPER_IN_FD:-}" ]] && [[ -n "${_TERMLM_HELPER_OUT_FD:-}" ]] && kill -0 "$pid" 2>/dev/null
}

termlm-config-path() {
  if [[ -n "${TERMLM_CONFIG_PATH:-}" ]]; then
    print -r -- "${TERMLM_CONFIG_PATH}"
    return
  fi
  print -r -- "${XDG_CONFIG_HOME:-$HOME/.config}/termlm/config.toml"
}

termlm-config-section-value() {
  local section="$1"
  local key="$2"
  local fallback="$3"
  local cfg
  cfg="$(termlm-config-path)"
  [[ -f "$cfg" ]] || {
    print -r -- "$fallback"
    return
  }

  local in_section=0
  local value="$fallback"
  local line
  while IFS= read -r line; do
    if [[ "$line" =~ '^[[:space:]]*\[([^]]+)\][[:space:]]*$' ]]; then
      if [[ "${match[1]}" == "$section" ]]; then
        in_section=1
      else
        in_section=0
      fi
      continue
    fi
    (( in_section )) || continue

    if [[ "$line" =~ "^[[:space:]]*${key}[[:space:]]*=[[:space:]]*\\\"([^\\\"]+)\\\"" ]]; then
      value="${match[1]}"
      break
    elif [[ "$line" =~ "^[[:space:]]*${key}[[:space:]]*=[[:space:]]*([^#[:space:]]+)" ]]; then
      value="${match[1]}"
      break
    fi
  done < "$cfg"

  print -r -- "$value"
}

termlm-expand-home-path() {
  local path="$1"
  if [[ "$path" == "~/"* ]]; then
    print -r -- "$HOME/${path#\~/}"
  else
    print -r -- "$path"
  fi
}

termlm-config-inference-provider() {
  termlm-config-section-value "inference" "provider" "local"
}

termlm-config-ollama-endpoint() {
  termlm-config-section-value "ollama" "endpoint" "http://127.0.0.1:11434"
}

termlm-selected-local-model-path() {
  local models_dir variant filename
  models_dir="$(termlm-config-section-value "model" "models_dir" "~/.local/share/termlm/models")"
  models_dir="$(termlm-expand-home-path "$models_dir")"
  variant="$(termlm-config-section-value "model" "variant" "E4B" | tr '[:lower:]' '[:upper:]')"

  if [[ "$variant" == "E2B" ]]; then
    filename="$(termlm-config-section-value "model" "e2b_filename" "gemma-4-E2B-it-Q4_K_M.gguf")"
  else
    filename="$(termlm-config-section-value "model" "e4b_filename" "gemma-4-E4B-it-Q4_K_M.gguf")"
  fi

  print -r -- "${models_dir}/${filename}"
}

termlm-warn-no-llm-provider() {
  local detail="$1"
  local docs_url="https://github.com/thtmnisamnstr/termlm/blob/main/docs/configuration.md#use-ollama-for-generation-local-embeddings-still-default"
  print -r -- "termlm: no configured LLM provider is available; agentic features are disabled until an LLM is configured."
  if [[ -n "$detail" ]]; then
    print -r -- "termlm: $detail"
  fi
  print -r -- "termlm: configure Ollama in: ${docs_url}"
}

termlm-maybe-warn-no-llm-provider() {
  if [[ "${_TERMLM_NO_LLM_WARNING_SHOWN:-0}" == "1" ]]; then
    return 0
  fi

  local configured_provider
  configured_provider="$(termlm-config-inference-provider)"

  if [[ "$configured_provider" == "local" ]]; then
    local local_model_path
    local_model_path="$(termlm-selected-local-model-path)"
    if [[ ! -f "$local_model_path" ]]; then
      termlm-warn-no-llm-provider "bundled local model is missing at ${local_model_path}."
      _TERMLM_NO_LLM_WARNING_SHOWN=1
      return 0
    fi
  fi

  if [[ "$configured_provider" != "local" && "$configured_provider" != "ollama" ]]; then
    termlm-warn-no-llm-provider "configured inference provider '${configured_provider}' is unsupported."
    _TERMLM_NO_LLM_WARNING_SHOWN=1
    return 0
  fi

  local client_bin
  client_bin="$(termlm-client-bin)"
  local status_out=""
  if status_out="$("$client_bin" status 2>/dev/null)"; then
    local runtime_provider provider_healthy
    runtime_provider="$(printf '%s\n' "$status_out" | awk -F': ' '/^provider:/ {print $2; exit}')"
    provider_healthy="$(printf '%s\n' "$status_out" | awk -F': ' '/^provider_healthy:/ {print $2; exit}')"
    if [[ "$provider_healthy" == "true" ]]; then
      return 0
    fi
    if [[ "$runtime_provider" == "ollama" || "$configured_provider" == "ollama" ]]; then
      local endpoint
      endpoint="$(termlm-config-ollama-endpoint)"
      termlm-warn-no-llm-provider "configured provider=ollama is unavailable (endpoint: ${endpoint})."
    else
      termlm-warn-no-llm-provider "configured local provider is unavailable."
    fi
    _TERMLM_NO_LLM_WARNING_SHOWN=1
    return 0
  fi

  if [[ "$configured_provider" == "ollama" ]]; then
    local endpoint
    endpoint="$(termlm-config-ollama-endpoint)"
    termlm-warn-no-llm-provider "configured provider=ollama could not be reached (endpoint: ${endpoint})."
    _TERMLM_NO_LLM_WARNING_SHOWN=1
  fi
}

termlm-helper-send() {
  local payload="$1"
  local attempt=1
  while (( attempt <= 3 )); do
    local fd="${_TERMLM_HELPER_IN_FD:-}"
    if [[ -n "$fd" ]] && termlm-helper-is-alive; then
      if print -r -u "$fd" -- "$payload" 2>/dev/null; then
        return 0
      fi
    fi
    termlm-start-helper >/dev/null 2>&1 || true
    sleep 0.1
    (( attempt += 1 ))
  done
  return 1
}

termlm-stop-helper() {
  local out_fd="${_TERMLM_HELPER_OUT_FD:-}"
  if [[ -n "$out_fd" ]]; then
    zle -F "$out_fd" 2>/dev/null || true
  fi
  if [[ -n "${_TERMLM_HELPER_IN_FD:-}" ]]; then
    exec {_TERMLM_HELPER_IN_FD}>&- 2>/dev/null || true
  fi
  if [[ -n "${_TERMLM_HELPER_OUT_FD:-}" ]]; then
    exec {_TERMLM_HELPER_OUT_FD}<&- 2>/dev/null || true
  fi
  if [[ -n "${_TERMLM_HELPER_FIFO_GUARD_IN_FD:-}" ]]; then
    exec {_TERMLM_HELPER_FIFO_GUARD_IN_FD}>&- 2>/dev/null || true
  fi
  if [[ -n "${_TERMLM_HELPER_FIFO_GUARD_OUT_FD:-}" ]]; then
    exec {_TERMLM_HELPER_FIFO_GUARD_OUT_FD}>&- 2>/dev/null || true
  fi

  local pid="${_TERMLM_HELPER_PID:-}"
  if [[ -n "$pid" ]]; then
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" >/dev/null 2>&1 || true
      local waited=0
      while kill -0 "$pid" 2>/dev/null && (( waited < 10 )); do
        sleep 0.05
        (( waited += 1 ))
      done
      if kill -0 "$pid" 2>/dev/null; then
        kill -9 "$pid" >/dev/null 2>&1 || true
      fi
      wait "$pid" >/dev/null 2>&1 || true
    fi
  fi

  [[ -n "${_TERMLM_HELPER_FIFO_IN:-}" ]] && rm -f -- "$_TERMLM_HELPER_FIFO_IN"
  [[ -n "${_TERMLM_HELPER_FIFO_OUT:-}" ]] && rm -f -- "$_TERMLM_HELPER_FIFO_OUT"

  _TERMLM_HELPER_PID=""
  _TERMLM_HELPER_IN_FD=""
  _TERMLM_HELPER_OUT_FD=""
  _TERMLM_HELPER_FIFO_IN=""
  _TERMLM_HELPER_FIFO_OUT=""
  _TERMLM_HELPER_FIFO_GUARD_IN_FD=""
  _TERMLM_HELPER_FIFO_GUARD_OUT_FD=""
}

termlm-start-helper() {
  if termlm-helper-is-alive && [[ -n "${_TERMLM_SHELL_ID:-}" ]]; then
    return 0
  fi

  termlm-stop-helper
  _TERMLM_SHELL_ID=""

  if ! termlm-ensure-daemon; then
    return 1
  fi

  mkdir -p -- "$_TERMLM_RUN_DIR"
  local fifo_in="${_TERMLM_RUN_DIR}/helper.in"
  local fifo_out="${_TERMLM_RUN_DIR}/helper.out"
  rm -f -- "$fifo_in" "$fifo_out"
  mkfifo "$fifo_in" "$fifo_out" || return 1
  _TERMLM_HELPER_FIFO_IN="$fifo_in"
  _TERMLM_HELPER_FIFO_OUT="$fifo_out"

  exec {_TERMLM_HELPER_FIFO_GUARD_IN_FD}<>"$fifo_in" || {
    termlm-stop-helper
    return 1
  }
  exec {_TERMLM_HELPER_FIFO_GUARD_OUT_FD}<>"$fifo_out" || {
    termlm-stop-helper
    return 1
  }

  local client_bin
  client_bin="$(termlm-client-bin)"
  local shell_pid shell_tty shell_version adapter_version
  shell_pid="$$"
  shell_tty="${TTY:-}"
  if [[ -z "$shell_tty" ]]; then
    shell_tty="$(tty 2>/dev/null || true)"
  fi
  if [[ -z "$shell_tty" || "$shell_tty" == *"not a tty"* ]]; then
    shell_tty="unknown"
  fi
  shell_version="${ZSH_VERSION:-unknown}"
  adapter_version="${TERMLM_ADAPTER_VERSION:-zsh-v1}"

  TERMLM_SHELL_PID="$shell_pid" \
    TERMLM_SHELL_TTY="$shell_tty" \
    TERMLM_SHELL_VERSION="$shell_version" \
    TERMLM_ADAPTER_VERSION="$adapter_version" \
    "$client_bin" bridge <"$fifo_in" >"$fifo_out" 2>/dev/null &!
  local pid=$!
  _TERMLM_HELPER_PID="$pid"

  exec {_TERMLM_HELPER_IN_FD}>"$fifo_in" || {
    termlm-stop-helper
    return 1
  }
  exec {_TERMLM_HELPER_OUT_FD}<"$fifo_out" || {
    termlm-stop-helper
    return 1
  }

  local waited_ms=0
  local max_wait_ms=5000
  local line
  while (( waited_ms < max_wait_ms )); do
    if IFS= read -r -t 0.05 line <&${_TERMLM_HELPER_OUT_FD}; then
      termlm-handle-run-task-line "$line"
      if [[ -n "${_TERMLM_SHELL_ID:-}" ]]; then
        zle -F "${_TERMLM_HELPER_OUT_FD}" termlm-handle-run-task-stream
        return 0
      fi
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      termlm-stop-helper
      return 1
    fi
    (( waited_ms += 50 ))
  done

  termlm-stop-helper
  return 1
}

termlm-register-shell() {
  if [[ -n "${_TERMLM_SHELL_ID:-}" ]] && termlm-helper-is-alive; then
    termlm-maybe-warn-no-llm-provider
    return 0
  fi

  if termlm-start-helper; then
    termlm-maybe-warn-no-llm-provider
    return 0
  fi

  termlm-maybe-warn-no-llm-provider
  return 1
}

termlm-send-shell-context() {
  if [[ -z "${_TERMLM_SHELL_ID:-}" ]] || ! termlm-helper-is-alive; then
    termlm-register-shell || return 0
  fi

  local -a aliases functions builtins
  aliases=("${(@f)$(termlm-build-alias-lines)}")
  functions=("${(@f)$(termlm-build-function-lines)}")
  builtins=(${(k)builtins})

  local material
  material="${(j:\n:)aliases}\n${(j:\n:)functions}"
  local context_hash
  context_hash="$(print -rn -- "$material" | shasum | awk '{print $1}')"
  if [[ "$context_hash" == "${_TERMLM_LAST_CONTEXT_HASH:-}" ]]; then
    return 0
  fi

  local payload aliases_json functions_json builtins_json
  local sep=""
  aliases_json=""
  local row name expansion
  for row in "${aliases[@]}"; do
    name="${row%%=*}"
    expansion="${row#*=}"
    [[ -z "$name" || "$name" == "$row" ]] && continue
    aliases_json+="${sep}{\"name\":\"$(termlm-json-escape "$name")\",\"expansion\":\"$(termlm-json-escape "$expansion")\"}"
    sep=","
  done

  sep=""
  functions_json=""
  local body_prefix
  for row in "${functions[@]}"; do
    name="${row%%|*}"
    body_prefix="${row#*|}"
    [[ -z "$name" || "$name" == "$row" ]] && continue
    functions_json+="${sep}{\"name\":\"$(termlm-json-escape "$name")\",\"body_prefix\":\"$(termlm-json-escape "$body_prefix")\"}"
    sep=","
  done

  sep=""
  builtins_json=""
  local b
  for b in "${builtins[@]}"; do
    builtins_json+="${sep}\"$(termlm-json-escape "$b")\""
    sep=","
  done

  payload="{\"op\":\"shell_context\",\"context_hash\":\"$(termlm-json-escape "$context_hash")\",\"aliases\":[${aliases_json}],\"functions\":[${functions_json}],\"builtins\":[${builtins_json}]}"
  termlm-helper-send "$payload" >/dev/null 2>&1 || true
  _TERMLM_LAST_CONTEXT_HASH="$context_hash"
}

termlm-core-bin() {
  echo "${TERMLM_CORE_BIN:-termlm-core}"
}

termlm-client-bin() {
  if [[ -n "${TERMLM_CLIENT_BIN:-}" ]]; then
    echo "${TERMLM_CLIENT_BIN}"
    return
  fi
  if command -v termlm >/dev/null 2>&1; then
    echo "termlm"
  else
    echo "termlm-client"
  fi
}

termlm-refresh-filesystem-context() {
  local client_bin
  client_bin="$(termlm-client-bin)"
  "$client_bin" refresh-context --cwd "$PWD" >/dev/null 2>&1 &!
}

termlm-daemon-boot-timeout-ms() {
  if [[ -n "${TERMLM_DAEMON_BOOT_TIMEOUT_SECS:-}" ]]; then
    local env_secs="${TERMLM_DAEMON_BOOT_TIMEOUT_SECS}"
    if [[ "$env_secs" == <-> && "$env_secs" -gt 0 ]]; then
      echo $(( env_secs * 1000 ))
      return
    fi
  fi

  local cfg="${XDG_CONFIG_HOME:-$HOME/.config}/termlm/config.toml"
  local boot_secs=60
  if [[ -f "$cfg" ]]; then
    local in_daemon=0 line
    while IFS= read -r line; do
      if [[ "$line" =~ '^[[:space:]]*\[[^]]+\][[:space:]]*$' ]]; then
        if [[ "$line" == *"[daemon]"* ]]; then
          in_daemon=1
        else
          in_daemon=0
        fi
        continue
      fi
      (( in_daemon )) || continue
      if [[ "$line" =~ '^[[:space:]]*boot_timeout_secs[[:space:]]*=[[:space:]]*([0-9]+)[[:space:]]*$' ]]; then
        boot_secs="${match[1]}"
        break
      fi
    done < "$cfg"
  fi
  [[ "$boot_secs" == <-> && "$boot_secs" -gt 0 ]] || boot_secs=60
  echo $(( boot_secs * 1000 ))
}

termlm-status-index-ready() {
  local status_out="$1"
  local progress_line progress_phase progress_percent chunk_count
  progress_line="$(printf '%s\n' "$status_out" | awk -F': ' '/^index_progress:/ {print $2; exit}')"
  progress_phase="$(printf '%s\n' "$progress_line" | sed -E 's/^phase=([^[:space:]]+).*/\1/')"
  progress_percent="$(printf '%s\n' "$progress_line" | awk '{for(i=1;i<=NF;i++){if($i ~ /^percent=/){split($i,a,"="); print a[2]; exit}}}')"
  chunk_count="$(printf '%s\n' "$status_out" | awk -F': ' '/^index_chunk_count:/ {print $2; exit}')"

  if [[ -z "$progress_line" && -z "$chunk_count" ]]; then
    return 0
  fi
  if [[ "$chunk_count" == <-> && "$chunk_count" -gt 0 ]]; then
    return 0
  fi
  if [[ "$progress_phase" == "complete" || "$progress_phase" == "idle" ]]; then
    if awk -v pct="${progress_percent:-0}" 'BEGIN { exit !(pct+0 >= 100.0) }'; then
      return 0
    fi
  fi
  return 1
}

termlm-daemon-ready() {
  local client_bin="$1"
  local status_out
  status_out="$("$client_bin" status --verbose 2>/dev/null)" || return 1
  termlm-status-index-ready "$status_out"
}

termlm-ensure-daemon() {
  local client_bin
  client_bin="$(termlm-client-bin)"

  if termlm-daemon-ready "$client_bin"; then
    return 0
  fi

  local core_bin
  core_bin="$(termlm-core-bin)"
  local daemon_log_file
  daemon_log_file="$(termlm-daemon-log-file)"
  mkdir -p -- "${daemon_log_file:h}" 2>/dev/null || true
  "$core_bin" --detach >>"$daemon_log_file" 2>&1 < /dev/null || {
    print -r -- "termlm: failed to launch termlm-core"
    return 1
  }

  local waited_ms=0
  local announced=0
  local max_wait_ms
  max_wait_ms="$(termlm-daemon-boot-timeout-ms)"
  [[ "$max_wait_ms" == <-> && "$max_wait_ms" -gt 0 ]] || max_wait_ms=60000
  while (( waited_ms < max_wait_ms )); do
    if termlm-daemon-ready "$client_bin"; then
      return 0
    fi

    if (( waited_ms >= 1000 && announced == 0 )); then
      print -r -- "termlm: starting termlm-core..."
      announced=1
    fi

    sleep 0.1
    (( waited_ms += 100 ))
  done

  print -r -- "termlm: failed to start termlm-core"
  return 1
}

termlm-daemon-log-file() {
  if [[ -n "${TERMLM_DAEMON_LOG_FILE:-}" ]]; then
    echo "$TERMLM_DAEMON_LOG_FILE"
    return
  fi
  if [[ -n "${XDG_STATE_HOME:-}" ]]; then
    echo "${XDG_STATE_HOME}/termlm/termlm.log"
    return
  fi
  echo "${HOME}/.local/state/termlm/termlm.log"
}

termlm-report-daemon-died() {
  local log_file
  log_file="$(termlm-daemon-log-file)"
  print -r -- "termlm: daemon died"
  if [[ -r "$log_file" ]]; then
    local tail_lines
    tail_lines="$(tail -n 8 -- "$log_file" 2>/dev/null)"
    if [[ -n "$tail_lines" ]]; then
      print -r -- "termlm: recent logs:"
      print -r -- "$tail_lines"
    fi
  fi
}

termlm-reset-after-connection-lost() {
  _TERMLM_WAITING_MODEL=0
  termlm-clear-task-status
  termlm-mark-task-closed
  if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
    termlm-exit-prompt-mode
  else
    zle reset-prompt
  fi
}

termlm-mark-task-closed() {
  termlm-clear-task-status
  local closed_task_id="${_TERMLM_TASK_ID:-${_TERMLM_APPROVAL_TASK_ID:-${_TERMLM_CLARIFICATION_TASK_ID:-}}}"
  if [[ -n "$closed_task_id" ]]; then
    _TERMLM_CLOSED_TASK_ID="$closed_task_id"
  fi
  if [[ -n "${_TERMLM_ACKED_PENDING_TASK_ID:-}" && "$closed_task_id" == "$_TERMLM_ACKED_PENDING_TASK_ID" ]]; then
    _TERMLM_ACKED_PENDING_TASK_ID=""
  fi
  _TERMLM_WAITING_MODEL=0
  _TERMLM_TASK_ID=""
  _TERMLM_CLARIFICATION_TASK_ID=""
  _TERMLM_APPROVAL_TASK_ID=""
  _TERMLM_APPROVAL_CMD=""
  _TERMLM_EDITING_APPROVAL_TASK_ID=""
  _TERMLM_OUTPUT_STARTED=0
  _TERMLM_OUTPUT_NEEDS_NEWLINE=0
}

termlm-is-closed-task-event() {
  local task_id="$1"
  [[ -n "$task_id" && "$task_id" == "${_TERMLM_CLOSED_TASK_ID:-}" && "${_TERMLM_TASK_ID:-}" != "$task_id" ]]
}

termlm-is-current-task-event() {
  local task_id="$1"
  [[ -z "$task_id" ]] && return 0
  [[ "$task_id" == "${_TERMLM_TASK_ID:-}" \
    || "$task_id" == "${_TERMLM_PENDING_TASK_ID:-}" \
    || "$task_id" == "${_TERMLM_APPROVAL_TASK_ID:-}" \
    || "$task_id" == "${_TERMLM_CLARIFICATION_TASK_ID:-}" ]]
}

termlm-should-ignore-task-event() {
  local task_id="$1"
  if termlm-is-closed-task-event "$task_id"; then
    return 0
  fi
  if [[ -z "${_TERMLM_TASK_ID:-}" \
    && -z "${_TERMLM_PENDING_TASK_ID:-}" \
    && -z "${_TERMLM_APPROVAL_TASK_ID:-}" \
    && -z "${_TERMLM_CLARIFICATION_TASK_ID:-}" ]]; then
    return 1
  fi
  if termlm-is-current-task-event "$task_id"; then
    return 1
  fi
  return 0
}

termlm-abandon-active-task() {
  local preserve_session="${1:-0}"
  local task_id="${_TERMLM_TASK_ID:-}"

  termlm-mark-task-closed
  if [[ -n "$task_id" ]]; then
    termlm-abort-task "$task_id" >/dev/null 2>&1 || true
  fi

  if [[ $_TERMLM_SESSION_MODE -eq 1 ]]; then
    if (( preserve_session == 1 )); then
      zle reset-prompt
    else
      termlm-exit-session-mode
    fi
  else
    termlm-exit-prompt-mode
  fi
}

termlm-start-task() {
  local prompt="$1"
  termlm-load-capture-settings

  local mode="?"
  if [[ $_TERMLM_SESSION_MODE -eq 1 ]]; then
    mode="/p"
  fi

  if [[ -z "$_TERMLM_SHELL_ID" ]] || ! termlm-helper-is-alive; then
    termlm-register-shell || return 1
  fi
  termlm-send-shell-context

  local task_id
  task_id="$(uuidgen 2>/dev/null | tr '[:upper:]' '[:lower:]')"
  local payload
  if [[ -n "$task_id" ]]; then
    payload="{\"op\":\"start_task\",\"task_id\":\"$(termlm-json-escape "$task_id")\",\"mode\":\"$(termlm-json-escape "$mode")\",\"prompt\":\"$(termlm-json-escape "$prompt")\",\"cwd\":\"$(termlm-json-escape "$PWD")\"}"
  else
    payload="{\"op\":\"start_task\",\"mode\":\"$(termlm-json-escape "$mode")\",\"prompt\":\"$(termlm-json-escape "$prompt")\",\"cwd\":\"$(termlm-json-escape "$PWD")\"}"
  fi
  if ! termlm-helper-send "$payload"; then
    zle -M "termlm: connection lost" 2>/dev/null || print -r -- "termlm: connection lost"
    _TERMLM_WAITING_MODEL=0
    return 1
  fi

  _TERMLM_WAITING_MODEL=1
  _TERMLM_TASK_ID="$task_id"
  _TERMLM_CLARIFICATION_TASK_ID=""
  _TERMLM_APPROVAL_TASK_ID=""
  _TERMLM_APPROVAL_CMD=""
  _TERMLM_EDITING_APPROVAL_TASK_ID=""
  _TERMLM_CLOSED_TASK_ID=""
  _TERMLM_OUTPUT_STARTED=0
  _TERMLM_OUTPUT_NEEDS_NEWLINE=0
  termlm-show-task-status "termlm: thinking..."
  zle reset-prompt
}

termlm-show-task-status() {
  local message="$1"
  _TERMLM_STATUS_MESSAGE_ACTIVE=1
  zle -M "$message" 2>/dev/null || true
}

termlm-clear-task-status() {
  if [[ "${_TERMLM_STATUS_MESSAGE_ACTIVE:-0}" -eq 1 ]]; then
    zle -M "" 2>/dev/null || true
    _TERMLM_STATUS_MESSAGE_ACTIVE=0
  fi
}

termlm-begin-async-output() {
  termlm-clear-task-status
  zle -I 2>/dev/null || true
  if [[ "${_TERMLM_OUTPUT_STARTED:-0}" -eq 0 ]]; then
    print -r -- ""
    _TERMLM_OUTPUT_STARTED=1
    _TERMLM_OUTPUT_NEEDS_NEWLINE=0
  fi
}

termlm-note-output-chunk() {
  local chunk="$1"
  if [[ "$chunk" == *$'\n' ]]; then
    _TERMLM_OUTPUT_NEEDS_NEWLINE=0
  elif [[ -n "$chunk" ]]; then
    _TERMLM_OUTPUT_NEEDS_NEWLINE=1
  fi
}

termlm-finish-async-output-line() {
  termlm-begin-async-output
  if [[ "${_TERMLM_OUTPUT_NEEDS_NEWLINE:-0}" -eq 1 ]]; then
    print -r -- ""
    _TERMLM_OUTPUT_NEEDS_NEWLINE=0
  fi
}

termlm-handle-run-task-stream() {
  local fd="$1"
  local line=""

  if ! IFS= read -r line <&$fd; then
    local was_waiting="${_TERMLM_WAITING_MODEL:-0}"
    local was_session="${_TERMLM_SESSION_MODE:-0}"
    termlm-stop-helper
    if [[ "$was_waiting" == "1" ]]; then
      _TERMLM_WAITING_MODEL=0
      if [[ -n "${_TERMLM_TASK_ID:-}" ]]; then
        if [[ "${_TERMLM_OUTPUT_NEEDS_NEWLINE:-0}" -eq 1 ]]; then
          print -r -- ""
          _TERMLM_OUTPUT_NEEDS_NEWLINE=0
        fi
        termlm-report-daemon-died
        termlm-mark-task-closed
        if [[ "$was_session" == "1" ]]; then
          zle reset-prompt
        else
          termlm-exit-prompt-mode
        fi
      fi
    fi
    return
  fi

  termlm-handle-run-task-line "$line"
}

termlm-handle-run-task-line() {
  local line="$1"
  local kind
  kind="$(termlm-json-event-kind "$line")"

  case "$kind" in
    shell_registered)
      _TERMLM_SHELL_ID="$(termlm-json-field "$line" "shell_id")"
      ;;
    task_started)
      _TERMLM_TASK_ID="$(termlm-json-field "$line" "task_id")"
      ;;
    model_text)
      local task_id chunk_b64 chunk
      task_id="$(termlm-json-field "$line" "task_id")"
      chunk_b64="$(termlm-json-field "$line" "chunk_b64")"
      chunk="$(termlm-base64-decode "$chunk_b64")"
      if termlm-should-ignore-task-event "$task_id"; then
        return
      fi
      if [[ -z "$_TERMLM_TASK_ID" ]]; then
        _TERMLM_TASK_ID="$task_id"
      fi
      termlm-begin-async-output
      print -rn -u 2 -- "$chunk"
      termlm-note-output-chunk "$chunk"
      ;;
    needs_clarification)
      _TERMLM_WAITING_MODEL=0
      local task_id question_b64 question
      task_id="$(termlm-json-field "$line" "task_id")"
      question_b64="$(termlm-json-field "$line" "question_b64")"
      question="$(termlm-base64-decode "$question_b64")"
      if termlm-should-ignore-task-event "$task_id"; then
        return
      fi
      _TERMLM_TASK_ID="$task_id"
      _TERMLM_CLARIFICATION_TASK_ID="$task_id"
      termlm-finish-async-output-line
      print -r -- "❓ ${question}"
      if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
        termlm-enter-prompt-mode
      else
        zle reset-prompt
      fi
      ;;
    proposed_command)
      _TERMLM_WAITING_MODEL=0
      local task_id cmd_b64 cmd requires
      task_id="$(termlm-json-field "$line" "task_id")"
      cmd_b64="$(termlm-json-field "$line" "cmd_b64")"
      cmd="$(termlm-base64-decode "$cmd_b64")"
      requires="$(termlm-json-bool-field "$line" "requires_approval")"
      if termlm-should-ignore-task-event "$task_id"; then
        return
      fi
      termlm-finish-async-output-line
      termlm-handle-proposed-event "$task_id" "$cmd" "$requires"
      ;;
    index_progress|provider_status|status_report|pong|index_update|retrieval_chunk|daemon_event)
      ;;
    task_complete)
      local task_id
      task_id="$(termlm-json-field "$line" "task_id")"
      if termlm-should-ignore-task-event "$task_id"; then
        return
      fi
      local suppress_prompt_reset=0
      if [[ $_TERMLM_SESSION_MODE -eq 0 \
        && -n "${_TERMLM_ACKED_PENDING_TASK_ID:-}" \
        && "$task_id" == "$_TERMLM_ACKED_PENDING_TASK_ID" ]]; then
        suppress_prompt_reset=1
      fi
      if [[ "${_TERMLM_OUTPUT_NEEDS_NEWLINE:-0}" -eq 1 ]]; then
        print -r -- ""
      fi
      termlm-mark-task-closed
      if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
        if (( suppress_prompt_reset == 0 )); then
          termlm-exit-prompt-mode
        else
          _TERMLM_MODE="normal"
          PS1="${_TERMLM_SAVED_PS1:-$PS1}"
          zle -K main
        fi
      else
        zle reset-prompt
      fi
      ;;
    error)
      local task_id err_kind msg_b64 msg
      task_id="$(termlm-json-field "$line" "task_id")"
      err_kind="$(termlm-json-field "$line" "kind")"
      msg_b64="$(termlm-json-field "$line" "message_b64")"
      msg="$(termlm-base64-decode "$msg_b64")"
      if termlm-should-ignore-task-event "$task_id"; then
        return
      fi
      if [[ -z "$_TERMLM_TASK_ID" && -n "$task_id" ]]; then
        _TERMLM_TASK_ID="$task_id"
      fi
      termlm-finish-async-output-line
      print -r -- "termlm: ${msg}"
      if [[ "$err_kind" == "bad_protocol" || "$err_kind" == "BadProtocol" ]]; then
        termlm-mark-task-closed
        if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
          termlm-exit-prompt-mode
        else
          zle reset-prompt
        fi
      fi
      ;;
    timeout)
      termlm-finish-async-output-line
      termlm-mark-task-closed
      print -r -- "termlm: request timed out"
      if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
        termlm-exit-prompt-mode
      else
        zle reset-prompt
      fi
      ;;
    *)
      ;;
  esac
}

termlm-handle-proposed-event() {
  local task_id="$1"
  local cmd="$2"
  local requires="$3"

  _TERMLM_TASK_ID="$task_id"

  if termlm-safety-floor-match "$cmd" >/dev/null; then
    print -r -- "termlm: blocked by adapter safety floor"
    termlm-send-decision "$task_id" --decision abort >/dev/null 2>&1 || true
    _TERMLM_TASK_ID=""
    if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
      termlm-exit-prompt-mode
    else
      zle reset-prompt
    fi
    return
  fi

  if [[ "$requires" == "true" ]]; then
    _TERMLM_WAITING_MODEL=0
    _TERMLM_APPROVAL_TASK_ID="$task_id"
    _TERMLM_APPROVAL_CMD="$cmd"
    _TERMLM_EDITING_APPROVAL_TASK_ID=""
    termlm-approval-prompt "$cmd"
    zle reset-prompt
    return
  fi

  termlm-run-approved-command "$task_id" "$cmd"
}

termlm-run-approved-command() {
  local task_id="$1"
  local approved_cmd="$2"

  _TERMLM_ACKED_PENDING_TASK_ID=""
  _TERMLM_PENDING_TASK_ID="$task_id"
  _TERMLM_PENDING_CMD="$approved_cmd"
  _TERMLM_PENDING_CWD_BEFORE="$PWD"
  _TERMLM_PENDING_STARTED_AT="$EPOCHREALTIME"
  (( _TERMLM_PENDING_SEQ += 1 ))
  if termlm-capture-enabled; then
    _TERMLM_PENDING_STDOUT_FILE="${_TERMLM_RUN_DIR}/stdout.${_TERMLM_PENDING_SEQ}"
    _TERMLM_PENDING_STDERR_FILE="${_TERMLM_RUN_DIR}/stderr.${_TERMLM_PENDING_SEQ}"
  else
    _TERMLM_PENDING_STDOUT_FILE=""
    _TERMLM_PENDING_STDERR_FILE=""
  fi

  if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
    _TERMLM_MODE="normal"
    PS1="${_TERMLM_SAVED_PS1:-$PS1}"
    zle -K main
  fi
  BUFFER="$approved_cmd"
  CURSOR=${#BUFFER}
  zle reset-prompt
  zle .accept-line
}

termlm-reject-pending-approval() {
  local task_id="${_TERMLM_APPROVAL_TASK_ID:-}"
  if [[ -n "$task_id" ]] && ! termlm-send-decision "$task_id" --decision rejected >/dev/null 2>&1; then
    return 0
  fi
  _TERMLM_APPROVAL_TASK_ID=""
  _TERMLM_APPROVAL_CMD=""
  _TERMLM_EDITING_APPROVAL_TASK_ID=""
  termlm-mark-task-closed
  if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
    termlm-exit-prompt-mode
  else
    zle reset-prompt
  fi
}

termlm-handle-approval-key() {
  local key="$1"
  local task_id="${_TERMLM_APPROVAL_TASK_ID:-}"
  local cmd="${_TERMLM_APPROVAL_CMD:-}"
  [[ -n "$task_id" ]] || return 0

  case "$key" in
    y|Y)
      _TERMLM_APPROVAL_TASK_ID=""
      _TERMLM_APPROVAL_CMD=""
      _TERMLM_EDITING_APPROVAL_TASK_ID=""
      if termlm-send-decision "$task_id" --decision approved; then
        termlm-run-approved-command "$task_id" "$cmd"
      fi
      ;;
    a|A)
      _TERMLM_APPROVAL_TASK_ID=""
      _TERMLM_APPROVAL_CMD=""
      _TERMLM_EDITING_APPROVAL_TASK_ID=""
      if termlm-send-decision "$task_id" --decision approve-all; then
        termlm-run-approved-command "$task_id" "$cmd"
      fi
      ;;
    e|E)
      _TERMLM_EDITING_APPROVAL_TASK_ID="$task_id"
      BUFFER="$cmd"
      CURSOR=${#BUFFER}
      zle reset-prompt
      ;;
    $'\x1b')
      termlm-send-decision "$task_id" --decision abort >/dev/null 2>&1 || true
      _TERMLM_APPROVAL_TASK_ID=""
      _TERMLM_APPROVAL_CMD=""
      _TERMLM_EDITING_APPROVAL_TASK_ID=""
      termlm-mark-task-closed
      if [[ $_TERMLM_SESSION_MODE -eq 0 ]]; then
        termlm-exit-prompt-mode
      else
        zle reset-prompt
      fi
      ;;
    *)
      termlm-reject-pending-approval
      ;;
  esac
}

termlm-finish-edited-approval() {
  local edited="$1"
  local task_id="${_TERMLM_EDITING_APPROVAL_TASK_ID:-}"
  [[ -n "$task_id" ]] || return 0

  _TERMLM_APPROVAL_TASK_ID=""
  _TERMLM_APPROVAL_CMD=""
  _TERMLM_EDITING_APPROVAL_TASK_ID=""
  if termlm-send-decision "$task_id" --decision edited --edited-command "$edited"; then
    termlm-run-approved-command "$task_id" "$edited"
  fi
}

termlm-abort-task() {
  local task_id="$1"
  [[ -z "$task_id" ]] && return 0
  termlm-send-decision "$task_id" --decision abort >/dev/null 2>&1 || true
}

termlm-send-decision() {
  local task_id="$1"
  shift

  local decision=""
  local edited_command=""
  local text=""

  while (( $# > 0 )); do
    case "$1" in
      --decision)
        decision="$2"
        shift 2
        ;;
      --edited-command)
        edited_command="$2"
        shift 2
        ;;
      --text)
        text="$2"
        shift 2
        ;;
      *)
        shift
        ;;
    esac
  done

  local decision_enum=""
  case "$decision" in
    approved) decision_enum="approved" ;;
    rejected) decision_enum="rejected" ;;
    edited) decision_enum="edited" ;;
    approve-all) decision_enum="approve_all_in_task" ;;
    abort) decision_enum="abort" ;;
    clarification) decision_enum="clarification" ;;
    *) return 1 ;;
  esac

  local payload
  payload="{\"op\":\"user_response\",\"task_id\":\"$(termlm-json-escape "$task_id")\",\"decision\":\"${decision_enum}\""
  if [[ -n "$edited_command" ]]; then
    payload+=",\"edited_command\":\"$(termlm-json-escape "$edited_command")\""
  fi
  if [[ -n "$text" ]]; then
    payload+=",\"text\":\"$(termlm-json-escape "$text")\""
  fi
  payload+="}"

  if termlm-helper-send "$payload"; then
    if [[ "$decision_enum" == "rejected" || "$decision_enum" == "clarification" ]]; then
      _TERMLM_WAITING_MODEL=1
    fi
    if [[ "$decision_enum" == "abort" ]]; then
      _TERMLM_WAITING_MODEL=0
    fi
    return 0
  fi

  zle -M "termlm: connection lost" 2>/dev/null || print -r -- "termlm: connection lost"
  termlm-reset-after-connection-lost
  return 1
}
