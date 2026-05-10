# Adapter Contract Tests

This directory contains the support-gate checks for shell adapters.

- `zsh_adapter_contract.sh` verifies the v1 zsh adapter contract:
  - required widget/hook registration
  - prompt/session mode trigger paths
  - approval UX controls and defaults
  - execution via `BUFFER` + `zle .accept-line`
  - capture/ack + interactive command observation hooks
  - PTY-driven behavioral checks for prompt/session flows and unregister semantics
  - duplicated immutable adapter safety floor

The PTY behavior check uses `expect`; ensure it is installed on the test host.

Compatibility extension scripts live under `tests/compatibility/`:
- `terminal_matrix.sh` runs the PTY contract over multiple `TERM` variants and checks wrapper interop.
- `ssh_env_smoke.sh` runs the same contract with SSH session environment variables set.
- `macos_profile.sh` validates macOS/zsh baseline compatibility constraints.
- `plugin_manager_matrix.sh` exercises plain-source, Oh My Zsh-style, zinit-style, and antidote-style plugin loading paths under PTY automation.

Run manually from repo root:

```bash
bash tests/adapter-contract/zsh_adapter_contract.sh
```
