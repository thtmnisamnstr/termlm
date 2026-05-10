termlm-delete-char-or-list() {
  if [[ $_TERMLM_SESSION_MODE -eq 1 ]]; then
    local trimmed="${${BUFFER:-}//[[:space:]]/}"
    if [[ -z "$trimmed" && -z "${RBUFFER:-}" ]]; then
      BUFFER=""
      if [[ -n "${_TERMLM_TASK_ID:-}" || ${_TERMLM_WAITING_MODEL:-0} -eq 1 ]]; then
        termlm-abandon-active-task 0
      else
        termlm-exit-session-mode
      fi
      return
    fi
  fi

  zle .delete-char-or-list
}
