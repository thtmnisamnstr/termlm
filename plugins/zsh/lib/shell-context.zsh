termlm-build-alias-lines() {
  local -a alias_lines
  alias_lines=("${(@f)$(alias)}")
  print -l -- $alias_lines
}

termlm-build-function-lines() {
  local -a func_lines
  local f
  for f in ${(k)functions}; do
    [[ "$f" = (_*|*-*) ]] && continue
    func_lines+=("$f|${functions[$f]:0:1024}")
  done
  print -l -- $func_lines
}
