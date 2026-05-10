# Contributing to termlm

Thanks for contributing.

## Prerequisites

- Rust toolchain from `rust-toolchain.toml`
- macOS with zsh for adapter/runtime contract validation
- `cmake` in `PATH` for local provider build path

## Development Loop

1. Format and lint:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

2. Run tests:

```bash
cargo test --workspace --all-targets --locked
bash tests/adapter-contract/zsh_adapter_contract.sh
bash tests/compatibility/macos_profile.sh
bash tests/compatibility/terminal_matrix.sh
bash tests/compatibility/ssh_env_smoke.sh
bash tests/compatibility/plugin_manager_matrix.sh
```

3. Run perf gate harness when relevant:

```bash
cargo run -p termlm-test --release --locked -- --suite tests/fixtures/termlm-test-suite.toml --mode all --provider local --perf-gates tests/perf/perf-gates.toml
```

## Pull Request Expectations

- Keep changes scoped and explain behavior changes clearly.
- Add or update tests for non-trivial behavior changes.
- Update docs for user-visible changes (`README`, `docs/`).
- Avoid force-push rewriting after active review unless requested.

## Commit and Changelog

- Use concise, imperative commit subjects.
- For user-facing changes, include a `CHANGELOG.md` entry in the `Unreleased` section.

## Reporting Bugs

Open an issue with:

- exact command(s)
- config snippet (redacted)
- `termlm status --verbose` output
- daemon log tail (`~/.local/state/termlm/termlm.log`)
- reproduction steps
