# termlm FAQ

## Which shell is supported?

`zsh` is the only supported shell adapter in v1.

## Where should I start in the docs?

Use:

- [getting-started.md](getting-started.md) for first install/use
- [README.md](README.md) for role-based navigation

## Which platform is supported?

Official local-provider support is macOS 13+ on Apple Silicon (`darwin-arm64`).

## How do I install quickly?

Installer prerequisites:

- `curl`
- `python3`
- `shasum`

```bash
curl -fsSL https://raw.githubusercontent.com/thtmnisamnstr/termlm/main/scripts/install.sh | bash
```

Then run:

```bash
termlm init zsh
```

Open a new zsh session.

## How do I upgrade?

```bash
termlm upgrade
```

`termlm update` is accepted as a hidden alias.

## Why is `termlm upgrade` lightweight?

Upgrade installs the `no-models` artifact, preserves existing inference models, and bootstraps only
embedding/index assets if missing.

## How do I check if my setup is healthy?

```bash
termlm doctor --strict
termlm status --verbose
```

## Why does prompt mode not start?

Check:

1. plugin sourced in `~/.zshrc`
2. daemon reachable (`termlm status`)
3. plugin load order (`termlm` before autosuggestion/highlighting wrappers)

## Where are logs and state?

- config: `~/.config/termlm/config.toml`
- daemon log: `~/.local/state/termlm/termlm.log`
- models: `~/.local/share/termlm/models`
- index: `~/.local/share/termlm/index`

## How do I uninstall?

```bash
termlm uninstall --yes
```

Then remove the `termlm.plugin.zsh` source line from `~/.zshrc`.
