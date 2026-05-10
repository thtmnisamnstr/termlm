# Adapter-side immutable floor (defense in depth)
_TERMLM_SAFETY_FLOOR_PATTERNS=(
  '^[[:space:]]*rm[[:space:]]+-[[:alpha:]]*r[[:alpha:]]*[[:space:]]+/([[:space:]]|$)'
  '^[[:space:]]*rm[[:space:]]+-[[:alpha:]]*r[[:alpha:]]*[[:space:]]+(\$HOME|~)(/|[[:space:]]|$)'
  '^[[:space:]]*rm[[:space:]]+-[[:alpha:]]*r[[:alpha:]]*[[:space:]]+/\*'
  '^[[:space:]]*:\(\)[[:space:]]*\{[[:space:]]*:[[:space:]]*\|[[:space:]]*:[[:space:]]*&[[:space:]]*\}[[:space:]]*;[[:space:]]*:'
  '(^|[[:space:]])dd[[:space:]]+.*of=/dev/(disk|rdisk|sd|nvme)'
  '>[[:space:]]*/dev/(disk|rdisk|sd|nvme)'
  '(^|[[:space:]])mkfs(\.[[:alnum:]_]+)?[[:space:]]+/dev/'
  '^[[:space:]]*sudo[[:space:]]+rm[[:space:]]+-[[:alpha:]]*r'
  '(^|[[:space:]])rm[[:space:]]+-[[:alpha:]]*r[[:alpha:]]*[[:space:]]+/(System|Library|usr|bin|sbin|etc|var)(/|[[:space:]]|$)'
  '(^|[[:space:]])(chmod|chown)[[:space:]]+-R[[:space:]]+[^[:space:]]+[[:space:]]+/([[:space:]]|$)'
  '>[[:space:]]*/(System|Library|usr|bin|sbin|etc)/'
  '(^|[[:space:]])diskutil[[:space:]]+(eraseDisk|eraseVolume|secureErase)'
  '(^|[[:space:]])csrutil[[:space:]]+disable'
  '(^|[[:space:]])spctl[[:space:]]+--master-disable'
  '(^|[[:space:]])nvram[[:space:]]+-c'
)

termlm-safety-floor-match() {
  local cmd="$1"
  local pat
  for pat in $_TERMLM_SAFETY_FLOOR_PATTERNS; do
    if [[ "$cmd" =~ "$pat" ]]; then
      echo "$pat"
      return 0
    fi
  done
  return 1
}
