termlm-self-insert() {
  if [[ -n "${_TERMLM_APPROVAL_TASK_ID:-}" && -z "${_TERMLM_EDITING_APPROVAL_TASK_ID:-}" ]]; then
    termlm-handle-approval-key "$KEYS"
    return
  fi

  if [[ "$KEYS" == "?" ]]; then
    if [[ "$LBUFFER" == *"\\" ]]; then
      zle .self-insert
      return
    fi

    if [[ -z "${${LBUFFER}//[[:space:]]/}" && -z "$RBUFFER" ]]; then
      termlm-enter-prompt-mode
      BUFFER=""
      CURSOR=0
      return
    fi
  fi

  zle .self-insert
}
