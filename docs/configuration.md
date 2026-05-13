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
The model can use them for web/current-information prompts such as "search the web", "look up", "latest", or prompts containing an HTTP(S) URL. Command prompts still prefer local command docs and read-only local observations first, but can use web search/read as a fallback when local retrieval is missing or insufficient.

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

### Disable opaque read-only shell probes

`termlm` can let the model run tightly allowlisted, non-modifying command probes such as `pwd`, `ls`, `find`, `stat`, `git status`, and `which` while it is grounding an answer. These probes are not shown in the terminal UI and are capped by timeout/output limits. The final command still requires normal approval.

```toml
[local_tools]
readonly_command_enabled = false
```

### Command-doc indexing behavior

On install and reindex, `termlm` builds both vector and lexical search indexes from local command documentation (`man`, `--help`, `-h`, shell builtins, aliases, and functions where available). Retrieved chunks are expanded back to their source command document before they are sent to the model, then deduplicated by command.

During indexing, `termlm` also generates a small `USAGE INTENTS` section for each command document. This is not an exhaustive option permutation list; it is a bounded set of task phrases, option hints, common combinations, and similar-command distinctions. It helps prompts like "files only, not directories" retrieve `find -type f`, while keeping the index small enough for normal installs and upgrades.

Use delta reindexing for normal PATH/tooling changes:

```bash
termlm reindex --mode delta
```

Use a full reindex only for repair, incompatible index-version changes, or when you intentionally want to rebuild the command-doc corpus from scratch:

```bash
termlm reindex --mode full
```

### Inspect retrieval as a builder

The normal UI stays quiet, but there are two manual ways to inspect hybrid retrieval.

Run a one-off retrieval check:

```bash
termlm retrieve --prompt "find large files in this directory" --top-k 8
```

Use JSON when you want exact rank, score, source, path, and text fields:

```bash
termlm retrieve --prompt "find large files in this directory" --top-k 8 --json
```

To capture what real prompt runs retrieved, opt in to trace files:

```toml
[debug]
retrieval_trace_enabled = true
retrieval_trace_dir = "~/.local/state/termlm/retrieval-traces"
retrieval_trace_max_files = 25
```

Then reload config:

```bash
termlm reload-config
```

New prompt runs will write JSON files to:

```text
~/.local/state/termlm/retrieval-traces/
```

List the newest traces:

```bash
ls -lt ~/.local/state/termlm/retrieval-traces | head
```

Open the newest trace with `jq`:

```bash
jq . "$(ls -t ~/.local/state/termlm/retrieval-traces/*.json | head -1)"
```

Each trace includes the prompt, top-K setting, retrieval trace type, and the retrieved command-doc chunks with ranks, scores, command names, section names, source paths, and snippets. Trace files include raw prompt text and retrieved doc snippets, so keep this off unless you are actively debugging retrieval.

### Filesystem context snapshot

`termlm` keeps a small generated context file at:

```text
~/.local/share/termlm/context/filesystem.md
```

It includes the home directory, standard home folders such as Desktop/Documents/Downloads, top-level home folders, and a bounded current-directory listing from the last refresh. The installer creates it, the zsh plugin refreshes it when a shell loads, and `termlm reload-config` refreshes it before signaling the daemon. This gives the model local filesystem grounding without scanning directories on every prompt.

### Reduce context capture footprint

```toml
[terminal_context]
capture_all_interactive_commands = true
max_entries = 30
```

After termlm has been used in a shell, it observes command names, working directories, exit status, and timing by default so later prompts can refer to recent terminal activity. Commands run before the first termlm interaction stay outside termlm's runtime context. Capturing stdout/stderr for every manually typed command is off by default because it is more invasive and can interfere with some terminal setups. To opt in:

```toml
[terminal_context]
capture_command_output = true
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
