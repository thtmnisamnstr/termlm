#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PTY_CONTRACT="${ROOT_DIR}/tests/adapter-contract/zsh_pty_contract.zsh"
WRAPPER_INTEROP="${ROOT_DIR}/tests/compatibility/zsh_wrapper_interop.zsh"

if ! command -v zsh >/dev/null 2>&1; then
  echo "compatibility failure: zsh is required" >&2
  exit 1
fi

if ! command -v expect >/dev/null 2>&1; then
  echo "compatibility failure: expect is required" >&2
  exit 1
fi

terms=(
  "screen-256color"
  "tmux-256color"
  "xterm-kitty"
  "alacritty"
)

for term in "${terms[@]}"; do
  echo "terminal compatibility: TERM=${term}"
  TERMLM_TEST_TERM="${term}" zsh "${PTY_CONTRACT}"
done

echo "terminal compatibility: wrapper interop (autosuggestions/highlighting load order)"
zsh "${WRAPPER_INTEROP}"

echo "terminal compatibility checks passed."
