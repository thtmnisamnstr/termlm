# termlm Architecture Overview

This document describes the implemented v1 architecture: zsh adapter + shell-neutral core.

## Runtime Components

### 1) zsh adapter (`plugins/zsh/`)

Responsibilities:

- prompt/session UX (`?`, `/p`, `/q`)
- approval UI (`y/n/e/a`)
- command execution in real interactive zsh via ZLE
- command completion ack and terminal-context observation
- helper lifecycle (`termlm bridge`) and shell registration

The adapter does not directly perform inference orchestration.

### 2) client/bridge (`crates/termlm-client`)

Responsibilities:

- user/operator CLI (`status`, `reindex`, `reload-config`, `upgrade`, etc.)
- bridge mode used by zsh plugin (`termlm bridge`)
- protocol framing and daemon IPC transport
- release upgrade/install workflow

### 3) daemon (`crates/termlm-core`)

Responsibilities:

- socket lifecycle, shell registry, task lifecycle
- provider orchestration (local runtime or Ollama)
- planning/validation loop and safety enforcement
- dynamic tool routing and context-budget assembly
- index lifecycle, retrieval, source ledger, caching
- web/local-tools integration

### 4) supporting crates

- `termlm-protocol`: IPC message schema + shared structs
- `termlm-config`: config schema/defaults/validation/reload-class rules
- `termlm-safety`: safety floor + critical pattern matcher + command parser
- `termlm-indexer`: command docs extraction, chunking, vector+lexical index, retrieval
- `termlm-inference`: provider interface + local/Ollama implementations
- `termlm-web`: web search/read, extraction, SSRF/network guardrails, cache
- `termlm-local-tools`: read-only local grounding tools and redaction/text detection
- `termlm-test`: fixture/perf harness

## IPC Contract

Transport:

- Unix domain socket
- length-prefixed frames (`u32` big-endian via `LengthDelimitedCodec`)
- JSON payloads (`tokio-serde`)
- max frame size: `1 MiB` (`MAX_FRAME_BYTES = 1024 * 1024`)

Protocol families:

- client -> daemon: `RegisterShell`, `StartTask`, `UserResponse`, `Ack`, `ObservedCommand`,
  `ShellContext`, `Reindex`, `Retrieve`, `Status`, `ProviderHealth`, `Shutdown`, `Ping`
- daemon -> client: streaming model output/events, proposed commands, task completion, status,
  provider/index progress, protocol and runtime errors

## Task Lifecycle (Happy Path)

1. User enters prompt mode with `?` and submits a natural-language prompt.
2. zsh adapter sends `StartTask` through bridge to daemon.
3. Daemon classifies request, builds bounded context, exposes tools dynamically.
4. Provider streams events (text/tool calls/structured responses).
5. Daemon validates and emits `ProposedCommand` (or clarification/error).
6. Adapter collects decision (`approved`, `rejected`, `edited`, `approve-all`, `abort`).
7. Approved command executes in real shell via `BUFFER` + `zle .accept-line`.
8. Adapter sends `Ack` on completion; daemon finalizes task state.
9. `preexec`/`precmd` observation path sends additional interactive command context.

## Safety and Trust Boundaries

- Safety floor exists in both daemon and zsh adapter.
- Daemon validates command structure and command existence before surfacing proposals.
- Adapter is the only component that executes terminal commands in the user shell.
- Local read-only tools are bounded and redacted.
- Web access is policy-gated and source-labeled separately from local docs.

## State and Data Locations

- config: `~/.config/termlm/config.toml`
- runtime socket/pid: `$XDG_RUNTIME_DIR/termlm.sock` and `$XDG_RUNTIME_DIR/termlm.pid` (resolved with fallback runtime dir)
- daemon log: `~/.local/state/termlm/termlm.log`
- models: `~/.local/share/termlm/models`
- index: `~/.local/share/termlm/index`
- install receipt (upgrade/install): `~/.local/share/termlm/install-receipt.json`

## Performance Posture

- warm startup path for daemon/provider readiness
- bounded context assembly + dynamic tool exposure
- hybrid retrieval (vector + lexical), `f16` vector storage by default
- perf gates enforced by `termlm-test` and `tests/perf/perf-gates.toml`

See [`performance.md`](performance.md) for measured metrics and CI evidence flow.
