termlm-approval-prompt() {
  local cmd="$1"
  local cols
  cols=$(tput cols 2>/dev/null || echo 80)
  local max=$(( cols - 8 ))
  local shown="$cmd"
  if (( ${#shown} > max )); then
    shown="${shown[1,max-1]}…"
  fi

  print -r -- "┌─ proposed command ─────────────────────────────────────────────"
  print -r -- "│ $shown"
  print -r -- "└─ [y]es  [n]o(default)  [e]dit  [a]ll-in-this-task ─────────────"
}
