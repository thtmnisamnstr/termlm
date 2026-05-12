# termlm CLI Reference

This page documents the currently implemented CLI surface.

Primary binaries:

- `termlm` (main user command)
- `termlm-core` (daemon process)
- `termlm-client` (compat shim; mirrors `termlm` behavior)

## `termlm` User Commands

### `termlm status [--verbose]`

Returns daemon/provider/index/task state.

`--verbose` includes:

- source ledger entries for last task
- stage timing breakdown

### `termlm upgrade`

Upgrades from GitHub Releases using the matching `no-models` bundle for the current platform.
See [`release-upgrades.md`](release-upgrades.md) for full behavior and env controls.

### `termlm reload-config`

Signals daemon config reload (`SIGHUP` using configured pid file). Hot-reload keys apply without
daemon restart.

### `termlm stop`

Sends shutdown request to daemon.

### `termlm ping`

Connectivity smoke check to daemon socket.

### `termlm reindex --mode <delta|full|compact>`

Indexer maintenance command.

- `delta`: incremental update; use this for normal PATH/tooling changes
- `full`: full rebuild; reserve for incompatible or corrupt index state
- `compact`: tombstone/stale-fragment compaction

Compat flags are accepted:

- `termlm reindex --full`
- `termlm reindex --compact`

### `termlm init zsh [--print-only] [--force]`

Adds canonical zsh plugin source line to `~/.zshrc`.

- `--print-only`: print the source line without editing
- `--force`: append canonical line even when an existing `termlm.plugin.zsh` source line is detected

### `termlm doctor [--strict] [--json]`

Runs environment and install diagnostics.

- `--strict`: fails non-zero if strict checks fail
- `--json`: emit machine-readable report

### `termlm uninstall [--yes] [--keep-models] [--dry-run]`

Removes installed binaries and plugin files.

- `--yes`: required for actual removal (unless `--dry-run`)
- `--keep-models`: preserve model directory
- `--dry-run`: print planned removals only

## Advanced and Adapter-Internal `termlm` Commands

These commands exist for adapter plumbing, harnesses, and protocol debugging and are hidden from
default help output. Typical users should not run them directly.

Task/protocol helpers:

- `termlm run-task --prompt <text> [--mode ?|/p] [--cwd <path>] [--shell-id <uuid>]`
- `termlm respond-task --task-id <uuid> --decision <approved|rejected|edited|approve-all|abort|clarification> [--edited-command <cmd>] [--text <message>]`
- `termlm ack-task --task-id <uuid> --command <cmd> --cwd-before <path> --cwd-after <path> --exit-status <code> [--command-seq <n>] ...`
- `termlm retrieve --prompt <text> [--top-k <n>] [--json]`

Bridge/adapter internals:

- `termlm bridge`
- `termlm helper --ready-file <path>`
- `termlm register-shell`
- `termlm unregister-shell --shell-id <uuid>`
- `termlm send-shell-context --shell-id <uuid> [--context-hash <hash>] [--alias <name=expansion>]... [--function <name|body_prefix>]... [--builtin <name>]...`
- `termlm observe-command --shell-id <uuid> --raw-command <cmd> --expanded-command <cmd> --cwd-before <path> --cwd-after <path> --exit-status <code> ...`

## `termlm-core` Commands

`termlm-core` runs the daemon directly.

Usage:

```bash
termlm-core [--config <path>] [--sandbox-cwd <path>] [--detach]
```

Options:

- `--config`: alternate config file path (default: `~/.config/termlm/config.toml`)
- `--sandbox-cwd`: force daemon working directory to a resolved/canonicalized path
- `--detach`: background daemonization path used by adapter auto-start

## Exit/Failure Notes

- Most `termlm` commands require daemon socket connectivity.
- `upgrade` and `reload-config` have dedicated flow paths and do not rely on the normal message loop.
- Protocol and validation failures are surfaced as non-zero exits with structured daemon error kinds where available.
