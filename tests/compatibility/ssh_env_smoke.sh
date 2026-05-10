#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PTY_CONTRACT="${ROOT_DIR}/tests/adapter-contract/zsh_pty_contract.zsh"

if ! command -v zsh >/dev/null 2>&1; then
  echo "compatibility failure: zsh is required" >&2
  exit 1
fi

if ! command -v expect >/dev/null 2>&1; then
  echo "compatibility failure: expect is required" >&2
  exit 1
fi

echo "ssh compatibility: exercising adapter contract with SSH session env vars"
SSH_CONNECTION="192.0.2.10 60000 192.0.2.20 22" \
SSH_CLIENT="192.0.2.10 60000 22" \
SSH_TTY="/dev/ttys999" \
TERMLM_TEST_TERM="screen-256color" \
  zsh "${PTY_CONTRACT}"

echo "ssh compatibility checks passed."
