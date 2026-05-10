termlm-accept-line() {
  if [[ "$BUFFER" =~ '^/p[[:space:]]*$' && $_TERMLM_SESSION_MODE -eq 0 ]]; then
    if [[ ${_TERMLM_WAITING_MODEL:-0} -eq 1 ]]; then
      termlm-abandon-active-task 0
    fi
    BUFFER=""
    termlm-enter-session-mode
    return
  fi

  if [[ "$BUFFER" =~ '^/q[[:space:]]*$' && $_TERMLM_SESSION_MODE -eq 1 ]]; then
    if [[ -n "$_TERMLM_TASK_ID" || ${_TERMLM_WAITING_MODEL:-0} -eq 1 ]]; then
      termlm-abandon-active-task 0
    else
      BUFFER=""
      termlm-exit-session-mode
    fi
    return
  fi

  if [[ "$_TERMLM_MODE" == "prompt" || $_TERMLM_SESSION_MODE -eq 1 ]]; then
    if [[ ${_TERMLM_WAITING_MODEL:-0} -eq 1 && "$BUFFER" =~ '^[[:space:]]*[a-zA-Z_./][a-zA-Z0-9_./-]*([[:space:]]|$)' ]]; then
      local typed="$BUFFER"
      if [[ $_TERMLM_SESSION_MODE -eq 1 ]]; then
        termlm-abandon-active-task 1
      else
        termlm-abandon-active-task 0
      fi
      BUFFER="$typed"
      CURSOR=${#BUFFER}
      return
    fi

    local prompt="$BUFFER"
    BUFFER=""

    if [[ -n "$prompt" ]]; then
      termlm-start-task "$prompt"
    else
      zle reset-prompt
    fi
    return
  fi

  zle .accept-line
}
