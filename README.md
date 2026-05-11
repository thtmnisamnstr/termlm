# termlm

Command-line work has a lot of context: local tools, shell state, project conventions, recent output, and platform-specific flags.

`termlm` is a shell-native AI assistant for zsh that uses that context to propose commands without taking over your terminal.

Type `?`, describe the task, review the proposed command, and approve or edit it. Approved commands run through your real zsh execution path, so aliases, functions, working directory, history, hooks, and normal terminal behavior still apply.

It is built for developers who want AI assistance inside the terminal workflow they already use, with local command docs, explicit approval, and safety checks in the loop.

## What It Feels Like

```text
~/project % ?
?> create a directory called archive

proposed command
  mkdir -p archive

[y]es  [n]o  [e]dit  [a]ll-in-this-task
```

Approve with `y`, reject with `n`, edit with `e`, or approve the rest of the current task with `a`.

Good first prompts:

```text
? where am i
? create a directory called archive
? find Python files containing TODO
? show me what changed in this git branch
? list files by size, largest first
```

For a longer back-and-forth, enter session mode with `/p` and leave it with `/q`.

## Why Use It

- **It runs in your shell, not beside it.** Approved commands are inserted into zsh and executed with `zle .accept-line`, not sent through a separate PTY.
- **It checks local command docs.** `termlm` builds a searchable index from installed command docs (`man`, `--help`, `-h`) and uses hybrid vector plus BM25 retrieval before proposing commands.
- **It makes approval explicit.** You always see the proposed command before execution unless you intentionally change approval settings.
- **It has a hard safety floor.** Catastrophic command patterns are blocked in both the daemon and the zsh adapter.
- **It works locally by default.** Release bundles include local GGUF model assets for generation and embeddings, with optional Ollama support if you prefer that.
- **It keeps upgrades light.** `termlm upgrade` installs platform binaries and plugin updates while preserving downloaded model files.

## Install

Supported first-version setup:

- macOS 13+ on Apple Silicon
- zsh 5.8+
- `curl`, `python3`, and `shasum`

Install the latest release:

```bash
curl -fsSL https://raw.githubusercontent.com/thtmnisamnstr/termlm/main/scripts/install.sh | bash
```

The first install downloads model chunks, verifies them, starts the daemon, and waits for local command indexing to be ready. That can take several minutes on a clean machine.

Enable the zsh plugin:

```bash
termlm init zsh
```

Reload zsh in the current terminal, or open a new terminal tab:

```bash
exec zsh -l
```

Then run:

```bash
termlm doctor --strict
termlm status
```

If either command fails, use the [Troubleshooting](docs/troubleshooting.md) checklist. The zsh plugin will also try to start the daemon automatically the first time you enter prompt mode.

## Daily Use

At an empty zsh prompt:

| Input | Meaning |
|---|---|
| `?` | One-shot prompt mode |
| `/p` | Session mode for follow-up prompts |
| `/q` | Exit session mode |

Approval controls:

| Key | Action |
|---|---|
| `y` | Approve and run |
| `n` | Reject |
| `e` | Edit before running |
| `a` | Approve remaining commands in this task |

Common CLI commands:

```bash
termlm status --verbose
termlm doctor --strict
termlm reindex --mode delta
termlm reload-config
termlm stop
```

Use `termlm reindex --mode full` after major PATH/tooling changes, or `termlm reindex --mode compact` to rebuild and compact index files.

## How It Works

`termlm` has two pieces:

- a small zsh adapter in `plugins/zsh/`
- a Rust daemon, `termlm-core`, that handles indexing, retrieval, planning, validation, model calls, and safety checks

For command prompts, the daemon gathers local shell context, retrieves relevant command docs, asks the model for a structured command proposal, validates the result, and then sends the proposed command back to the zsh adapter. If the local model fails to produce a structured command for a known simple request, `termlm` can fall back to a conservative built-in command draft.

## Inference And Privacy

Default behavior is local-first:

- local generation model: bundled Gemma GGUF asset
- local embedding model: bundled BGE-small GGUF asset
- no telemetry
- web tools are controlled by config and used only for web/current-information tasks

Optional Ollama generation is available by setting:

```toml
[inference]
provider = "ollama"
```

See [docs/configuration.md](docs/configuration.md) for the full config guide.

## Upgrade Or Uninstall

Upgrade:

```bash
termlm upgrade
```

Upgrade downloads the platform `no-models` bundle, verifies checksums, installs binaries and the zsh plugin, preserves local model files, and checks that the installed binaries run.

Uninstall:

```bash
termlm uninstall --yes
```

## Documentation

Start here:

- [Getting Started](docs/getting-started.md)
- [Quick Recipes](docs/quick-recipes.md)
- [Troubleshooting](docs/troubleshooting.md)

Reference docs:

- [Documentation Hub](docs/README.md)
- [Configuration](docs/configuration.md)
- [CLI Reference](docs/cli-reference.md)
- [zsh Adapter](docs/zsh-adapter.md)
- [Release And Upgrade Notes](docs/release-upgrades.md)
- [Operator Runbook](docs/operator-runbook.md)
- [Compatibility Policy](docs/compatibility-policy.md)

Engineering evidence:

- [Spec Conformance Matrix](docs/spec-conformance-matrix.md)
- [Requirements](docs/requirements.md)
- [Performance](docs/performance.md)
- [Local CI](docs/local-ci.md)

## Contributor Workflow

Run local workflow-equivalent CI:

```bash
bash scripts/ci/run_local_ci.sh
```

Fast iteration:

```bash
bash scripts/ci/run_local_ci.sh --quick
```

`--quick` skips reliability, security, hardware matrix, Ollama parity, and release-packaging/rehearsal lanes.

GitHub workflow split:

- `.github/workflows/ci.yml`: fast push/PR gate
- `.github/workflows/extended-validation.yml`: manual full validation/perf evidence lane

Long-run reliability and upgrade rehearsal:

```bash
bash tests/reliability/soak_24h.sh /tmp/termlm-soak-24h
bash tests/release/upgrade_rehearsal.sh
```

## Policies

- [CONTRIBUTING.md](CONTRIBUTING.md)
- [SECURITY.md](SECURITY.md)
- [SUPPORT.md](SUPPORT.md)
