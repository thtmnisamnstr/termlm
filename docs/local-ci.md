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

## Response Accuracy Gate

Run the fast prompt/retrieval gate directly:

```bash
bash scripts/ci/run_accuracy_gate.sh --level commit
```

`commit` runs the broad 108-case retrieval suite, the safety/event subset, and the focused usability retrieval suite. It writes JSON and Markdown reports under `/tmp/termlm-accuracy-YYYYmmdd-HHMMSS`, including a tuning packet you can feed into an LLM or use manually.

Run the fuller local pass before a release tuning session:

```bash
bash scripts/ci/run_accuracy_gate.sh --level full
```

`full` adds the broad end-to-end diagnostic lane and runs the real zsh plugin journey when the local GGUF model assets are installed. Diagnostic failures are reported but are not blocking unless you ask for strict mode.

Make release usability require the real zsh/model path on a machine with models installed:

```bash
TERMLM_ACCURACY_REQUIRE_REAL=1 \
bash scripts/ci/run_accuracy_gate.sh --level release
```

That path requires the real zsh journey to run and exercises retrieval traces, prompt answers, command approval/execution, compound commands, and the local web-search/read fixture.

Make diagnostic harness failures blocking when you are intentionally tuning those failures:

```bash
bash scripts/ci/run_accuracy_gate.sh --level full --strict-diagnostics
```

Install the pre-commit hook:

```bash
bash scripts/dev/install_git_hooks.sh
```

The hook runs the `commit` accuracy gate. To temporarily skip it for a local-only WIP commit:

```bash
TERMLM_SKIP_ACCURACY_GATE=1 git commit
```

Optional LLM-assisted tuning notes:

```bash
TERMLM_ACCURACY_TUNER_CMD='llm "Review this termlm accuracy report and suggest prompt/retrieval/tool-orchestration fixes."' \
bash scripts/ci/run_accuracy_gate.sh --level full
```

The tuner command receives the aggregate Markdown report on stdin and writes `llm-tuning-notes.md` beside the gate artifacts. Tuning should change retrieval ranking, document expansion, planner prompts, validation feedback, safe informational tool orchestration, or clarification behavior; do not add production shortcuts for individual test prompts.

Run the focused real-model usability suite directly when you are checking prompt quality before a release:

```bash
TERMLM_E2E_REAL=1 TERMLM_TEST_REINDEX_TIMEOUT_SECS=240 \
cargo run -p termlm-test -- \
  --suite tests/fixtures/termlm-usability-suite.toml \
  --mode e2e \
  --skip-benchmarks
```

Check command-doc retrieval for the same prompts:

```bash
TERMLM_E2E_REAL=1 TERMLM_TEST_REINDEX_TIMEOUT_SECS=240 \
cargo run -p termlm-test -- \
  --suite tests/fixtures/termlm-usability-suite.toml \
  --mode retrieval \
  --skip-benchmarks
```

Run the real zsh plugin journey that mirrors user behavior in an interactive shell:

```bash
bash tests/usability/zsh_user_journey.sh
```

The zsh journey builds `termlm`, creates an isolated home directory with Desktop/Downloads/Documents fixtures, sources the real zsh plugin in a PTY, submits prompts with `?`, approves/rejects proposed commands, verifies retrieval traces, and exercises a local web-search/read fixture at `release` level. It skips automatically when the local GGUF model assets are missing. To make this a hard release gate on a machine with models installed:

```bash
TERMLM_ZSH_USABILITY_REQUIRE_MODEL=1 \
TERMLM_ZSH_USABILITY_LEVEL=release \
bash tests/usability/zsh_user_journey.sh
```

Local CI runs this lane by default when model assets are present. Use `--zsh-usability-level smoke|release|full` to choose coverage, or `--skip-zsh-usability` for loops where you explicitly do not want the real plugin/model run.

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
- zsh usability transcripts/retrieval traces when `TERMLM_ZSH_USABILITY_ARTIFACT_DIR` or `TERMLM_ZSH_USABILITY_KEEP_ARTIFACTS=1` is set
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
- The zsh usability journey also needs the local generative and embedding GGUF files:
  - `~/.local/share/termlm/models/gemma-4-E4B-it-Q4_K_M.gguf`
  - `~/.local/share/termlm/models/bge-small-en-v1.5.Q4_K_M.gguf`
- Real local runtime evidence is perf-gated by `tests/perf/real-runtime-gates.toml` (or `--real-runtime-gates` override).
- Ollama parity requires the `ollama` binary and network/model pull availability.
- Release rehearsal runs by default in full mode and validates install -> upgrade -> cleanup with a local mock release API.
- Reliability drills support additional stability controls for long soak runs:
  - `TERMLM_RELIABILITY_RETRIES` (default `3`)
  - `TERMLM_RELIABILITY_RETRY_DELAY_SECS` (default `0.05`)
  - `TERMLM_SOAK_LOOP_SLEEP_SECS` (default `0.02`)
