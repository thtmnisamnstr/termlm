# termlm zsh Adapter (v1)

The v1 adapter is implemented in `plugins/zsh/` and is the only supported shell adapter in v1.

## Runtime Components

- `termlm.plugin.zsh`: bootstrap, state init, widget registration, hooks, shell registration
- `widgets/self-insert.zsh`: `?` sigil entry handling
- `widgets/accept-line.zsh`: `/p`, `/q`, prompt submit, implicit abort flow
- `widgets/prompt-mode.zsh`: indicator rendering, prompt/session mode transitions
- `widgets/approval.zsh`: `y/n/e/a` command approval UX
- `widgets/safety-floor.zsh`: duplicate immutable adapter-side safety floor
- `lib/ipc.zsh`: persistent helper process (`termlm bridge`) and event stream handling
- `lib/capture.zsh`: command capture wrapper and truncation logic
- `lib/terminal-observer.zsh`: all-interactive command observation via `preexec`/`precmd`
- `lib/shell-context.zsh`: alias/function/builtin inventory upload

## Key Behavior

- Typing `?` at an empty prompt enters prompt mode.
- `/p` enters session mode; `/q` exits session mode.
- `\?` is treated as a literal character (no mode switch).
- `Ctrl-D` exits session mode when buffer is empty.
- While waiting on model output, typing a plausible shell command and pressing Enter triggers task abort and returns command control to the shell.
- Proposed commands requiring approval use single-key decisions:
  - `y`: approve
  - `n` or Enter: reject
  - `e`: edit in `$EDITOR`
  - `a`: approve all for current task only
- Approved commands execute in real interactive zsh via `BUFFER` + `zle .accept-line`.

## Prompt Indicators

Defaults:

- prompt mode: `?> `
- session mode: `?? `

Config source: `[prompt]` in `~/.config/termlm/config.toml`, with env overrides:

- `TERMLM_PROMPT_INDICATOR`
- `TERMLM_SESSION_INDICATOR`
- `TERMLM_PROMPT_USE_COLOR`

## Daemon and Helper Interaction

- Adapter ensures daemon availability (auto-starts `termlm-core --detach` if needed).
- A persistent helper process (`termlm bridge`) maintains shell registration and stream handling.
- The helper stream is consumed asynchronously through `zle -F`, so prompt editing remains responsive during output.

## Hooks Registered

- `preexec`: begins command observation/capture
- `precmd`: emits ack for termlm-issued command completion and emits observed command context
- `zshexit`: aborts active task, unregisters shell, stops helper, restores shell state

## Load Order Requirements

Source `termlm` before widget-wrapping plugins to avoid double wrapping conflicts:

- `zsh-autosuggestions`
- `zsh-syntax-highlighting`
- other plugins that replace `self-insert` or `accept-line`

Examples:

```zsh
# plain ~/.zshrc
source ~/.local/share/termlm/plugins/zsh/termlm.plugin.zsh
source /path/to/zsh-autosuggestions/zsh-autosuggestions.zsh
source /path/to/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh
```

```zsh
# Oh My Zsh
plugins=(... termlm zsh-autosuggestions zsh-syntax-highlighting)
```

```zsh
# zinit
zi light thtmnisamnstr/termlm
zi light zsh-users/zsh-autosuggestions
zi light zsh-users/zsh-syntax-highlighting
```

```zsh
# antidote
antidote bundle thtmnisamnstr/termlm
antidote bundle zsh-users/zsh-autosuggestions
antidote bundle zsh-users/zsh-syntax-highlighting
```

## Config Keys Used Directly by Adapter

- `[prompt] indicator`, `session_indicator`, `use_color`
- `[capture] enabled`, `max_bytes`
- `[terminal_context] capture_all_interactive_commands`, `max_output_bytes_per_command`, `exclude_tui_commands`, `exclude_command_patterns`

Environment overrides:

- `TERMLM_DISABLE=1`
- `TERMLM_CAPTURE_ENABLED`
- `TERMLM_CAPTURE_MAX_BYTES`
- `TERMLM_DAEMON_BOOT_TIMEOUT_SECS`
- `TERMLM_CORE_BIN`
- `TERMLM_CLIENT_BIN`
