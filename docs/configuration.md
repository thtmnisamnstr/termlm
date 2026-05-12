# termlm Configuration Guide

This guide covers the settings that matter most for day-to-day usage and low-latency operation.
For full key coverage, see [`config-reference.md`](config-reference.md).

Baseline diagnostics command:

```bash
termlm doctor --strict
```

## Config File Location

- Default path: `~/.config/termlm/config.toml`
- The file is auto-created from defaults on first daemon/client use.

## High-Impact Defaults

These are the defaults that most directly affect behavior:

- `[inference] provider = "local"`
- `[inference] tool_calling_required = true`
- `[model] variant = "E4B"`
- `[model] auto_download = true`
- `[approval] mode = "manual"`
- `[web] enabled = true`
- `[web] expose_tools = true`
- `[web] provider = "duckduckgo_html"`
- `[indexer] embedding_provider = "local"`
- `[indexer] vector_storage = "f16"`
- `[performance] profile = "performance"`

## Recommended Starting Edits

### Keep local-first and strict approval

```toml
[inference]
provider = "local"
tool_calling_required = true

[approval]
mode = "manual"
```

### Use Ollama for generation (local embeddings still default)

```toml
[inference]
provider = "ollama"

[ollama]
endpoint = "http://127.0.0.1:11434"
model = "gemma4:e4b"
allow_remote = false
allow_plain_http_remote = false
healthcheck_on_start = true
```

### Disable web search/read

Web search and page reading are on by default with the no-token DuckDuckGo HTML provider.
The task router uses them for web/current-information prompts such as "search the web", "look up", "latest", or prompts containing an HTTP(S) URL. Command prompts still prefer local command docs first, but can use web search/read as a fallback when local retrieval is missing or insufficient.

Disable all daemon-owned web requests:

```toml
[web]
enabled = false
```

Keep the web runtime configured but hide `web_search` and `web_read` from the model:

```toml
[web]
expose_tools = false
```

### Reduce context capture footprint

```toml
[terminal_context]
capture_all_interactive_commands = true
max_entries = 30
max_output_bytes_per_command = 16384
```

## Reload Behavior

Apply most config changes without restart:

```bash
termlm reload-config
```

### Restart-required keys

Changes under these keys require daemon restart to fully apply:

- `model.*`
- `inference.provider`
- `ollama.endpoint`
- `performance.profile`
- `indexer.embed_filename`
- `indexer.embed_dim`
- `indexer.vector_storage`
- `indexer.embedding_provider`
- `indexer.lexical_index_impl`
- `indexer.embed_query_prefix`
- `indexer.embed_doc_prefix`
- `web.provider`

Restart sequence:

```bash
termlm stop
```

Then either:

- trigger startup from zsh by entering prompt mode (`?`), or
- start manually with `termlm-core --detach`

## Validation Rules You Will Hit

The config validator enforces:

- `inference.provider`: `local` or `ollama`
- `approval.mode`: `manual`, `manual_critical`, or `auto`
- `performance.profile`: `eco`, `balanced`, or `performance`
- `web.provider`: `duckduckgo_html`, `custom_json`, `brave`, `kagi`, `tavily`, `whoogle`, or `none`
- `indexer.vector_storage`: `f16` or `f32`
- `indexer.embedding_provider`: `local` or `ollama`
- `context_budget.trim_strategy`: `priority_newest_first`
- `local_tools.text_detection.mode`: `content` or `binary_magic`

Additional web constraints:

- `web.provider = "custom_json"` requires `web.search_endpoint`
- `web.provider` in `brave|kagi|tavily` requires `web.search_api_key_env`
- `web.search_endpoint` must be absolute URL with valid scheme policy
- `web.extract.include_images` must remain `false` in v1

## Unknown Keys

Unknown keys are ignored by schema parsing and reported as warnings in daemon logs on config load/reload.
Use exact key names from the default generated config to avoid drift.

## Shell-Side Overrides

The zsh adapter supports environment overrides for selected runtime behavior:

- `TERMLM_DISABLE=1` disables plugin load
- `TERMLM_PROMPT_INDICATOR`, `TERMLM_SESSION_INDICATOR`, `TERMLM_PROMPT_USE_COLOR`
- `TERMLM_CAPTURE_ENABLED`, `TERMLM_CAPTURE_MAX_BYTES`
- `TERMLM_DAEMON_BOOT_TIMEOUT_SECS`
- `TERMLM_CORE_BIN`, `TERMLM_CLIENT_BIN`

These are useful for debugging and temporary experiments; prefer `config.toml` for durable configuration.
