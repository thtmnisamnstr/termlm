typeset -g _TERMLM_PROMPT_INDICATOR='● ? '
typeset -g _TERMLM_SESSION_INDICATOR='● /p '
typeset -g _TERMLM_PROMPT_USE_COLOR=1

termlm-indicator-color-active() {
  (( _TERMLM_PROMPT_USE_COLOR )) || return 1
  [[ -t 1 || -t 2 ]]
}

termlm-render-indicator() {
  local indicator="$1"
  local color="$2"
  if ! termlm-indicator-color-active; then
    print -r -- "$indicator"
    return 0
  fi
  if [[ "$indicator" == *"%F{"* || "$indicator" == *"%f"* || "$indicator" == *"%{"* ]]; then
    print -r -- "$indicator"
    return 0
  fi
  print -r -- "%F{$color}${indicator}%f"
}

termlm-current-prompt-indicator() {
  termlm-render-indicator "$_TERMLM_PROMPT_INDICATOR" "blue"
}

termlm-current-session-indicator() {
  termlm-render-indicator "$_TERMLM_SESSION_INDICATOR" "yellow"
}

termlm-load-prompt-settings() {
  local cfg="${XDG_CONFIG_HOME:-$HOME/.config}/termlm/config.toml"

  if [[ -n "${TERMLM_PROMPT_INDICATOR:-}" ]]; then
    _TERMLM_PROMPT_INDICATOR="${TERMLM_PROMPT_INDICATOR}"
  else
    _TERMLM_PROMPT_INDICATOR='● ? '
  fi

  if [[ -n "${TERMLM_SESSION_INDICATOR:-}" ]]; then
    _TERMLM_SESSION_INDICATOR="${TERMLM_SESSION_INDICATOR}"
  else
    _TERMLM_SESSION_INDICATOR='● /p '
  fi

  if [[ -n "${TERMLM_PROMPT_USE_COLOR:-}" ]]; then
    if [[ "${TERMLM_PROMPT_USE_COLOR}" == "0" || "${TERMLM_PROMPT_USE_COLOR:l}" == "false" ]]; then
      _TERMLM_PROMPT_USE_COLOR=0
    else
      _TERMLM_PROMPT_USE_COLOR=1
    fi
  else
    _TERMLM_PROMPT_USE_COLOR=1
  fi

  [[ ! -f "$cfg" ]] && return 0

  local in_prompt=0
  local line
  while IFS= read -r line; do
    if [[ "$line" =~ '^[[:space:]]*\[[^]]+\][[:space:]]*$' ]]; then
      if [[ "$line" == *"[prompt]"* ]]; then
        in_prompt=1
      else
        in_prompt=0
      fi
      continue
    fi

    (( in_prompt )) || continue

    if [[ -z "${TERMLM_PROMPT_INDICATOR:-}" ]] && [[ "$line" =~ '^[[:space:]]*indicator[[:space:]]*=[[:space:]]*"(.*)"[[:space:]]*$' ]]; then
      _TERMLM_PROMPT_INDICATOR="${match[1]}"
      continue
    fi
    if [[ -z "${TERMLM_SESSION_INDICATOR:-}" ]] && [[ "$line" =~ '^[[:space:]]*session_indicator[[:space:]]*=[[:space:]]*"(.*)"[[:space:]]*$' ]]; then
      _TERMLM_SESSION_INDICATOR="${match[1]}"
      continue
    fi
    if [[ -z "${TERMLM_PROMPT_USE_COLOR:-}" ]] && [[ "$line" =~ '^[[:space:]]*use_color[[:space:]]*=[[:space:]]*(true|false)[[:space:]]*$' ]]; then
      if [[ "${match[1]:l}" == "true" ]]; then
        _TERMLM_PROMPT_USE_COLOR=1
      else
        _TERMLM_PROMPT_USE_COLOR=0
      fi
      continue
    fi
  done < "$cfg"

  if [[ -z "${TERMLM_PROMPT_INDICATOR:-}" && "$_TERMLM_PROMPT_INDICATOR" == '?> ' ]]; then
    _TERMLM_PROMPT_INDICATOR='● ? '
  fi
  if [[ -z "${TERMLM_SESSION_INDICATOR:-}" && "$_TERMLM_SESSION_INDICATOR" == '?? ' ]]; then
    _TERMLM_SESSION_INDICATOR='● /p '
  fi
}

termlm-enter-prompt-mode() {
  _TERMLM_MODE="prompt"
  termlm-load-prompt-settings
  _TERMLM_SAVED_PS1="${_TERMLM_SAVED_PS1:-$PS1}"
  PS1="$(termlm-current-prompt-indicator)"
  zle -K termlm-prompt
  zle reset-prompt
}

termlm-exit-prompt-mode() {
  local redraw="${1:-1}"
  _TERMLM_MODE="normal"
  PS1="${_TERMLM_SAVED_PS1:-$PS1}"
  zle -K main
  if (( redraw != 0 )); then
    zle reset-prompt
  fi
}

termlm-enter-session-mode() {
  _TERMLM_SESSION_MODE=1
  _TERMLM_MODE="session"
  termlm-load-prompt-settings
  _TERMLM_SAVED_PS1="${_TERMLM_SAVED_PS1:-$PS1}"
  PS1="$(termlm-current-session-indicator)"
  zle -K termlm-prompt
  zle reset-prompt
}

termlm-exit-session-mode() {
  _TERMLM_SESSION_MODE=0
  _TERMLM_TASK_ID=""
  _TERMLM_CLARIFICATION_TASK_ID=""
  _TERMLM_APPROVAL_TASK_ID=""
  _TERMLM_APPROVAL_CMD=""
  _TERMLM_EDITING_APPROVAL_TASK_ID=""
  _TERMLM_OUTPUT_STARTED=0
  _TERMLM_OUTPUT_NEEDS_NEWLINE=0
  termlm-exit-prompt-mode
}

termlm-line-pre-redraw() {
  local desired_prompt desired_session
  desired_prompt="$(termlm-current-prompt-indicator)"
  desired_session="$(termlm-current-session-indicator)"
  if [[ "$_TERMLM_MODE" == "prompt" ]]; then
    if [[ "$PS1" != "$desired_prompt" ]]; then
      PS1="$desired_prompt"
    fi
    zle -K termlm-prompt
  elif [[ $_TERMLM_SESSION_MODE -eq 1 ]]; then
    if [[ "$PS1" != "$desired_session" ]]; then
      PS1="$desired_session"
    fi
    zle -K termlm-prompt
  elif [[ "$_TERMLM_MODE" == "normal" && "$KEYMAP" == "termlm-prompt" ]]; then
    zle -K main
  fi
}

termlm-line-init() {
  termlm-line-pre-redraw
  if [[ -n "${_TERMLM_DEFERRED_PROMPT:-}" ]]; then
    local deferred_prompt="$_TERMLM_DEFERRED_PROMPT"
    _TERMLM_DEFERRED_PROMPT=""
    termlm-start-task "$deferred_prompt"
  fi
}
