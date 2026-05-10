#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macos compatibility: skipped (host is not macOS)"
  exit 0
fi

if ! command -v sw_vers >/dev/null 2>&1; then
  echo "compatibility failure: sw_vers is required on macOS" >&2
  exit 1
fi

version="$(sw_vers -productVersion)"
major="${version%%.*}"
arch="$(uname -m)"
zsh_version="$(zsh --version | awk '{print $2}')"

echo "macos compatibility: macOS=${version} arch=${arch} zsh=${zsh_version}"

if [[ "${major}" -lt 13 ]]; then
  echo "compatibility failure: macOS ${version} is below minimum supported 13" >&2
  exit 1
fi

if [[ "${arch}" != "arm64" ]]; then
  echo "macos compatibility: warning: non-Apple-Silicon host (${arch}); local provider is best-effort"
fi

zsh_major="${zsh_version%%.*}"
zsh_minor="${zsh_version#*.}"
zsh_minor="${zsh_minor%%.*}"

if [[ "${zsh_major}" -lt 5 ]] || { [[ "${zsh_major}" -eq 5 ]] && [[ "${zsh_minor}" -lt 8 ]]; }; then
  echo "compatibility failure: zsh ${zsh_version} is below minimum supported 5.8" >&2
  exit 1
fi

echo "macos compatibility checks passed."
