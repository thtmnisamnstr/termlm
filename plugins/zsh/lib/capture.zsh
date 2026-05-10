typeset -g _TERMLM_CAPTURE_ENABLED=1
typeset -g _TERMLM_CAPTURE_MAX_BYTES=16384

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

termlm-wrap-command-for-capture() {
  local cmd="$1"
  local n="$2"

  if ! termlm-capture-enabled; then
    echo "$cmd"
    return
  fi

  local out="${_TERMLM_RUN_DIR}/stdout.${n}"
  local err="${_TERMLM_RUN_DIR}/stderr.${n}"
  echo "( { $cmd; } > >(tee \"$out\") 2> >(tee \"$err\" >&2) )"
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
