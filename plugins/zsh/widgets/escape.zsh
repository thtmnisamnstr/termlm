termlm-cancel-prompt() {
  if [[ "${_TERMLM_MODE:-normal}" != "prompt" && ${_TERMLM_SESSION_MODE:-0} -eq 0 ]]; then
    zle .send-break 2>/dev/null || zle reset-prompt
    return
  fi

  local task_id="${_TERMLM_TASK_ID:-}"
  if [[ -z "$task_id" ]]; then
    task_id="${_TERMLM_APPROVAL_TASK_ID:-${_TERMLM_CLARIFICATION_TASK_ID:-}}"
  fi

  if [[ -n "$task_id" ]]; then
    termlm-send-decision "$task_id" --decision abort >/dev/null 2>&1 || true
  fi

  BUFFER=""
  LBUFFER=""
  RBUFFER=""
  CURSOR=0
  termlm-mark-task-closed

  if [[ ${_TERMLM_SESSION_MODE:-0} -eq 1 ]]; then
    termlm-exit-session-mode
  else
    termlm-exit-prompt-mode
  fi
}
