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

1. `termlm-core --detach`
2. `termlm status`
3. verify runtime dir/socket permissions

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
termlm reindex --mode full
termlm reindex --mode compact
```

## Ollama endpoint issues

Actions:

1. verify `[inference].provider = "ollama"`
2. verify `[ollama].endpoint`
3. verify remote/http policy flags
4. run `termlm status --verbose` and inspect provider health fields
