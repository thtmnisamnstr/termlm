#!/usr/bin/env bash
set -euo pipefail

if ! command -v shellcheck >/dev/null 2>&1; then
  echo "shellcheck is required" >&2
  exit 1
fi

mapfile_scripts() {
  find "$@" -type f | while IFS= read -r f; do
    if head -n 1 "$f" | grep -Eq '^#!/usr/bin/env bash|^#!/bin/bash'; then
      printf '%s\n' "$f"
    fi
  done
}

# Bash only. zsh plugin scripts are intentionally excluded.
FILES="$(mapfile_scripts scripts tests)"
if [[ -z "${FILES}" ]]; then
  echo "no bash scripts found"
  exit 0
fi

# shellcheck disable=SC2086
shellcheck ${FILES}

echo "shell lint passed"
