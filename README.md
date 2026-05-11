# termlm

`termlm` is a low-latency, safety-first, shell-native AI assistant for zsh, built on a shell-neutral Rust daemon, bundled with Google Gemma edge models for local inference (with optional Ollama connectivity).

## Why termlm

- Shell-native execution, not PTY emulation: approved commands execute through zsh `BUFFER` + `.accept-line`, so aliases, functions, history, and shell hooks behave like normal typed commands.
- Defense-in-depth safety: an immutable catastrophic-command safety floor is enforced in both daemon and adapter, plus critical-pattern checks and explicit approval controls (`y/n/e/a`).
- Structured command generation only: command proposals come from structured tool-calls; there is no prose-to-command fallback path.
- Hallucination-resistant command quality: hybrid docs retrieval (`man`, `--help`, `-h`) plus command-existence/flag validation in a bounded plan/revise loop before proposals are shown.
- Performance by design contract: warm-path startup, dynamic context/tool budgeting, mmap-backed indexes, f16 vectors, and perf gates enforced in CI/local evidence.
- Local inference included by default: release bundles include Google Gemma edge model artifacts for on-device local inference without a separate model setup step.
- Optional Ollama provider: `termlm` can route inference to local Ollama when configured, while preserving the same orchestration and safety/approval flow.
- Local-first grounding with controlled web: read-only local tools and terminal context are preferred; web access is routed only when needed and guarded (SSRF/redirect/robots/cache controls) with source labeling.
- Lightweight upgrades: `termlm upgrade` installs platform `no-models` bundles, verifies checksums, preserves local model files, and removes temporary artifacts.
- Private-by-default posture: no telemetry, local provider default, explicit opt-in for remote Ollama and web behavior.

## Differentiators At A Glance

| Area | termlm |
|---|---|
| Shell integration | Uses real zsh execution path (`BUFFER` + `.accept-line`) instead of bypassing shell semantics |
| Safety model | Immutable floor in daemon + adapter with approval/edit flow and critical-pattern blocking |
| Command correctness | Retrieval + existence/flag validation + revise loop before surfacing commands |
| Latency model | Warmed runtime/index path with bounded context/tool exposure and perf-gated targets |
| Inference options | Bundled Google Gemma edge local inference by default, optional local Ollama provider |
| Operational UX | One-command upgrade path with no-model artifacts and deterministic cleanup |

## Inference Options

- Default: bundled Google Gemma edge model artifacts used for local on-device inference.
- Optional: Ollama provider (`inference.provider = "ollama"`) for local Ollama-backed inference.
- Provider behavior stays consistent at the orchestrator layer (planning, safety floor, approvals, tool routing, and validation flow).

## Support Scope (v1)

- Shell adapter: `zsh` (`>= 5.8`)
- Official local-provider baseline: macOS 13+ on Apple Silicon (`darwin-arm64`)
- Bash/fish adapters are not supported in v1

## Privacy and Network Posture

- No telemetry is collected or sent.
- Default path is local-first (`inference.provider = "local"`).
- Network access is limited to configured web tools and optional Ollama usage.

## Quick Start

Prerequisites:

- `curl`
- `python3`
- `shasum`

Install:

```bash
curl -fsSL https://raw.githubusercontent.com/thtmnisamnstr/termlm/main/scripts/install.sh | bash
```

Notes:

- installer waits for runtime/model/index readiness by default and prints periodic progress
- first install can take several minutes while models are prepared and docs are indexed

Enable in zsh:

```bash
termlm init zsh
```

Open a new zsh session, then verify:

```bash
termlm doctor --strict
termlm status
```

If status shows daemon unreachable, run:

```bash
termlm-core --detach
```

At an empty prompt:

- `?` one-shot prompt mode
- `/p` session mode
- `/q` exit session mode

Approval keys:

- `y` approve
- `n` reject
- `e` edit
- `a` approve-all remaining commands

## Upgrade

```bash
termlm upgrade
```

(`termlm update` is accepted as a hidden alias.)

Upgrade behavior:

- selects platform-compatible `no-models` release bundle
- requires `SHA256SUMS` verification
- installs binaries + zsh plugin
- preserves local model files
- bootstraps embedding/index readiness (without downloading bundled inference GGUF)
- removes temporary download/extraction artifacts before exit

## Common Operations

```bash
termlm status --verbose
termlm doctor --strict
termlm reindex --mode delta
termlm reindex --mode full
termlm reindex --mode compact
termlm reload-config
termlm stop
termlm uninstall --yes
```

## Documentation

Start here:

- [Getting Started](docs/getting-started.md)
- [Documentation Hub](docs/README.md)

High-value docs:

- Operations: [docs/operator-runbook.md](docs/operator-runbook.md)
- Configuration: [docs/configuration.md](docs/configuration.md)
- CLI surface: [docs/cli-reference.md](docs/cli-reference.md)
- Upgrades/releases: [docs/release-upgrades.md](docs/release-upgrades.md)
- CI workflows: [docs/ci-workflows.md](docs/ci-workflows.md)
- Troubleshooting: [docs/troubleshooting.md](docs/troubleshooting.md)
- FAQ: [docs/faq.md](docs/faq.md)

Spec coverage and evidence:

- [docs/spec-conformance-matrix.md](docs/spec-conformance-matrix.md)
- [docs/requirements.md](docs/requirements.md)
- [docs/performance.md](docs/performance.md)
- [docs/local-ci.md](docs/local-ci.md)

Current status:
- Core v1 functionality (zsh adapter + shell-neutral daemon) is implemented.
- Spec conformance matrix is green across FR and NFR for the declared v1 scope; release evidence workflows remain part of the release checklist.

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

Long-run reliability + upgrade rehearsal:

```bash
bash tests/reliability/soak_24h.sh /tmp/termlm-soak-24h
bash tests/release/upgrade_rehearsal.sh
```

## Policies

- [CONTRIBUTING.md](CONTRIBUTING.md)
- [SECURITY.md](SECURITY.md)
- [SUPPORT.md](SUPPORT.md)
