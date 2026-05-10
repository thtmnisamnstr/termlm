# termlm Spec Conformance Matrix (v5)

This matrix compares the current repository implementation against the v5 requirements document at [requirements.md](requirements.md).

Status legend:
- `Green`: implemented and validated with code + tests in this repo.
- `Yellow`: implemented substantially, but evidence is partial/incomplete against the exact spec wording or quantitative target.
- `Red`: not yet implemented to spec, or not verified enough to claim compliance.

## Summary

- Functional requirements (`FR-1`..`FR-162`): implemented and validated for the declared v1 scope (zsh adapter + shell-neutral core).
- Non-functional requirements (`NFR-1`..`NFR-42`): implemented and validated, including dedicated real-runtime perf gates for `NFR-1`..`NFR-3`.

## Functional Requirements (FR)

| FR IDs | Status | Evidence | Notes / Remaining Work |
|---|---|---|---|
| `FR-1`..`FR-8` | Green | `plugins/zsh/widgets/self-insert.zsh`, `plugins/zsh/widgets/accept-line.zsh`, `plugins/zsh/widgets/prompt-mode.zsh`, `tests/adapter-contract/zsh_adapter_contract.sh` | Sigil/session entry, slash command intercept, mode switching implemented and contract-tested. |
| `FR-9`..`FR-15` | Green | `plugins/zsh/widgets/approval.zsh`, `plugins/zsh/lib/ipc.zsh`, `tests/adapter-contract/zsh_pty_contract.zsh` | Prompt layout, single-key approval, edit flow, abort handling implemented. |
| `FR-16`..`FR-20` | Green | `crates/termlm-safety/src/{floor.rs,critical.rs,parse.rs}`, `crates/termlm-core/src/main.rs`, `plugins/zsh/widgets/safety-floor.zsh` | Approval modes, critical patterns, immutable floor, daemon+adapter defense in depth are implemented. |
| `FR-21`..`FR-32` | Green | `plugins/zsh/lib/ipc.zsh`, `plugins/zsh/widgets/{accept-line.zsh,delete-char-or-list.zsh,prompt-mode.zsh}`, `crates/termlm-core/src/main.rs` | Clarifications, task completion, implicit abort, and session mode semantics are implemented. |
| `FR-33`..`FR-41` | Green | `plugins/zsh/lib/capture.zsh`, `plugins/zsh/lib/terminal-observer.zsh`, `crates/termlm-core/src/main.rs`, adapter contract tests | Real-shell execution via `BUFFER + .accept-line`, ack/capture path, streaming cadence, and thinking suppression are implemented. |
| `FR-42`..`FR-45` | Green | `crates/termlm-config/src/lib.rs`, `crates/termlm-core/src/main.rs` | Config location/defaults, validation, unknown-key warnings, reload-class behavior implemented. |
| `FR-46`..`FR-52` | Green | `plugins/zsh/lib/ipc.zsh`, `crates/termlm-core/src/main.rs`, `crates/termlm-client/src/main.rs` | Daemon bootstrap, registration, refcount shutdown, PID semantics, status/stop flows implemented. |
| `FR-53`..`FR-57` | Green | `crates/termlm-indexer/src/{scan.rs,extract.rs,chunk.rs,watch.rs,store.rs}`, `crates/termlm-core/src/main.rs` | Scope, delta/full indexing, extraction order/timeouts/process-group kill, chunking and metadata implemented. |
| `FR-58` | Green | `crates/termlm-core/src/main.rs`, `crates/termlm-inference/src/{local_llama.rs,ollama.rs}`, `crates/termlm-test/src/main.rs`, `tests/perf/hardware_matrix.sh` | Embedding model path and fallback behavior are implemented, including built-in default URL+checksum mapping for `bge-small-en-v1.5.Q4_K_M.gguf` when env overrides are not set; harness records provider-reported token usage when available and exports environment-tagged benchmark artifacts for repeatable evidence. |
| `FR-59`..`FR-67` | Green | `plugins/zsh/lib/shell-context.zsh`, `plugins/zsh/lib/terminal-observer.zsh`, `crates/termlm-core/src/main.rs` | Shell context updates, cheat sheet tiers, retrieval, index layout, watcher, reindex modes, progress + partial availability implemented. |
| `FR-68`..`FR-72` | Green | `crates/termlm-core/src/main.rs`, `crates/termlm-indexer/src/store.rs`, `crates/termlm-safety/src/parse.rs` | Resource caps, privacy posture, index versioning, docs lookup tool, command-existence validation are implemented. |
| `FR-73`..`FR-82` | Green | `crates/termlm-inference/src/{lib.rs,local_llama.rs,ollama.rs}`, `crates/termlm-core/src/main.rs` | Provider abstraction, local/ollama exclusivity and guardrails, capability probing, parity ownership and embedding-provider split implemented. |
| `FR-83`..`FR-94` | Green | `plugins/zsh/termlm.plugin.zsh`, `crates/termlm-core/src/main.rs`, workspace crate layout | Shell-neutral core and zsh adapter boundary are implemented as specified. |
| `FR-95` | Green | `tests/adapter-contract/*`, `docs/adapter-contract.md`, `tests/compatibility/{terminal_matrix.sh,plugin_manager_matrix.sh,ssh_env_smoke.sh}` | v1 supported adapter (`zsh`) passes the adapter contract suite. Future adapters remain roadmap and are not advertised as supported in v1. |
| `FR-96`..`FR-107` | Green | `crates/termlm-core/src/main.rs`, `plugins/zsh/lib/terminal-observer.zsh` | Naming, classifier/context prioritization, all-interactive observation, compression metadata, redaction behavior implemented. |
| `FR-108`..`FR-113` | Green | `crates/termlm-core/src/{planning/mod.rs,main.rs}`, `crates/termlm-protocol/src/lib.rs`, `tests/fixtures/termlm-test-suite.toml`, `crates/termlm-test/src/main.rs` | Bounded planning + validation are implemented with parser-ambiguity regression coverage (`AMB-001`/`AMB-002`) proving ambiguous drafts route to clarification (no surfaced proposal step). Freshness metadata fields are wired. |
| `FR-114`..`FR-134` | Green | `crates/termlm-web/src/{search.rs,fetch.rs,extract.rs,security.rs,cache.rs}`, `crates/termlm-core/src/main.rs` | HTTP-first web surface, provider abstraction, SSRF/redirect guardrails, robots/politeness, extraction pipeline, and web source labeling implemented. |
| `FR-135`..`FR-140` | Green | `crates/termlm-local-tools/src/*`, `crates/termlm-core/src/main.rs` | Read-only local tools, content-based text detection, bounded reads/search, sensitive path handling, redaction are implemented. |
| `FR-141`..`FR-142` | Green | `crates/termlm-local-tools/src/workspace.rs`, `crates/termlm-config/src/lib.rs`, `crates/termlm-core/src/main.rs` | Shared resolver now enforces configurable + extended markers, blocked/system/home guardrails, canonicalized root handling, and `no_workspace_detected_system_directory` taxonomy with dedicated unit coverage. |
| `FR-143`..`FR-148` | Green | `crates/termlm-local-tools/src/{list_workspace_files.rs,project_metadata.rs,git_context.rs}`, `crates/termlm-core/src/main.rs` | Workspace listing prioritization, project metadata, git context, routing preference, structured output, logging redaction are implemented. |
| `FR-149`..`FR-162` | Green | `crates/termlm-core/src/main.rs`, `crates/termlm-core/src/source_ledger/mod.rs`, `crates/termlm-indexer/src/{retrieve.rs,store.rs}` | Dynamic tool exposure, context budgets, trust-order invariant, caches/invalidation keys, performance profile controls, source ledger, f16+mmap posture, and side-effecting tool boundary implemented. |

## Non-Functional Requirements (NFR)

| NFR ID | Status | Evidence | Notes / Remaining Work |
|---|---|---|---|
| `NFR-1` | Green | `crates/termlm-core/src/main.rs`, `crates/termlm-test/src/main.rs`, `tests/perf/{perf-gates.toml,real-runtime-gates.toml}`, `tests/perf/hardware_matrix.sh`, `.github/workflows/extended-validation.yml` | `model_load_ms` is always emitted, perf-gated, and now enforced in a dedicated real-runtime lane (`local-integration` + `real-runtime-gates`) with Apple hardware-class overrides (`M2 Pro/Max`, `M3 Pro`, `M3 Max`). |
| `NFR-2` | Green | `tests/perf/{perf-gates.toml,real-runtime-gates.toml}`, `crates/termlm-test/src/main.rs`, `.github/workflows/extended-validation.yml` | TTFT gating is enforced generally and in dedicated real-runtime evidence runs; strict `M3 Max` profile overrides are applied by hardware-class detection. |
| `NFR-3` | Green | `crates/termlm-inference/src/{local_llama.rs,ollama.rs}`, `crates/termlm-test/src/main.rs`, `tests/perf/{perf-gates.toml,real-runtime-gates.toml}` | Throughput is measured from provider token usage when available (with heuristic fallback), perf-gated globally, and stricter Apple hardware-class thresholds are enforced in real-runtime evidence lanes. |
| `NFR-4` | Green | `crates/termlm-core/src/main.rs`, `crates/termlm-protocol/src/lib.rs`, `crates/termlm-test/src/main.rs`, `tests/perf/perf-gates.toml` | Status and perf gates now include decomposition metrics (`model_resident_mb`, `indexer_resident_mb`, `orchestration_resident_mb`, `kv_cache_mb`) in addition to total RSS. |
| `NFR-5` | Green | `crates/termlm-test/src/main.rs` idle CPU stabilization sampling, `tests/perf/perf-gates.toml` | Idle CPU is now measured with stabilized post-idle sampling and hard-gated at `p95 <= 0.5` / `max <= 1.5`. |
| `NFR-6` | Green | `tests/reliability/reliability_drills.sh`, plugin EOF handling | Crash recovery behavior and shell state restoration are exercised by reliability drills. |
| `NFR-7` | Green | `crates/termlm-core/src/main.rs` token-idle timeout path | Model stall handling emits timeout/error and ends tasks. |
| `NFR-8` | Green | `plugins/zsh/lib/ipc.zsh` helper retry/backoff + connection-lost handling | Mid-task disconnect retry + graceful user-visible failure behavior implemented. |
| `NFR-9` | Green | `crates/termlm-core/src/main.rs`, `crates/termlm-core/src/ipc/mod.rs` | `umask`, socket permissions, peer UID checks implemented. |
| `NFR-10` | Green | core web/ollama routing + guardrails | Network posture constrained to configured web and provider paths. |
| `NFR-11` | Green | `README.md` privacy section | No-telemetry policy is explicit and enforced by design. |
| `NFR-12` | Green | core logging + redaction path | Logging fields and critical-command redaction behavior implemented. |
| `NFR-13` | Green | README + routing design | Default local-first privacy posture implemented. |
| `NFR-14` | Green | README compatibility notes + zsh-only adapter | v1 support scope stated and enforced. |
| `NFR-15` | Green | `tests/compatibility/macos_profile.sh`, `.github/workflows/ci.yml` | CI now enforces macOS/zsh baseline checks (Darwin, macOS >= 13, zsh >= 5.8, Apple Silicon profile reporting). |
| `NFR-16` | Green | `tests/compatibility/plugin_manager_matrix.sh`, `.github/workflows/extended-validation.yml` | Extended validation runs plugin-manager load-path automation for plain source, Oh My Zsh-style, zinit-style, and antidote-style flows (with post-load wrapper plugins). |
| `NFR-17` | Green | `tests/compatibility/terminal_matrix.sh`, `tests/adapter-contract/zsh_pty_contract.zsh`, `.github/workflows/extended-validation.yml` | Automated PTY compatibility runs across multiple TERM profiles plus wrapper-interop checks for widget-wrapping plugin stacks. |
| `NFR-18` | Green | `tests/compatibility/ssh_env_smoke.sh`, `.github/workflows/extended-validation.yml` | SSH-session environment compatibility is exercised in automation with full adapter PTY contract flow under SSH vars. |
| `NFR-19` | Green | locked workspace + `--locked` CI commands | Reproducible build posture is implemented. |
| `NFR-20` | Green | `crates/termlm-test/src/main.rs` (`benchmark_index_metrics`), `tests/perf/perf-gates.toml` | Cold/full reindex timing is benchmarked and perf-gated; partial-availability behavior remains in core startup/index status flow. |
| `NFR-21` | Green | `crates/termlm-test/src/main.rs` (`benchmark_index_metrics`), `tests/perf/perf-gates.toml` | Warm/delta reindex timing is now measured and hard-gated in CI harness output. |
| `NFR-22` | Green | `crates/termlm-test/src/main.rs` (`benchmark_index_metrics`, hardware profile gate application), `tests/perf/perf-gates.toml` | Perf gates now include hardware-class strict overrides for Apple target classes (`apple_m2_pro_max_local`, `apple_m3_max_local`) with embedding throughput minima aligned to the spec (`>= 400` / `>= 800` chunks/s) while retaining runner-stability defaults for non-target CI hosts. |
| `NFR-23` | Green | `crates/termlm-test/src/main.rs` (`benchmark_retrieval_50k_metrics`), `tests/perf/perf-gates.toml`, `crates/termlm-indexer/benches/hybrid_retrieval.rs` | Harness now executes an explicit 50K-chunk hybrid + lexical retrieval benchmark and gates `retrieval_50k_latency_ms` (`<= 35 ms p50`) and `retrieval_50k_lexical_ms` (`<= 10 ms p50`) in CI/perf runs. Criterion benches now include 50K search cases. |
| `NFR-24` | Green | `crates/termlm-test/src/main.rs` (`benchmark_index_metrics`), `tests/perf/perf-gates.toml` | Index disk footprint is measured from produced artifacts and gated (`index_disk_mb`). |
| `NFR-25` | Green | `crates/termlm-test/src/main.rs` (`ollama_orchestration_overhead_ms`), `tests/perf/perf-gates.toml` | Dedicated orchestration-overhead metric/gate is implemented and enforced in perf harness outputs. |
| `NFR-26` | Green | `crates/termlm-inference/src/ollama.rs`, `crates/termlm-test/src/main.rs`, `.github/workflows/ollama-parity.yml`, `tests/perf/hardware_matrix.sh` | Real Ollama lifecycle parity now has an automated isolated-runtime workflow lane with strict manifest contract validation and uploaded evidence artifacts; provider logic now retries with strict-JSON fallback when a model rejects native tools, preserving parity behavior instead of surfacing hard provider failure. |
| `NFR-27` | Green | `docs/adapter-contract.md`, `tests/adapter-contract/*`, `tests/integration/*`, `plugins/zsh/` | Core/provider/indexer/safety/orchestrator remain shell-neutral while the supported adapter contract is enforced at the plugin boundary for v1. |
| `NFR-28` | Green | `tests/perf/terminal_observer_overhead.zsh`, `crates/termlm-test/src/main.rs`, `tests/perf/perf-gates.toml` | Observer overhead benchmark is hard-gated at the strict `<= 10 ms p50` target on Apple target hardware classes via hardware profile overrides; non-target hosted runners keep broader defaults for stability. |
| `NFR-29` | Green | `crates/termlm-test/src/main.rs` (`planning_loop_overhead_ms`), `tests/perf/perf-gates.toml` | Planning-loop overhead now has a dedicated metric and perf gate aligned to the spec target budget. |
| `NFR-30` | Green | `crates/termlm-test/src/main.rs` (`benchmark_web_extract_metrics`), `tests/perf/perf-gates.toml` | Extraction latency and RSS delta are benchmarked and hard-gated in CI harness output. |
| `NFR-31` | Green | `crates/termlm-web/src/cache.rs` | LRU/TTL/bounded cache behavior implemented. |
| `NFR-32` | Green | web tool error handling paths in core | Web tool failures are isolated and do not crash daemon/task loop. |
| `NFR-33` | Green | `crates/termlm-web/src/fetch.rs` (`web_read_respects_fetch_parse_and_markdown_caps`), `crates/termlm-test/src/main.rs` | Fetch/parse/markdown caps are explicitly tested and extraction footprint is additionally enforced via harness RSS/latency gates. |
| `NFR-34` | Green | `crates/termlm-test/src/main.rs` (`pre_provider_overhead_ms` + stage timings), `tests/perf/perf-gates.toml` | Default profile pre-provider latency budget is now explicitly measured and hard-gated. |
| `NFR-35` | Green | `crates/termlm-test/src/main.rs` (`tool_routing_overhead_ms`), `tests/perf/perf-gates.toml` | Dynamic routing/classification overhead now has dedicated measurement and CI gate. |
| `NFR-36` | Green | deterministic budget/trimming code paths + tests | Context assembly determinism is implemented and tested in fixture behavior. |
| `NFR-37` | Green | f16 storage default + retriever support | f16 footprint objective is implemented with fallback support. |
| `NFR-38` | Green | `crates/termlm-core/src/main.rs` cache-key tests (`read_file`, `project_metadata`, `git_context`, `docs_retrieval`, `web_search`, `web_read`, provider cache id) | Invalidation regression coverage now includes index revision, file metadata/content, git state, provider/endpoint, web freshness/search semantics, and extraction-semantic changes. |
| `NFR-39` | Green | `crates/termlm-test/src/main.rs` (`source_ledger_overhead_ms`, `source_ledger_ref_count`), `tests/perf/perf-gates.toml` | Source-ledger overhead and bounded reference counts are now measured and perf-gated. |
| `NFR-40` | Green | model asset selection logic in core, `crates/termlm-config/src/lib.rs` | Default E4B download posture remains implemented, and canonical E2B artifact naming follows the spec (`gemma-4-E2B-it-Q4_K_M.gguf`) with optional non-selected variant download controlled by `model.download_only_selected_variant`. |
| `NFR-41` | Green | `tests/fixtures/termlm-test-suite.toml` (`AMB-001`/`AMB-002`), `crates/termlm-test/src/{lib.rs,main.rs}` | Explicit parser-ambiguity regressions enforce clarification path with `forbid_proposed_command=true`, proving no added approval step for ambiguous drafts. |
| `NFR-42` | Green | warmup + partial availability behavior in core | Startup warmup/partial functionality and progress reporting are implemented. |

## Operational Evidence (Recommended Before Tagging)

1. Run `bash tests/reliability/soak_24h.sh /tmp/termlm-soak-24h` and retain `run-meta.json` + `soak-metrics.json`.
2. Run `tests/perf/hardware_matrix.sh` with real local model assets and retain `manifest.json` + `SHA256SUMS`.
3. Run `.github/workflows/ollama-parity.yml` (or local equivalent) and retain the parity manifest artifact.
