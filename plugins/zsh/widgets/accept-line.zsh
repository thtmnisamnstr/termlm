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
    if [[ -n "${_TERMLM_EDITING_APPROVAL_TASK_ID:-}" ]]; then
      local edited="$BUFFER"
      BUFFER=""

      if [[ -z "${edited//[[:space:]]/}" ]]; then
        termlm-handle-approval-key "n"
        return
      fi

      termlm-finish-edited-approval "$edited"
      return
    fi

    if [[ -n "${_TERMLM_APPROVAL_TASK_ID:-}" ]]; then
      termlm-handle-approval-key $'\n'
      return
    fi

    if [[ -n "${_TERMLM_CLARIFICATION_TASK_ID:-}" ]]; then
      local reply="$BUFFER"
      local task_id="$_TERMLM_CLARIFICATION_TASK_ID"
      BUFFER=""

      if [[ -z "${reply//[[:space:]]/}" ]]; then
        zle reset-prompt
        return
      fi

      _TERMLM_TASK_ID="$task_id"
      _TERMLM_CLARIFICATION_TASK_ID=""
      if ! termlm-send-decision "$task_id" --decision clarification --text "$reply"; then
        print -r -- "termlm: clarification response failed"
        _TERMLM_WAITING_MODEL=0
        _TERMLM_TASK_ID=""
        [[ $_TERMLM_SESSION_MODE -eq 0 ]] && termlm-exit-prompt-mode || zle reset-prompt
        return
      fi
      _TERMLM_WAITING_MODEL=1
      termlm-show-task-status "termlm: thinking..."
      zle reset-prompt
      return
    fi

    if [[ ${_TERMLM_WAITING_MODEL:-0} -eq 1 && "$BUFFER" =~ '^[[:space:]]*[a-zA-Z_./][a-zA-Z0-9_./-]*([[:space:]]|$)' ]]; then
      local typed="$BUFFER"
      if [[ $_TERMLM_SESSION_MODE -eq 1 ]]; then
        termlm-abandon-active-task 1
      else
        termlm-abandon-active-task 0
      fi
      BUFFER="$typed"
      CURSOR=${#BUFFER}
      zle .accept-line
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
