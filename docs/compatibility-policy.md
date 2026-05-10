# Compatibility Policy

## Supported in v1

- Shell: `zsh` (`>= 5.8`)
- Official local-provider runtime target: macOS 13+ on Apple Silicon (`darwin-arm64`)

## Not supported in v1

- bash/fish adapters
- non-macOS local-provider support as a release contract

## Stability commitments

- Public user commands (`status`, `reindex`, `reload-config`, `upgrade`, `doctor`, `init`, `uninstall`) are stability-priority interfaces.
- Internal adapter/bridge/protocol commands may change without user-facing stability guarantees.

## Upgrade compatibility

- `termlm upgrade` expects official release bundles with `SHA256SUMS`.
- `no-models` upgrade path preserves local model files.
