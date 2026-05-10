# termlm

`termlm` is a zsh-first terminal assistant with a shell-neutral Rust daemon.

## What Users Need to Know First

- Trigger prompt mode by typing `?` at an empty prompt.
- Use `/p` for session mode and `/q` to exit session mode.
- Proposed commands are approved with `y` / `n` / `e` / `a`.
- Approved commands execute in your real shell through ZLE (`BUFFER` + `.accept-line`), not through a sidecar shell.
- `termlm upgrade` installs the latest compatible `no-models` release bundle and keeps existing model files.

## Privacy and Network Posture

- No telemetry is collected or sent by this project.
- With `inference.provider = "local"` (default), generation + embeddings + local grounding stay on-device.
- Network access is limited to configured web tools and optional Ollama usage.

## Supported Scope (v1)

- Shell adapter: `zsh` (`>= 5.8`)
- Supported local-provider baseline: macOS 13+ on Apple Silicon
- Bash/fish adapters are not supported in v1

## Install From GitHub Releases

Fast path:

```bash
curl -fsSL https://raw.githubusercontent.com/thtmnisamnstr/termlm/main/scripts/install.sh | bash
```

Manual path:

Release artifacts are split per target:

- `with-models` bundle: default first-time install path
- `no-models` bundle: lightweight binary/plugin update path
- `SHA256SUMS`: checksum manifest consumed by installer and upgrader

Typical first install:

```bash
tar -xzf termlm-vX.Y.Z-darwin-arm64-with-models.tar.gz
cd termlm
./install.sh
```

This installs:

- binaries to `~/.local/bin` (`termlm`, `termlm-core`, `termlm-client`)
- zsh plugin to `~/.local/share/termlm/plugins/zsh`
- model files to `~/.local/share/termlm/models` (unless `--skip-models`)

Make sure `~/.local/bin` is in `PATH`.

## Enable In zsh

```bash
termlm init zsh
```

This appends the canonical plugin source line to `~/.zshrc`.
Load order still matters: source `termlm` before widget-wrapping plugins like
`zsh-autosuggestions` and `zsh-syntax-highlighting`.

## Verify Setup

Open a fresh zsh session after updating `~/.zshrc`, then run:

```bash
termlm status
```

If the daemon is not running yet, start it once with `termlm-core --detach`.

In zsh, type `?` at an empty prompt and submit a prompt.

## Upgrade

```bash
termlm upgrade
```

`termlm upgrade`:

- discovers latest release on GitHub
- downloads the matching `no-models` artifact for the current platform
- requires `SHA256SUMS` and verifies bundle checksums
- installs binaries/plugin and preserves models
- removes all temporary download/extraction artifacts before exit

## Common Operations

```bash
termlm status --verbose
termlm doctor --strict
termlm reindex --mode delta
termlm reindex --mode full
termlm reindex --mode compact
termlm reload-config
termlm stop
termlm uninstall --yes
```

## Configuration

Config path: `~/.config/termlm/config.toml` (auto-created from defaults).

Start here:

- [`docs/configuration.md`](docs/configuration.md)
- [`docs/operator-runbook.md`](docs/operator-runbook.md)

## Documentation Index

- User/ops configuration and commands: [`docs/configuration.md`](docs/configuration.md), [`docs/operator-runbook.md`](docs/operator-runbook.md)
- CLI command reference: [`docs/cli-reference.md`](docs/cli-reference.md)
- Architecture and crate responsibilities: [`docs/architecture.md`](docs/architecture.md)
- FAQ and troubleshooting: [`docs/faq.md`](docs/faq.md), [`docs/troubleshooting.md`](docs/troubleshooting.md)
- Quick prompt recipes: [`docs/quick-recipes.md`](docs/quick-recipes.md)
- Complete config keys: [`docs/config-reference.md`](docs/config-reference.md)
- Release packaging + upgrades: [`docs/release-upgrades.md`](docs/release-upgrades.md)
- Compatibility policy: [`docs/compatibility-policy.md`](docs/compatibility-policy.md)
- Model asset licensing/provenance notes: [`docs/models-and-licenses.md`](docs/models-and-licenses.md)
- zsh adapter behavior and shell integration: [`docs/zsh-adapter.md`](docs/zsh-adapter.md)
- Adapter capability contract (support gate): [`docs/adapter-contract.md`](docs/adapter-contract.md)
- Performance gates and benchmark workflow: [`docs/performance.md`](docs/performance.md)
- Spec coverage tracking matrix: [`docs/spec-conformance-matrix.md`](docs/spec-conformance-matrix.md)

## Project Policies

- Contribution guide: [`CONTRIBUTING.md`](CONTRIBUTING.md)
- Code of conduct: [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)
- Security policy: [`SECURITY.md`](SECURITY.md)
- Support policy: [`SUPPORT.md`](SUPPORT.md)
- Changelog: [`CHANGELOG.md`](CHANGELOG.md)

## Developer Quickstart

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --release --locked
bash tests/adapter-contract/zsh_adapter_contract.sh
bash tests/compatibility/macos_profile.sh
bash tests/compatibility/terminal_matrix.sh
bash tests/compatibility/ssh_env_smoke.sh
bash tests/compatibility/plugin_manager_matrix.sh
cargo run -p termlm-test --release --locked -- --suite tests/fixtures/termlm-test-suite.toml --mode all --provider local --perf-gates tests/perf/perf-gates.toml
```

Optional real-runtime lanes:

```bash
TERMLM_E2E_REAL=1 cargo run -p termlm-test -- --suite tests/fixtures/termlm-test-suite.toml --mode local-integration --provider local
TERMLM_TEST_OLLAMA=1 cargo run -p termlm-test -- --suite tests/fixtures/termlm-test-suite.toml --mode ollama-integration
```

Performance evidence + microbenches:

```bash
bash tests/perf/hardware_matrix.sh
cargo bench -p termlm-indexer --bench hybrid_retrieval
cargo bench -p termlm-web --bench extract_pipeline
```
