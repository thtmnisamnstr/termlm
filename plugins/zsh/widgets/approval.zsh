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

  local key=""
  if ! read -k 1 -s key; then
    echo
    echo "abort"
    return
  fi
  echo

  case "$key" in
    y|Y) echo "approved" ;;
    a|A) echo "approve_all" ;;
    e|E)
      local editor="${EDITOR:-vi}"
      local tmp="${TMPDIR:-/tmp}/termlm-edit-${_TERMLM_TASK_ID:-manual}.sh"
      print -r -- "$cmd" > "$tmp"
      "$editor" "$tmp"
      local edited
      edited="$(<"$tmp")"
      edited="${edited%$'\n'}"
      echo "edited:$edited"
      rm -f -- "$tmp"
      ;;
    $'\x03'|$'\x1b') echo "abort" ;;
    $'\r'|$'\n'|"") echo "rejected" ;;
    n|N) echo "rejected" ;;
    *) echo "rejected" ;;
  esac
}
