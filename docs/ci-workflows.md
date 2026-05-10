# CI Workflows

`termlm` uses a two-tier CI model to keep GitHub Actions fast while preserving full validation evidence.

## 1) Fast Gate (`.github/workflows/ci.yml`)

Trigger:

- `push`
- `pull_request`
- manual (`workflow_dispatch`)

Purpose:

- quick merge gate with deterministic runtime
- catch formatting, lint, compile, and core test regressions quickly

Lanes:

- `rust-linux-quick`
  - docs link check
  - shell lint
  - `cargo fmt --check`
  - `cargo clippy --workspace --locked -- -D warnings`
  - `cargo test --workspace --locked`
- `rust-macos-quick`
  - `cargo test --workspace --locked --no-run`
  - `bash tests/compatibility/macos_profile.sh`

Implementation notes:

- macOS lanes are pinned to `macos-14-arm64`.
- Rust dependency/build caching is enabled via `Swatinem/rust-cache`.

## 2) Extended Validation (`.github/workflows/extended-validation.yml`)

Trigger:

- manual only (`workflow_dispatch`)

Purpose:

- full quality/perf evidence lane
- release and adapter compatibility confidence before cutover/tagging

Lanes:

- `rust-macos-extended`
  - docs + shell lint + fmt + clippy
  - release-profile tests
  - adapter contract + terminal/plugin/SSH compatibility matrix
  - release smoke
  - perf hardware matrix (`local_stub_all`) with artifact upload (`tests/fixtures/termlm-perf-suite.toml`, `tests/perf/perf-gates.toml`)
  - perf gate file includes 50K retrieval metrics and hardware-class strict override profiles (`apple_m2_pro_max_local`, `apple_m3_pro_local`, `apple_m3_max_local`)
  - optional local real-runtime E2E when model file exists on runner, validated with `tests/perf/real-runtime-gates.toml`
- `rust-linux-maintenance`
  - `cargo machete` dependency hygiene check

## 3) Additional Manual Workflows

- `.github/workflows/release.yml`
  - runs on `v*` tag push and manual dispatch
  - performs release-grade validation + packaging and uploads `dist/*` as workflow artifacts
  - does not auto-publish GitHub releases/tags
- `.github/workflows/ollama-parity.yml`
  - manual Ollama parity evidence lane (`ollama_integration`)
- `.github/workflows/reliability.yml`
  - manual reliability drill lane
- `.github/workflows/security.yml`
  - manual security audit + SBOM generation

## 4) Local Equivalent

When Actions minutes are constrained, run:

```bash
bash scripts/ci/run_local_ci.sh
```

Quick local loop:

```bash
bash scripts/ci/run_local_ci.sh --quick
```

`--quick` skips reliability, security, hardware matrix, Ollama parity, and release-packaging/rehearsal lanes.
