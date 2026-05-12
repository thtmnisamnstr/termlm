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
- `full`: rebuild all index artifacts; reserve for incompatible or corrupt index state
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
2. verify binaries are installed and on PATH: `command -v termlm` and `command -v termlm-core`
3. inspect log file: `~/.local/state/termlm/termlm.log`
4. try a manual daemon start: `termlm-core --detach`, then `termlm status --verbose`
5. if the daemon still will not start, run `termlm-core` in the foreground to see the startup error directly
6. confirm whether daemon is alive before removing stale pid/socket files

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

1. for stale results after normal PATH/tooling changes, run `termlm reindex --mode delta`
2. after heavy churn, run `termlm reindex --mode compact` to reclaim stale fragments
3. use `termlm reindex --mode full` only for incompatible or corrupt index state
4. if still broken, stop daemon and remove index root under `~/.local/share/termlm/index/`

### Adapter/helper stream drop

Symptoms:

- `termlm: daemon died`
- prompt/session mode suddenly exits

Actions:

1. re-enter prompt mode (`?`) to trigger helper re-registration
2. check run directory permissions under `${XDG_RUNTIME_DIR:-/tmp}/termlm-$UID/`
3. inspect daemon log tail for protocol/provider failures
4. if prompt mode remains stuck, press `Esc` to reset adapter state, then run `termlm status --verbose`

## 6) Release / CI Gate Commands

Full local workflow-equivalent runner:

```bash
bash scripts/ci/run_local_ci.sh
```

Fast local iteration:

```bash
bash scripts/ci/run_local_ci.sh --quick
```

Fast GitHub push/PR gate equivalent (`.github/workflows/ci.yml`):

```bash
python3 scripts/ci/check_docs_links.py
bash scripts/ci/lint_shell.sh
cargo fmt --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
bash tests/compatibility/macos_profile.sh
```

Extended manual validation (`.github/workflows/extended-validation.yml`):

```bash
cargo test --workspace --all-targets --release --locked
bash tests/adapter-contract/zsh_adapter_contract.sh
bash tests/compatibility/terminal_matrix.sh
bash tests/compatibility/ssh_env_smoke.sh
bash tests/compatibility/plugin_manager_matrix.sh
cargo run -p termlm-test --release --locked -- --suite tests/fixtures/termlm-test-suite.toml --mode all --provider local --perf-gates tests/perf/perf-gates.toml
```

If index reindex waits time out on a large PATH host, raise harness limits:

```bash
TERMLM_TEST_REINDEX_TIMEOUT_SECS=300 \
cargo run -p termlm-test --release --locked -- --suite tests/fixtures/termlm-test-suite.toml --mode all --provider local --perf-gates tests/perf/perf-gates.toml
```

Optional real-runtime lanes:

```bash
TERMLM_E2E_REAL=1 cargo run -p termlm-test -- --suite tests/fixtures/termlm-test-suite.toml --mode local-integration --provider local
TERMLM_TEST_OLLAMA=1 cargo run -p termlm-test -- --suite tests/fixtures/termlm-test-suite.toml --mode ollama-integration
```

Release packaging:

```bash
VERSION=v0.1.0-alpha
cargo build -p termlm-client -p termlm-core --release --locked
scripts/release/package_release.sh --mode no-models --version "$VERSION" --target darwin-arm64 --out dist
scripts/release/package_release.sh --mode with-models --version "$VERSION" --target darwin-arm64 --out dist
cat dist/*.sha256 > dist/SHA256SUMS
```

Reliability drill:

```bash
TERMLM_SOAK_ITERS=40 bash tests/reliability/reliability_drills.sh
```

Duration-based soak target:

```bash
TERMLM_SOAK_DURATION_SECS=86400 bash tests/reliability/reliability_drills.sh
```

24-hour soak evidence (duration-based, concurrent client pressure, metrics artifact):

```bash
bash tests/reliability/soak_24h.sh /tmp/termlm-soak-24h
```

Short soak smoke validation before a full-day run:

```bash
TERMLM_SOAK_DURATION_SECS=120 \
TERMLM_SOAK_PARALLEL_CLIENTS=3 \
bash tests/reliability/soak_24h.sh /tmp/termlm-soak-smoke
```

or with explicit controls:

```bash
TERMLM_SOAK_DURATION_SECS=86400 \
TERMLM_SOAK_PARALLEL_CLIENTS=3 \
TERMLM_SOAK_PATH_CHURN_WINDOW=8 \
TERMLM_RELIABILITY_RETRIES=3 \
TERMLM_RELIABILITY_RETRY_DELAY_SECS=0.05 \
TERMLM_SOAK_LOOP_SLEEP_SECS=0.02 \
TERMLM_SOAK_METRICS_PATH=/tmp/termlm-soak-24h/soak-metrics.json \
bash tests/reliability/reliability_drills.sh
```

Local release-upgrade rehearsal:

```bash
bash tests/release/upgrade_rehearsal.sh
```
