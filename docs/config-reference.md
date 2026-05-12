# termlm Config Reference

This is the complete top-level config schema overview for `~/.config/termlm/config.toml`.

For behavior guidance, see [`configuration.md`](configuration.md).

## Top-level tables

- `[inference]`
- `[performance]`
- `[model]`
- `[ollama]`
- `[web]`
- `[web.extract]`
- `[approval]`
- `[behavior]`
- `[daemon]`
- `[logging]`
- `[indexer]`
- `[capture]`
- `[terminal_context]`
- `[local_tools]`
- `[local_tools.text_detection]`
- `[git_context]`
- `[project_metadata]`
- `[tool_routing]`
- `[context_budget]`
- `[cache]`
- `[source_ledger]`
- `[prompt]`
- `[session]`

## High-value keys by table

### `[inference]`

- `provider` (`local` or `ollama`)
- `tool_calling_required`
- `stream`
- `token_idle_timeout_secs`
- `startup_failure_behavior` (`fail`)

### `[performance]`

- `profile` (`eco|balanced|performance`)
- `warm_core_on_start`
- `keep_embedding_warm`
- `prewarm_common_docs`
- `indexer_priority_mode` (`usage|path_order`)
- `max_background_cpu_pct`

### `[model]`

- `variant` (`E4B`/`E2B`)
- `auto_download`
- `download_only_selected_variant`
- `models_dir`
- `e4b_filename`
- `e2b_filename`
- `context_tokens`
- `gpu_layers`
- `threads`

### `[ollama]`

- `endpoint`
- `model`
- `options` (dynamic map)
- `keep_alive`
- `request_timeout_secs`
- `connect_timeout_secs`
- `allow_remote`
- `allow_plain_http_remote`
- `healthcheck_on_start`

### `[web]`

- `enabled` (default: `true`; set `false` to disable daemon-owned web requests)
- `expose_tools` (default: `true`; set `false` to keep config but hide `web_search`/`web_read`)
- `provider` (default: `duckduckgo_html`, no API key)
- `search_endpoint`
- `search_api_key_env`
- `user_agent`
- `request_timeout_secs`
- `connect_timeout_secs`
- `max_results`
- `max_fetch_bytes`
- `max_pages_per_task`
- `cache_ttl_secs`
- `cache_max_bytes`
- `allowed_schemes`
- `allow_plain_http`
- `allow_local_addresses`
- `obey_robots_txt`
- `citation_required`
- `freshness_required_terms`
- `min_delay_between_requests_ms`
- `search_cache_ttl_secs`

### `[web.extract]`

- `strategy` (`auto|semantic_selector|readability|clean_full_page`)
- `output_format` (`markdown`)
- `include_images` (must remain `false` in v1)
- `include_links`
- `include_tables`
- `max_table_rows`
- `max_table_cols`
- `preserve_code_blocks`
- `strip_tracking_params`
- `max_html_bytes`
- `max_markdown_bytes`
- `min_extracted_chars`
- `dedupe_boilerplate`

### `[approval]`

- `mode` (`manual|manual_critical|auto`)
- `critical_patterns`
- `approve_all_resets_per_task`

### `[behavior]`

- `thinking`
- `allow_clarifications`
- `max_tool_rounds`
- `max_planning_rounds`
- `context_classifier_enabled`
- `clarification_timeout_secs`
- `command_timeout_secs`

### `[daemon]`

- `socket_path`
- `pid_file`
- `log_file`
- `log_level`
- `shutdown_grace_secs`
- `boot_timeout_secs`

### `[logging]`

- `redact_critical`

### `[indexer]`

- `enabled`
- `concurrency`
- `max_loadavg`
- `max_doc_bytes`
- `max_binaries`
- `max_chunks`
- `chunk_max_tokens`
- `cheatsheet_static_count`
- `rag_top_k`
- `rag_min_similarity`
- `rag_max_tokens`
- `lookup_max_bytes`
- `hybrid_retrieval_enabled`
- `lexical_index_enabled`
- `lexical_top_k`
- `exact_command_boost`
- `exact_flag_boost`
- `section_boost_options`
- `command_aware_retrieval`
- `command_aware_top_k`
- `validate_command_flags`
- `embedding_provider` (`local|ollama`)
- `embed_filename`
- `embed_dim`
- `embed_query_prefix`
- `embed_doc_prefix`
- `ollama_embed_model`
- `extra_paths`
- `ignore_paths`
- `fsevents_debounce_ms`
- `disk_write_coalesce_secs`
- `vector_storage` (`f16|f32`)
- `lexical_index_impl` (`embedded`)
- `priority_indexing`
- `priority_recent_commands`
- `priority_prompt_commands`
- `cache_retrieval_results`

### `[capture]`

- `enabled`
- `max_bytes`
- `redact_env`

### `[terminal_context]`

- `enabled`
- `capture_all_interactive_commands`
- `max_entries`
- `max_output_bytes_per_command`
- `recent_context_max_tokens`
- `older_context_max_tokens`
- `redact_secrets`
- `exclude_tui_commands`
- `exclude_command_patterns`

### `[local_tools]`

- `enabled`
- `redact_secrets`
- `default_max_bytes`
- `max_file_bytes`
- `max_search_results`
- `max_search_files`
- `max_workspace_entries`
- `respect_gitignore`
- `workspace_markers`
- `allow_home_as_workspace`
- `allow_system_dirs`
- `sensitive_path_allowlist`
- `include_hidden_by_default`

### `[local_tools.text_detection]`

- `mode` (`content|binary_magic`)
- `sample_bytes`
- `reject_nul_bytes`
- `accepted_encodings`
- `deny_binary_magic`

### `[git_context]`

- `enabled`
- `max_changed_files`
- `max_recent_commits`
- `include_diff_summary`
- `max_diff_bytes`

### `[project_metadata]`

- `enabled`
- `max_files_read`
- `max_bytes_per_file`
- `detect_scripts`
- `detect_package_managers`
- `detect_ci`

### `[tool_routing]`

- `dynamic_exposure_enabled`
- `always_expose_execute`
- `always_expose_lookup_docs`
- `expose_web_only_when_needed`
- `expose_terminal_context_only_when_needed`
- `expose_file_tools_for_local_questions`

### `[context_budget]`

- `enabled`
- `max_total_context_tokens`
- `reserve_response_tokens`
- `current_question_tokens`
- `recent_terminal_tokens`
- `older_session_tokens`
- `local_tool_result_tokens`
- `project_git_metadata_tokens`
- `docs_rag_tokens`
- `web_result_tokens`
- `cheat_sheet_tokens`
- `trim_strategy` (`priority_newest_first`)

### `[cache]`

- `enabled`
- `retrieval_cache_ttl_secs`
- `command_validation_cache_ttl_secs`
- `project_metadata_cache_ttl_secs`
- `git_context_cache_ttl_secs`
- `file_read_cache_ttl_secs`
- `web_cache_ttl_secs`
- `max_total_cache_bytes`

### `[source_ledger]`

- `enabled`
- `expose_on_status`
- `include_in_debug_logs`
- `max_refs_on_status`

### `[prompt]`

- `indicator`
- `session_indicator`
- `use_color`

### `[session]`

- `context_window_tokens`

## Hot-reload vs restart-required

See [`configuration.md`](configuration.md) for current restart-required key paths.
