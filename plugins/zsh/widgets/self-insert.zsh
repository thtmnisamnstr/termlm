termlm-self-insert() {
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
