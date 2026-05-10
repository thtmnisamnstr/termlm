#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if ! command -v zsh >/dev/null 2>&1; then
  echo "compatibility failure: zsh is required" >&2
  exit 1
fi

if ! command -v expect >/dev/null 2>&1; then
  echo "compatibility failure: expect is required" >&2
  exit 1
fi

create_wrapper_plugin() {
  local plugin_dir="$1"
  local plugin_name="$2"
  mkdir -p -- "${plugin_dir}"
  case "${plugin_name}" in
    zsh-autosuggestions)
      cat > "${plugin_dir}/${plugin_name}.plugin.zsh" <<'EOF'
if (( $+widgets[self-insert] )); then
  zle -A self-insert _as_prev_self_insert
  _as_wrap_self_insert() { zle _as_prev_self_insert }
  zle -N self-insert _as_wrap_self_insert
fi
EOF
      ;;
    zsh-syntax-highlighting)
      cat > "${plugin_dir}/${plugin_name}.plugin.zsh" <<'EOF'
if (( $+widgets[self-insert] )); then
  zle -A self-insert _hl_prev_self_insert
  _hl_wrap_self_insert() { zle _hl_prev_self_insert }
  zle -N self-insert _hl_wrap_self_insert
fi
if (( $+widgets[accept-line] )); then
  zle -A accept-line _hl_prev_accept_line
  _hl_wrap_accept_line() { zle _hl_prev_accept_line }
  zle -N accept-line _hl_wrap_accept_line
fi
EOF
      ;;
    *)
      cat > "${plugin_dir}/${plugin_name}.plugin.zsh" <<'EOF'
# noop plugin
EOF
      ;;
  esac
}

setup_fake_omz() {
  local root="$1"
  local omz_root="${root}/oh-my-zsh"
  local custom_plugins="${omz_root}/custom/plugins"
  mkdir -p -- "${custom_plugins}/termlm"
  cp -R "${ROOT_DIR}/plugins/zsh/." "${custom_plugins}/termlm/"
  create_wrapper_plugin "${custom_plugins}/zsh-autosuggestions" "zsh-autosuggestions"
  create_wrapper_plugin "${custom_plugins}/zsh-syntax-highlighting" "zsh-syntax-highlighting"

  cat > "${omz_root}/oh-my-zsh.sh" <<'EOF'
for plugin in "${plugins[@]}"; do
  if [[ -f "${ZSH}/custom/plugins/${plugin}/${plugin}.plugin.zsh" ]]; then
    source "${ZSH}/custom/plugins/${plugin}/${plugin}.plugin.zsh"
    continue
  fi
  if [[ -f "${ZSH}/plugins/${plugin}/${plugin}.plugin.zsh" ]]; then
    source "${ZSH}/plugins/${plugin}/${plugin}.plugin.zsh"
    continue
  fi
done
EOF
}

setup_fake_plugin_repo_root() {
  local root="$1"
  local pm_root="${root}/plugin-repos"
  mkdir -p -- "${pm_root}/termlm"
  cp -R "${ROOT_DIR}/plugins/zsh/." "${pm_root}/termlm/"
  create_wrapper_plugin "${pm_root}/zsh-autosuggestions" "zsh-autosuggestions"
  create_wrapper_plugin "${pm_root}/zsh-syntax-highlighting" "zsh-syntax-highlighting"
  echo "${pm_root}"
}

run_mode() {
  local mode="$1"
  local tmp_root
  tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/termlm-plugin-matrix.${mode}.XXXXXX")"
  local zdotdir="${tmp_root}/zdotdir"
  local mock_client="${tmp_root}/mock-termlm-client.sh"
  local mock_log="${tmp_root}/mock.log"
  mkdir -p -- "${zdotdir}" "${tmp_root}/xdg-config"
  : > "${mock_log}"

  cat > "${mock_client}" <<'EOF'
#!/usr/bin/env zsh
set -euo pipefail
log_file="${TERMLM_MOCK_LOG:?missing TERMLM_MOCK_LOG}"
json_field() {
  local line="$1"
  local key="$2"
  local pattern="\"${key}\":\"([^\"]*)\""
  if [[ "$line" =~ $pattern ]]; then
    print -r -- "${match[1]}"
  fi
}
encode_b64() {
  local input="$1"
  print -rn -- "$input" | base64 | tr -d '\n'
}
case "${1:-}" in
  status)
    exit 0
    ;;
  bridge)
    shell_id="00000000-0000-0000-0000-00000000b001"
    print -r -- "{\"event\":\"shell_registered\",\"shell_id\":\"${shell_id}\"}"
    print -r -- "event:shell_registered:${shell_id}" >> "${log_file}"
    while IFS= read -r line; do
      print -r -- "recv:${line}" >> "${log_file}"
      if [[ "$line" == *'"op":"start_task"'* ]]; then
        task_id="$(json_field "$line" "task_id")"
        mode="$(json_field "$line" "mode")"
        print -r -- "event:start_task:${task_id}:${mode}" >> "${log_file}"
        print -r -- "{\"event\":\"task_started\",\"task_id\":\"${task_id}\"}"
        msg_b64="$(encode_b64 "plugin-matrix-ok")"
        print -r -- "{\"event\":\"model_text\",\"task_id\":\"${task_id}\",\"chunk_b64\":\"${msg_b64}\"}"
        print -r -- "{\"event\":\"task_complete\",\"task_id\":\"${task_id}\"}"
      elif [[ "$line" == *'"op":"shell_context"'* ]]; then
        print -r -- "event:shell_context" >> "${log_file}"
      elif [[ "$line" == *'"op":"unregister_shell"'* ]]; then
        print -r -- "event:unregister_shell" >> "${log_file}"
        break
      fi
    done
    ;;
esac
EOF
  chmod +x "${mock_client}"

  local pm_root=""
  if [[ "${mode}" == "omz" ]]; then
    setup_fake_omz "${tmp_root}"
  elif [[ "${mode}" == "zinit" || "${mode}" == "antidote" ]]; then
    pm_root="$(setup_fake_plugin_repo_root "${tmp_root}")"
  fi

  cat > "${zdotdir}/.zshrc" <<EOF
export TERMLM_CLIENT_BIN="${mock_client}"
export TERMLM_CORE_BIN="/bin/false"
export TERMLM_MOCK_LOG="${mock_log}"
export XDG_CONFIG_HOME="${tmp_root}/xdg-config"
export TERMLM_CAPTURE_ENABLED=0
export TERMLM_PROMPT_INDICATOR='TERMLM_PROMPT> '
export TERMLM_SESSION_INDICATOR='TERMLM_SESSION> '
export PS1='TERMLM_NORMAL> '
bindkey -e
EOF

  case "${mode}" in
    plain)
      cat >> "${zdotdir}/.zshrc" <<EOF
source "${ROOT_DIR}/plugins/zsh/termlm.plugin.zsh"
EOF
      ;;
    omz)
      cat >> "${zdotdir}/.zshrc" <<EOF
export ZSH="${tmp_root}/oh-my-zsh"
plugins=(termlm zsh-autosuggestions zsh-syntax-highlighting)
source "\$ZSH/oh-my-zsh.sh"
EOF
      ;;
    zinit)
      cat >> "${zdotdir}/.zshrc" <<EOF
TERMLM_PM_ROOT="${pm_root}"
zi() {
  if [[ "\$1" == "light" ]]; then
    local repo="\$2"
    local name="\${repo##*/}"
    source "\${TERMLM_PM_ROOT}/\${name}/\${name}.plugin.zsh"
  fi
}
zi light user/termlm
zi light zsh-users/zsh-autosuggestions
zi light zsh-users/zsh-syntax-highlighting
EOF
      ;;
    antidote)
      cat >> "${zdotdir}/.zshrc" <<EOF
TERMLM_PM_ROOT="${pm_root}"
antidote() {
  if [[ "\$1" == "bundle" ]]; then
    local repo="\$2"
    local name="\${repo##*/}"
    source "\${TERMLM_PM_ROOT}/\${name}/\${name}.plugin.zsh"
  fi
}
antidote bundle user/termlm
antidote bundle zsh-users/zsh-autosuggestions
antidote bundle zsh-users/zsh-syntax-highlighting
EOF
      ;;
    *)
      echo "unsupported mode: ${mode}" >&2
      rm -rf -- "${tmp_root}"
      return 1
      ;;
  esac

  TERMLM_EXPECT_ZDOTDIR="${zdotdir}" expect <<'EOF' >/dev/null 2>&1
set timeout 30
set zdotdir $env(TERMLM_EXPECT_ZDOTDIR)
spawn env TERM=xterm-256color ZDOTDIR=$zdotdir zsh -i
expect -re {TERMLM_NORMAL> }
send -- "?"
expect -re {TERMLM_PROMPT> }
send -- "verify plugin manager\r"
expect -re {TERMLM_NORMAL> }
send -- "/p\r"
expect -re {TERMLM_SESSION> }
send -- "session request\r"
expect -re {TERMLM_SESSION> }
send -- "/q\r"
expect -re {TERMLM_NORMAL> }
send -- "exit\r"
expect eof
EOF

  if ! rg -q --fixed-strings -- 'event:shell_registered' "${mock_log}"; then
    echo "compatibility failure (${mode}): shell did not register" >&2
    cat "${mock_log}" >&2 || true
    rm -rf -- "${tmp_root}"
    return 1
  fi
  if ! rg -q --pcre2 'event:start_task:[^:]+:\?' "${mock_log}"; then
    echo "compatibility failure (${mode}): no prompt-mode task observed" >&2
    cat "${mock_log}" >&2 || true
    rm -rf -- "${tmp_root}"
    return 1
  fi
  if ! rg -q --pcre2 'event:start_task:[^:]+:/p' "${mock_log}"; then
    echo "compatibility failure (${mode}): no session-mode task observed" >&2
    cat "${mock_log}" >&2 || true
    rm -rf -- "${tmp_root}"
    return 1
  fi
  if ! rg -q --fixed-strings -- 'event:shell_context' "${mock_log}"; then
    echo "compatibility failure (${mode}): shell context event missing" >&2
    cat "${mock_log}" >&2 || true
    rm -rf -- "${tmp_root}"
    return 1
  fi

  rm -rf -- "${tmp_root}"
}

modes=(plain omz zinit antidote)
for mode in "${modes[@]}"; do
  echo "plugin-manager compatibility: ${mode}"
  run_mode "${mode}"
done

echo "plugin-manager compatibility checks passed."
