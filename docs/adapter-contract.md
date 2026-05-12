# termlm Adapter Contract (v1)

Adapters are shell-specific UI/runtime shims over the shell-neutral daemon.

## Required Responsibilities

1. Register shell session and capabilities.
2. Send shell context updates (aliases/functions/built-ins).
3. Start tasks from NL prompt input.
4. Render streamed text and proposal/clarification events.
5. Handle approval UX (`y/n/e/a`) and abort behavior.
6. Execute approved commands in the real interactive shell.
7. Send completion acknowledgements with exit status and optional captures.
8. Observe interactive commands after the shell has started using termlm and forward redacted context.
9. Preserve native shell history semantics.

## Capability Flags

- `prompt_mode`
- `session_mode`
- `single_key_approval`
- `edit_approval`
- `execute_in_real_shell`
- `command_completion_ack`
- `stdout_stderr_capture`
- `all_interactive_command_observation`
- `terminal_context_capture`
- `alias_capture`
- `function_capture`
- `builtin_inventory`
- `shell_native_history`

## Support Gate

An adapter is only considered supported after passing adapter-contract tests in
`tests/adapter-contract/`, including static contract checks, runtime widget behavior checks, and
PTY-driven interaction checks.
