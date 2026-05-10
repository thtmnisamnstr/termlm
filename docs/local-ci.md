# Local CI

When GitHub Actions is unavailable (billing limits, outage, private fork constraints), run workflow-equivalent checks locally with:

```bash
bash scripts/ci/run_local_ci.sh
```

This runner maps to repository workflows:

- `.github/workflows/ci.yml`
- `.github/workflows/extended-validation.yml`
- `.github/workflows/release.yml` (local packaging only; no signing/notary/upload)
- `.github/workflows/ollama-parity.yml`
- `.github/workflows/reliability.yml`
- `.github/workflows/security.yml`

Default GitHub push/PR CI (`ci.yml`) is intentionally fast. Full performance/reliability/release evidence lanes are manual workflows and are also covered by this local runner.

## Common Modes

Quick mode for tight loops:

```bash
bash scripts/ci/run_local_ci.sh --quick
```

`--quick` skips reliability, security, hardware matrix, Ollama parity, and release-packaging/rehearsal lanes.

Custom artifact directory:

```bash
bash scripts/ci/run_local_ci.sh --artifacts-dir /tmp/termlm-ci-run
```

Longer reliability soak:

```bash
bash scripts/ci/run_local_ci.sh --soak-iters 60
```

Duration-based reliability soak target:

```bash
bash scripts/ci/run_local_ci.sh --soak-duration-secs 86400
```

Use a different hardware-matrix suite:

```bash
bash scripts/ci/run_local_ci.sh --perf-suite tests/fixtures/termlm-test-suite.toml
```

Use a different perf gates file for hardware matrix lanes:

```bash
bash scripts/ci/run_local_ci.sh --perf-gates tests/perf/perf-gates.toml
```

Use a different perf gates file for the local real-runtime evidence lane:

```bash
bash scripts/ci/run_local_ci.sh --real-runtime-gates tests/perf/real-runtime-gates.toml
```

Increase hardware-matrix reindex timeout on slower hosts:

```bash
bash scripts/ci/run_local_ci.sh --perf-reindex-timeout-secs 420
```

Skip selected lanes:

```bash
bash scripts/ci/run_local_ci.sh --skip-security --skip-ollama-parity --skip-release-rehearsal
```

Run the 24-hour soak harness directly (writes structured metrics):

```bash
bash tests/reliability/soak_24h.sh /tmp/termlm-soak-artifacts
```

Shorter bounded soak sample (useful before a full 24h run):

```bash
TERMLM_SOAK_DURATION_SECS=120 \
TERMLM_SOAK_PARALLEL_CLIENTS=3 \
bash tests/reliability/soak_24h.sh /tmp/termlm-soak-smoke
```

## What It Produces

The script writes run artifacts to:

- `ARTIFACTS_DIR` env var if set, otherwise
- `/tmp/termlm-local-ci-YYYYmmdd-HHMMSS`

Artifacts include:

- hardware matrix manifests + JSON outputs
- Ollama parity manifest/output
- release packaging artifacts (`with-models`, `no-models`, split model parts, checksums)
- release-upgrade rehearsal evidence (`rehearsal-summary.json`, mock API/assets, install receipt)
- security SBOM files copied out of crate directories

## Notes

- Default hardware-matrix suite is `tests/fixtures/termlm-perf-suite.toml` (perf-focused subset).
- Default hardware-matrix gates file is `tests/perf/perf-gates.toml`.
- To run the full behavioral suite in hardware matrix lanes, pass `--perf-suite tests/fixtures/termlm-test-suite.toml`.
- If full-suite reindex waits are too short for your host PATH size, raise harness timeouts:
- `TERMLM_TEST_REINDEX_TIMEOUT_SECS`
- `TERMLM_TEST_REINDEX_FULL_TIMEOUT_SECS`
- `TERMLM_TEST_REINDEX_DELTA_TIMEOUT_SECS`
- For hardware-matrix lanes specifically, `TERMLM_PERF_MATRIX_REINDEX_TIMEOUT_SECS` defaults to `300` and can be increased for slower hosts.
- Real local runtime E2E is only executed when the local GGUF model file exists:
  - `~/.local/share/termlm/models/gemma-4-E4B-it-Q4_K_M.gguf`
- Real local runtime evidence is perf-gated by `tests/perf/real-runtime-gates.toml` (or `--real-runtime-gates` override).
- Ollama parity requires the `ollama` binary and network/model pull availability.
- Release rehearsal runs by default in full mode and validates install -> upgrade -> cleanup with a local mock release API.
- Reliability drills support additional stability controls for long soak runs:
  - `TERMLM_RELIABILITY_RETRIES` (default `3`)
  - `TERMLM_RELIABILITY_RETRY_DELAY_SECS` (default `0.05`)
  - `TERMLM_SOAK_LOOP_SLEEP_SECS` (default `0.02`)
