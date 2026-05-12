typeset -g _TERMLM_CAPTURE_ENABLED=1
typeset -g _TERMLM_CAPTURE_MAX_BYTES=16384
typeset -g _TERMLM_CAPTURE_ACTIVE=0
typeset -g _TERMLM_CAPTURE_SAVE_STDOUT_FD=-1
typeset -g _TERMLM_CAPTURE_SAVE_STDERR_FD=-1
typeset -g _TERMLM_CAPTURE_RESTORE_STDOUT_FD=-1
typeset -g _TERMLM_CAPTURE_RESTORE_STDERR_FD=-1
typeset -g _TERMLM_CAPTURE_STARTED_STDERR_TTY=0

termlm-load-capture-settings() {
  local cfg="${XDG_CONFIG_HOME:-$HOME/.config}/termlm/config.toml"

  if [[ -n "${TERMLM_CAPTURE_ENABLED:-}" ]]; then
    if [[ "${TERMLM_CAPTURE_ENABLED}" == "0" ]]; then
      _TERMLM_CAPTURE_ENABLED=0
    else
      _TERMLM_CAPTURE_ENABLED=1
    fi
  else
    _TERMLM_CAPTURE_ENABLED=1
  fi

  if [[ -n "${TERMLM_CAPTURE_MAX_BYTES:-}" ]]; then
    if [[ "${TERMLM_CAPTURE_MAX_BYTES}" =~ '^[0-9]+$' ]] && (( TERMLM_CAPTURE_MAX_BYTES > 0 )); then
      _TERMLM_CAPTURE_MAX_BYTES="${TERMLM_CAPTURE_MAX_BYTES}"
    else
      _TERMLM_CAPTURE_MAX_BYTES=16384
    fi
  else
    _TERMLM_CAPTURE_MAX_BYTES=16384
  fi

  [[ ! -f "$cfg" ]] && return 0

  local in_capture=0
  local line
  while IFS= read -r line; do
    if [[ "$line" =~ '^[[:space:]]*\[[^]]+\][[:space:]]*$' ]]; then
      if [[ "$line" == *"[capture]"* ]]; then
        in_capture=1
      else
        in_capture=0
      fi
      continue
    fi

    (( in_capture )) || continue

    if [[ -z "${TERMLM_CAPTURE_ENABLED:-}" ]] && [[ "$line" =~ '^[[:space:]]*enabled[[:space:]]*=[[:space:]]*(true|false)' ]]; then
      if [[ "${match[1]}" == "true" ]]; then
        _TERMLM_CAPTURE_ENABLED=1
      else
        _TERMLM_CAPTURE_ENABLED=0
      fi
    fi

    if [[ -z "${TERMLM_CAPTURE_MAX_BYTES:-}" ]] && [[ "$line" =~ '^[[:space:]]*max_bytes[[:space:]]*=[[:space:]]*([0-9]+)' ]]; then
      if (( match[1] > 0 )); then
        _TERMLM_CAPTURE_MAX_BYTES="${match[1]}"
      fi
    fi
  done < "$cfg"
}

termlm-capture-enabled() {
  (( _TERMLM_CAPTURE_ENABLED == 1 ))
}

termlm-start-output-capture() {
  local out="$1"
  local err="$2"
  [[ -n "$out" && -n "$err" ]] || return 1

  if (( _TERMLM_CAPTURE_ACTIVE == 1 )); then
    termlm-stop-output-capture
  fi

  : >| "$out" 2>/dev/null || return 1
  : >| "$err" 2>/dev/null || return 1

  local save_out save_err
  exec {save_out}>&1 || return 1
  exec {save_err}>&2 || {
    exec {save_out}>&- 2>/dev/null || true
    return 1
  }

  _TERMLM_CAPTURE_SAVE_STDOUT_FD="$save_out"
  _TERMLM_CAPTURE_SAVE_STDERR_FD="$save_err"
  _TERMLM_CAPTURE_RESTORE_STDOUT_FD="$save_out"
  _TERMLM_CAPTURE_RESTORE_STDERR_FD="$save_err"
  [[ -t 2 ]] && _TERMLM_CAPTURE_STARTED_STDERR_TTY=1 || _TERMLM_CAPTURE_STARTED_STDERR_TTY=0

  local tty_path="${TTY:-}"
  if [[ -z "$tty_path" || "$tty_path" == *"not a tty"* || ! -w "$tty_path" ]]; then
    tty_path="$(tty 2>/dev/null || true)"
  fi
  if [[ -n "$tty_path" && "$tty_path" != *"not a tty"* && -w "$tty_path" ]]; then
    if [[ ! -t 1 ]]; then
      local tty_out
      if exec {tty_out}>"$tty_path" 2>/dev/null; then
        _TERMLM_CAPTURE_RESTORE_STDOUT_FD="$tty_out"
      fi
    fi
    if [[ ! -t 2 ]]; then
      local tty_err
      if exec {tty_err}>"$tty_path" 2>/dev/null; then
        _TERMLM_CAPTURE_RESTORE_STDERR_FD="$tty_err"
      fi
    fi
  fi
  if [[ "$_TERMLM_CAPTURE_RESTORE_STDERR_FD" == "$save_err" && ! -t 2 && "$_TERMLM_CAPTURE_RESTORE_STDOUT_FD" != "-1" ]]; then
    local stderr_from_stdout
    if exec {stderr_from_stdout}>&${_TERMLM_CAPTURE_RESTORE_STDOUT_FD} 2>/dev/null; then
      _TERMLM_CAPTURE_RESTORE_STDERR_FD="$stderr_from_stdout"
    fi
  fi

  exec > >(tee "$out" >&${_TERMLM_CAPTURE_RESTORE_STDOUT_FD}) || {
    termlm-stop-output-capture
    return 1
  }
  exec 2> >(tee "$err" >&${_TERMLM_CAPTURE_RESTORE_STDERR_FD}) || {
    termlm-stop-output-capture
    return 1
  }

  _TERMLM_CAPTURE_ACTIVE=1
  return 0
}

termlm-stop-output-capture() {
  local restore_out="${_TERMLM_CAPTURE_RESTORE_STDOUT_FD:-$_TERMLM_CAPTURE_SAVE_STDOUT_FD}"
  local restore_err="${_TERMLM_CAPTURE_RESTORE_STDERR_FD:-$_TERMLM_CAPTURE_SAVE_STDERR_FD}"
  local save_out="${_TERMLM_CAPTURE_SAVE_STDOUT_FD:-}"
  local save_err="${_TERMLM_CAPTURE_SAVE_STDERR_FD:-}"

  if [[ "$restore_out" != "-1" ]]; then
    eval "exec 1>&${restore_out}" 2>/dev/null || true
  elif [[ "$save_out" != "-1" ]]; then
    eval "exec 1>&${save_out}" 2>/dev/null || true
  fi
  if [[ "$restore_err" != "-1" ]]; then
    eval "exec 2>&${restore_err}" 2>/dev/null || true
  elif [[ "$save_err" != "-1" ]]; then
    eval "exec 2>&${save_err}" 2>/dev/null || true
  fi
  if (( _TERMLM_CAPTURE_STARTED_STDERR_TTY == 1 )) && [[ ! -t 2 && -t 1 ]]; then
    exec 2>&1 || true
  fi
  [[ "$save_out" =~ '^[0-9]+$' ]] && exec {save_out}>&- 2>/dev/null || true
  [[ "$save_err" =~ '^[0-9]+$' ]] && exec {save_err}>&- 2>/dev/null || true
  if [[ "$restore_out" =~ '^[0-9]+$' && "$restore_out" != "$save_out" ]]; then
    exec {restore_out}>&- 2>/dev/null || true
  fi
  if [[ "$restore_err" =~ '^[0-9]+$' && "$restore_err" != "$save_err" ]]; then
    exec {restore_err}>&- 2>/dev/null || true
  fi
  _TERMLM_CAPTURE_ACTIVE=0
  _TERMLM_CAPTURE_SAVE_STDOUT_FD=-1
  _TERMLM_CAPTURE_SAVE_STDERR_FD=-1
  _TERMLM_CAPTURE_RESTORE_STDOUT_FD=-1
  _TERMLM_CAPTURE_RESTORE_STDERR_FD=-1
  _TERMLM_CAPTURE_STARTED_STDERR_TTY=0
}

termlm-read-captured-file-b64() {
  local file="$1"
  local max_bytes="$2"

  if [[ ! -f "$file" ]]; then
    echo ""
    echo "0"
    return
  fi

  local size
  size="$(wc -c < "$file" | tr -d '[:space:]')"
  if [[ -z "$size" ]]; then
    size=0
  fi

  local truncated=0
  local encoded=""
  if (( size > max_bytes )); then
    truncated=1
    encoded="$(head -c "$max_bytes" -- "$file" | base64 | tr -d '\n')"
  else
    encoded="$(base64 < "$file" | tr -d '\n')"
  fi

  echo "$encoded"
  echo "$truncated"
}
