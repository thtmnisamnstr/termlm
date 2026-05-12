# Getting Started

This guide is for first-time installation and first-use validation.
After it is installed, `termlm` is meant to feel like a small command helper inside your normal zsh prompt: ask for the shell task, inspect the proposed command, then approve or edit it.

## 1) Check Support Scope

- Shell: `zsh` only (v1)
- Platform: macOS 13+ on Apple Silicon for official local-provider support

## 2) Install

Required tooling:

- `curl`
- `python3`
- `shasum`

Quick install from releases:

```bash
curl -fsSL https://raw.githubusercontent.com/thtmnisamnstr/termlm/main/scripts/install.sh | bash
```

Install notes:

- installer waits for runtime/model/index readiness by default
- readiness and model-chunk download phases emit periodic progress lines
- first install can take several minutes depending on model/index state
- a clean install downloads local model assets and builds both vector and lexical command-doc indexes
- web search/read is available by default through DuckDuckGo HTML search and does not need an API key

Manual install is documented in [release-upgrades.md](release-upgrades.md).

## 3) Enable in zsh

```bash
termlm init zsh
```

Reload zsh in the current terminal, or open a new terminal tab:

```bash
exec zsh -l
```

## 4) Health Check

```bash
termlm doctor --strict
termlm status
```

If either command fails, use [troubleshooting.md](troubleshooting.md). The zsh plugin will also try to start the daemon automatically the first time you enter prompt mode.

## 5) First Commands

At an empty prompt:

- `?` enters one-shot prompt mode
- `/p` enters session mode
- `/q` exits session mode
- `Esc` cancels a prompt, response, clarification, approval, or session and returns to normal zsh
- clarification questions are answered at the `? ` prompt; `/p` stays in session until `/q`

Approval keys for proposed commands:

- `y` approve current command
- `n` reject current command
- `e` edit command inline before execute
- `a` approve all remaining commands in the task

Try these first:

```text
? where am i
? create a directory called archive
? find files containing TODO
? show me what changed in this git branch
```

## 6) Upgrade and Uninstall

Upgrade:

```bash
termlm upgrade
```

Uninstall:

```bash
termlm uninstall --yes
```

## 7) Next Docs

- Day-to-day operation: [operator-runbook.md](operator-runbook.md)
- Full config: [configuration.md](configuration.md)
- Command reference: [cli-reference.md](cli-reference.md)
- Issues: [troubleshooting.md](troubleshooting.md)
