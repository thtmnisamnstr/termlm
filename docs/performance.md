# termlm Performance Validation

`termlm` performance is enforced with:

1. end-to-end perf gates in `termlm-test`
2. hardware-matrix evidence bundles
3. microbenchmarks for retrieval/extraction hot paths

## 1) End-to-End Perf Gates

```bash
cargo run -p termlm-test -- --suite tests/fixtures/termlm-test-suite.toml --mode all --perf-gates tests/perf/perf-gates.toml
```

`tests/perf/perf-gates.toml` is the CI contract. Any threshold violation fails the run.

## 2) Hardware Matrix Evidence

```bash
bash tests/perf/hardware_matrix.sh
```

Outputs:

- per-case JSON results (`local_stub_all`, `local_real_e2e`, `ollama_integration` when enabled)
- `manifest.json` with pass/fail/skip and reasons
- `SHA256SUMS` for artifact integrity

`jq` is required for manifest/schema validation in this script.

Useful controls:

```bash
TERMLM_PERF_MATRIX_CASES=ollama_integration \
TERMLM_PERF_MATRIX_REQUIRE_OLLAMA=1 \
bash tests/perf/hardware_matrix.sh

TERMLM_PERF_MATRIX_LOCAL_MODEL_PATH="$HOME/.local/share/termlm/models/gemma-4-E4B-it-Q4_K_M.gguf" \
TERMLM_PERF_MATRIX_REQUIRE_REAL_LOCAL=1 \
bash tests/perf/hardware_matrix.sh
```

## 3) Key Metrics Enforced

Core latency/throughput:

- `ttft_ms`
- `task_latency_ms`
- `retrieval_latency_ms`
- `throughput_toks_per_sec`
- `planning_loop_overhead_ms`
- `tool_routing_overhead_ms`
- `pre_provider_overhead_ms`

Index/retrieval/web:

- `full_reindex_ms`
- `delta_reindex_ms`
- `embedding_chunks_per_sec`
- `index_disk_mb`
- `web_extract_latency_ms`
- `web_extract_latency_p95_ms`
- `web_extract_rss_delta_mb`

Shell/runtime overhead:

- `observed_command_overhead_ms`
- `observed_command_capture_overhead_ms`
- `idle_cpu_pct`
- `model_load_ms`

Memory:

- `rss_mb`
- `model_resident_mb`
- `indexer_resident_mb`
- `orchestration_resident_mb`
- `kv_cache_mb`

Observability/source-accounting:

- `source_ledger_ref_count`
- `source_ledger_overhead_ms`
- `stage_timings_ms.*`

## 4) Microbenchmarks

```bash
cargo bench -p termlm-indexer --bench hybrid_retrieval
cargo bench -p termlm-web --bench extract_pipeline
```

These isolate retrieval scoring and extraction regressions independent of provider/network variability.

## 5) CI Coverage

- `macos-14` lane runs full quality gates, adapter contracts, compatibility tests, and performance matrix (`local_stub_all`)
- optional real-runtime local lane runs when model artifact is present
- separate scheduled/manual Ollama parity workflow: `.github/workflows/ollama-parity.yml`
- `ubuntu-24.04` smoke lane enforces formatting, clippy, and workspace tests
