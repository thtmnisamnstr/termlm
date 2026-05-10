# Getting Started

This guide is for first-time installation and first-use validation.

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

Manual install is documented in [release-upgrades.md](release-upgrades.md).

## 3) Enable in zsh

```bash
termlm init zsh
```

Open a new zsh session after this step.

## 4) Health Check

```bash
termlm doctor --strict
termlm status
```

If status reports daemon unreachable, run:

```bash
termlm-core --detach
```

then retry `termlm status`.

## 5) First Commands

At an empty prompt:

- `?` enters one-shot prompt mode
- `/p` enters session mode
- `/q` exits session mode

Approval keys for proposed commands:

- `y` approve current command
- `n` reject current command
- `e` edit command before execute
- `a` approve all remaining commands in the task

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
