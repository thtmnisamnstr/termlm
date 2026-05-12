# termlm Troubleshooting

## Fast triage checklist

```bash
termlm doctor --strict
termlm status --verbose
```

If status fails, check daemon log:

```bash
tail -n 80 ~/.local/state/termlm/termlm.log
```

## Daemon unreachable

Symptoms:

- `cannot connect to ...termlm.sock`
- prompt mode cannot start

Actions:

1. Verify the installed binaries are on PATH:

   ```bash
   command -v termlm
   command -v termlm-core
   ```

2. Try a manual daemon start:

   ```bash
   termlm-core --detach
   termlm status --verbose
   ```

3. If status is still unreachable, inspect the daemon log:

   ```bash
   tail -n 120 ~/.local/state/termlm/termlm.log
   ```

4. If no useful log was written, run the daemon in the foreground to see the startup error directly:

   ```bash
   termlm-core
   ```

5. Check the common startup blockers:

   - config parse/validation errors in `~/.config/termlm/config.toml`
   - missing local model files under `~/.local/share/termlm/models`
   - unsupported local-provider platform
   - invalid `XDG_RUNTIME_DIR` or socket/pid-file permissions
   - an old daemon process still owning the configured socket

6. After fixing the blocker, restart cleanly:

   ```bash
   termlm stop || true
   termlm-core --detach
   termlm status --verbose
   ```

## Installer waits too long at readiness

Symptoms:

- install prints `Waiting for termlm runtime/model/index readiness ...` for a long time
- install exits with readiness timeout

Actions:

1. run `pgrep -af termlm-core || true`
2. run `termlm status --verbose`
3. check daemon log tail: `tail -n 120 ~/.local/state/termlm/termlm.log`
4. if needed, perform a clean reinstall reset:
   - `rm -rf ~/.local/state/termlm`
   - `rm -rf ~/.local/share/termlm`
   - `rm -rf ~/.config/termlm`

## Plugin not activating in zsh

Actions:

1. verify source line exists in `~/.zshrc`
2. run `termlm init zsh --print-only` and compare
3. ensure load order: source `termlm` before widget wrappers

## Local model missing

Symptoms:

- local provider startup fails

Actions:

1. check configured variant and filenames
2. check `~/.local/share/termlm/models`
3. install from `with-models` release or configure Ollama provider

## Upgrade fails due checksum/security checks

Symptoms:

- `release is missing SHA256SUMS; refusing upgrade`

Actions:

1. use official release artifacts containing `SHA256SUMS`
2. verify `TERMLM_GITHUB_REPO` points to the expected repo
3. avoid bypassing checksum verification except controlled testing

## Index issues

Symptoms:

- stale/missing command docs

Actions:

```bash
termlm reindex --mode delta
termlm reindex --mode compact
```

Use `termlm reindex --mode full` only when the index is incompatible or corrupt and delta/compact do not repair it.

## Ollama endpoint issues

Actions:

1. verify `[inference].provider = "ollama"`
2. verify `[ollama].endpoint`
3. verify remote/http policy flags
4. run `termlm status --verbose` and inspect provider health fields
