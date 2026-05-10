# termlm zsh adapter (v1)

[[ "${TERMLM_DISABLE:-0}" == "1" ]] && return 0

if [[ -n "${_TERMLM_PLUGIN_LOADED:-}" ]]; then
  return 0
fi
export _TERMLM_PLUGIN_LOADED=1

_TERMLM_MODE="normal"
_TERMLM_SESSION_MODE=0
_TERMLM_SAVED_PS1="${PS1}"
_TERMLM_SHELL_ID=""
_TERMLM_HELPER_PID=""
_TERMLM_HELPER_IN_FD=""
_TERMLM_HELPER_OUT_FD=""
_TERMLM_HELPER_FIFO_IN=""
_TERMLM_HELPER_FIFO_OUT=""
_TERMLM_HELPER_FIFO_GUARD_IN_FD=""
_TERMLM_HELPER_FIFO_GUARD_OUT_FD=""
_TERMLM_TASK_ID=""
_TERMLM_PENDING_TASK_ID=""
_TERMLM_PENDING_CMD=""
_TERMLM_PENDING_CWD_BEFORE=""
_TERMLM_PENDING_STARTED_AT=0
_TERMLM_PENDING_STDOUT_FILE=""
_TERMLM_PENDING_STDERR_FILE=""
_TERMLM_PENDING_SEQ=0
_TERMLM_LAST_CONTEXT_HASH=""
_TERMLM_WAITING_MODEL=0
_TERMLM_RUN_DIR="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/termlm-${UID}/run-$$"
mkdir -p -- "$_TERMLM_RUN_DIR"

source "${0:A:h}/widgets/prompt-mode.zsh"
source "${0:A:h}/widgets/self-insert.zsh"
source "${0:A:h}/widgets/accept-line.zsh"
source "${0:A:h}/widgets/delete-char-or-list.zsh"
source "${0:A:h}/widgets/approval.zsh"
source "${0:A:h}/widgets/safety-floor.zsh"
source "${0:A:h}/lib/ipc.zsh"
source "${0:A:h}/lib/capture.zsh"
source "${0:A:h}/lib/terminal-observer.zsh"
source "${0:A:h}/lib/shell-context.zsh"

autoload -Uz add-zsh-hook

termlm-load-prompt-settings
termlm-load-capture-settings
termlm-load-observer-settings

zle -N self-insert termlm-self-insert
zle -N accept-line termlm-accept-line
zle -N zle-line-pre-redraw termlm-line-pre-redraw
zle -N termlm-delete-char-or-list

if ! bindkey -l | grep -qx 'termlm-prompt'; then
  bindkey -N termlm-prompt main
fi
bindkey -M termlm-prompt '^D' termlm-delete-char-or-list

add-zsh-hook preexec termlm-preexec
add-zsh-hook precmd termlm-precmd
add-zsh-hook zshexit termlm-zshexit

termlm-register-shell
termlm-send-shell-context
