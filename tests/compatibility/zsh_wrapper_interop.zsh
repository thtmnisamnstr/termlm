#!/usr/bin/env zsh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"

source "${ROOT_DIR}/plugins/zsh/widgets/prompt-mode.zsh"
source "${ROOT_DIR}/plugins/zsh/widgets/self-insert.zsh"
source "${ROOT_DIR}/plugins/zsh/widgets/accept-line.zsh"

fail() {
  print -r -- "wrapper-interop failure: $*" >&2
  exit 1
}

assert_eq() {
  local got="$1"
  local expected="$2"
  local msg="$3"
  [[ "$got" == "$expected" ]] || fail "$msg (got='$got' expected='$expected')"
}

typeset -gA _WIDGETS
typeset -ga _ZLE_CALLS
typeset -g _ACCEPT_LINE_COUNT=0
typeset -g _ABANDON_COUNT=0

zle() {
  _ZLE_CALLS+=("$*")
  case "${1:-}" in
    -N)
      _WIDGETS["${2:-}"]="${3:-}"
      ;;
    -K)
      KEYMAP="${2:-main}"
      ;;
    .self-insert)
      LBUFFER+="${KEYS:-}"
      BUFFER="${LBUFFER}${RBUFFER:-}"
      CURSOR=${#LBUFFER}
      ;;
    .accept-line)
      _ACCEPT_LINE_COUNT=$(( _ACCEPT_LINE_COUNT + 1 ))
      ;;
    reset-prompt)
      ;;
    *)
      local widget="${1:-}"
      local fn="${_WIDGETS["$widget"]-}"
      if [[ -z "$fn" ]]; then
        fail "unknown widget invocation: $widget"
      fi
      "$fn"
      ;;
  esac
}

termlm-abandon-active-task() {
  _ABANDON_COUNT=$(( _ABANDON_COUNT + 1 ))
  _TERMLM_WAITING_MODEL=0
}

reset_state() {
  _WIDGETS=()
  _ZLE_CALLS=()
  _ACCEPT_LINE_COUNT=0
  _ABANDON_COUNT=0
  KEYMAP="main"
  PS1='$ '
  BUFFER=""
  LBUFFER=""
  RBUFFER=""
  CURSOR=0
  KEYS=""
  _TERMLM_MODE="normal"
  _TERMLM_SESSION_MODE=0
  _TERMLM_SAVED_PS1='$ '
  _TERMLM_PROMPT_INDICATOR='?> '
  _TERMLM_SESSION_INDICATOR='?? '
  _TERMLM_WAITING_MODEL=0
}

reset_state

# termlm install
zle -N self-insert termlm-self-insert
zle -N accept-line termlm-accept-line

# autosuggestions-style wrapper loaded after termlm
typeset -g _AUTOSUGGEST_ORIG_SELF_INSERT="${_WIDGETS["self-insert"]}"
autosuggest-self-insert-wrapper() {
  "$_AUTOSUGGEST_ORIG_SELF_INSERT"
}
zle -N self-insert autosuggest-self-insert-wrapper

# syntax-highlighting-style wrapper loaded after autosuggestions
typeset -g _SYNTAX_ORIG_SELF_INSERT="${_WIDGETS["self-insert"]}"
typeset -g _SYNTAX_ORIG_ACCEPT_LINE="${_WIDGETS["accept-line"]}"
syntax-highlight-self-insert-wrapper() {
  "$_SYNTAX_ORIG_SELF_INSERT"
}
syntax-highlight-accept-line-wrapper() {
  "$_SYNTAX_ORIG_ACCEPT_LINE"
}
zle -N self-insert syntax-highlight-self-insert-wrapper
zle -N accept-line syntax-highlight-accept-line-wrapper

# ensure ? trigger still reaches termlm through wrappers
KEYS='?'
zle self-insert
assert_eq "$_TERMLM_MODE" "prompt" "wrapped self-insert should preserve termlm prompt-mode entry"
assert_eq "$KEYMAP" "termlm-prompt" "wrapped self-insert should still switch keymap"
assert_eq "$BUFFER" "" "prompt entry should clear BUFFER"

# ensure implicit abort still works through wrapped accept-line
_TERMLM_MODE="prompt"
_TERMLM_WAITING_MODEL=1
BUFFER="ls -la"
LBUFFER="$BUFFER"
zle accept-line
assert_eq "$_ABANDON_COUNT" "1" "wrapped accept-line should preserve implicit abort behavior"
assert_eq "$BUFFER" "ls -la" "implicit abort should preserve typed command in BUFFER"
assert_eq "$CURSOR" "${#BUFFER}" "cursor should move to end after implicit abort"

print -r -- "zsh wrapper interop checks passed."
