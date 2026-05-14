# termlm

Command-line work has a lot of context: local tools, shell state, project conventions, recent output, and platform-specific flags.

`termlm` is a shell-native AI assistant for zsh that uses that context to propose commands without taking over your terminal.

Type `?`, describe the task, review the proposed command, and approve or edit it. Approved commands run through your real zsh execution path, so aliases, functions, working directory, history, hooks, and normal terminal behavior still apply.

If the request is too vague to turn into a command safely, `termlm` asks a focused follow-up instead of guessing.

It is built for developers who want AI assistance inside the terminal workflow they already use, with local command docs, explicit approval, and safety checks in the loop.

## What It Feels Like

```text
~/project % ?
?> create a directory called archive

proposed command
  mkdir -p archive

y accept   n/Enter reject   e edit   a accept all   Esc cancel
```

Approve with `y`, reject with `n` or Enter, edit with `e`, approve the rest of the current task with `a`, or cancel with `Esc`.

Good first prompts:

```text
? where am i
? create a directory called archive
? find Python files containing TODO
? show me what changed in this git branch
? list files by size, largest first
```

For a longer back-and-forth, enter session mode with `/p` and leave it with `/q`.
`Esc` cancels the current prompt or response and returns you to the normal zsh prompt.

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

Install also writes a small filesystem context snapshot under `~/.local/share/termlm/context/`. It records your home path, standard home folders, and a bounded current-directory listing so prompts like “where is my Desktop?” have useful local context before the model has to reason. The snapshot is refreshed when the zsh plugin loads and when you run `termlm reload-config`.

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
| `Esc` | Cancel the current prompt/response or leave session mode |

Prompt mode uses a blue `● ?` indicator, and long-running responses show a short `termlm: thinking...` status so the shell does not look stuck.

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

Use `termlm reindex --mode delta` for normal PATH/tooling changes. `full` is a heavier repair option for incompatible or corrupt index state; `compact` rewrites index files to remove tombstones.

## How It Works

`termlm` has two pieces:

- a small zsh adapter in `plugins/zsh/`
- a Rust daemon, `termlm-core`, that handles indexing, retrieval, planning, validation, model calls, and safety checks

For command prompts, the daemon gathers local shell context, gives the model read-only tools, and lets the model decide what it needs before proposing anything. It can run hybrid command-doc retrieval, look up exact command docs, inspect bounded local files/project/git context, run tightly allowlisted read-only shell probes, and fall back to web search/read when local grounding is missing or current information matters. Those intermediate observations stay out of the terminal UI; users see the final answer, a clarification question, or a proposed command for approval.

Retrieved command-doc chunks are expanded back to their source document before they are sent to the model, then deduplicated by command. While indexing, `termlm` also adds a small generated “usage intents” section to each command document from the local `man`/`--help` options, so prompts like “files only, not directories” can match `find -type f` instead of relying on raw man-page wording alone. If there still is not enough signal, `termlm` asks a clarification question instead of substituting a hard-coded answer.

## Inference And Privacy

Default behavior is local-first:

- local generation model: bundled Gemma GGUF asset
- local embedding model: bundled BGE-small GGUF asset
- no telemetry
- web search/read are enabled out of the box with the no-token DuckDuckGo HTML provider, used for current information and as a fallback when local command docs are missing or insufficient

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

Current zsh sessions refresh stale `termlm` helper state automatically after upgrade. If you are upgrading from an older alpha and the next prompt cannot reach the daemon, run `exec zsh -l` once.

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

Run the response accuracy gate before commits:

```bash
bash scripts/ci/run_accuracy_gate.sh --level commit
```

Before release, run the stricter accuracy path on a machine with the bundled GGUF models installed:

```bash
TERMLM_ACCURACY_REQUIRE_REAL=1 \
bash scripts/ci/run_accuracy_gate.sh --level release
```

The release path drives the actual zsh plugin in a PTY, submits prompts with `?`, approves commands, checks retrieval traces, exercises compound command flows, and verifies the local web-search/read path.

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
