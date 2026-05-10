# termlm Operator Runbook

This runbook is focused on real operational commands and recovery paths.

## 1) Fast Health Check

```bash
termlm status
termlm status --verbose
termlm doctor --strict
```

Check these first:

- `provider_healthy`
- `active_shells` and `active_tasks`
- `index_progress` phase/percent
- `web: enabled=... provider=...`
- `stage_timings_ms` (verbose mode)

## 2) Reindex Operations

```bash
termlm reindex --mode delta
termlm reindex --mode full
termlm reindex --mode compact
```

Meaning:

- `delta`: update only changed/new/removed command docs
- `full`: rebuild all index artifacts
- `compact`: rewrite index files to remove tombstones and stale fragments

Compat flags are still accepted:

```bash
termlm reindex --full
termlm reindex --compact
```

## 3) Config Apply and Restart

```bash
termlm reload-config
termlm stop
```

- `reload-config` sends daemon reload signal (`SIGHUP`) and applies hot-reload keys.
- restart-required keys need a daemon restart (`termlm stop`, then next shell interaction or manual daemon start).

## 4) Upgrade

```bash
termlm upgrade
```

Behavior:

- queries latest GitHub release
- selects platform-compatible `no-models` artifact
- installs binaries + zsh plugin
- preserves local models
- deletes temporary upgrade artifacts

## 5) Failure Triage

### Daemon unreachable

Symptoms:

- `cannot connect`/`cannot reach daemon`
- prompt mode cannot start tasks

Actions:

1. run `termlm status`
2. inspect log file: `~/.local/state/termlm/termlm.log`
3. confirm whether daemon is alive before removing stale pid/socket files

### Provider unavailable

Symptoms:

- startup failure
- `InferenceProviderUnavailable`

Actions:

1. validate `[inference] provider`
2. if provider is `ollama`, verify endpoint and remote/http policy flags
3. run `termlm status --verbose` and inspect provider health/latency/context fields

### Index corruption or stale retrieval

Symptoms:

- missing expected docs in retrieval
- index load warnings/errors

Actions:

1. `termlm reindex --mode full`
2. if still broken, stop daemon and remove index root under `~/.local/share/termlm/index/`
3. restart and run `termlm reindex --mode compact` after heavy churn periods

### Adapter/helper stream drop

Symptoms:

- `termlm: daemon died`
- prompt/session mode suddenly exits

Actions:

1. re-enter prompt mode (`?`) to trigger helper re-registration
2. check run directory permissions under `${XDG_RUNTIME_DIR:-/tmp}/termlm-$UID/`
3. inspect daemon log tail for protocol/provider failures

## 6) Release / CI Gate Commands

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --release --locked
bash tests/adapter-contract/zsh_adapter_contract.sh
bash tests/compatibility/macos_profile.sh
bash tests/compatibility/terminal_matrix.sh
bash tests/compatibility/ssh_env_smoke.sh
bash tests/compatibility/plugin_manager_matrix.sh
cargo run -p termlm-test --release --locked -- --suite tests/fixtures/termlm-test-suite.toml --mode all --provider local --perf-gates tests/perf/perf-gates.toml
```

Optional real-runtime lanes:

```bash
TERMLM_E2E_REAL=1 cargo run -p termlm-test -- --suite tests/fixtures/termlm-test-suite.toml --mode local-integration --provider local
TERMLM_TEST_OLLAMA=1 cargo run -p termlm-test -- --suite tests/fixtures/termlm-test-suite.toml --mode ollama-integration
```

Release packaging:

```bash
cargo build -p termlm-client -p termlm-core --release --locked
scripts/release/package_release.sh --mode no-models --version vX.Y.Z --target darwin-arm64 --out dist
scripts/release/package_release.sh --mode with-models --version vX.Y.Z --target darwin-arm64 --out dist
cat dist/*.sha256 > dist/SHA256SUMS
```

Reliability drill:

```bash
TERMLM_SOAK_ITERS=40 bash tests/reliability/reliability_drills.sh
```
