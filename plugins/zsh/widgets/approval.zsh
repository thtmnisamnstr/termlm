termlm-approval-prompt() {
  local cmd="$1"
  local cols
  cols=$(tput cols 2>/dev/null || echo 80)
  local max=$(( cols - 4 ))
  (( max < 24 )) && max=24

  print -r -- "┌─ proposed command ─────────────────────────────────────────────"
  local line part
  local -a lines
  lines=("${(@f)cmd}")
  (( ${#lines[@]} == 0 )) && lines=("")
  for line in "${lines[@]}"; do
    if [[ -z "$line" ]]; then
      print -r -- "│"
      continue
    fi
    while (( ${#line} > max )); do
      part="${line[1,max]}"
      print -r -- "│ $part"
      line="${line[max+1,-1]}"
    done
    print -r -- "│ $line"
  done
  print -r -- "├─ keys ─────────────────────────────────────────────────────────"
  print -r -- "│ y accept   n/Enter reject   e edit   a accept all   Esc cancel"
  print -r -- "└────────────────────────────────────────────────────────────────"
}
