# Contributing to termlm

Thanks for contributing.

## Prerequisites

- Rust toolchain from `rust-toolchain.toml`
- macOS with zsh for adapter/runtime contract validation
- `cmake` in `PATH` for local provider build path

Note: To clear termlm fully from your system for testing purposes (or otherwise), run:
```
termlm uninstall --yes
rm -rf ~/.local/state/termlm
rm -rf ~/.local/share/termlm
rm -rf ~/.config/termlm
```

## Development Loop

1. Required pre-commit validation (minimum gate):

```bash
python3 scripts/ci/check_docs_links.py
bash scripts/ci/lint_shell.sh
cargo fmt --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
bash tests/compatibility/macos_profile.sh
```

2. Extended validation before pushing high-risk changes (adapter/runtime/perf/release paths):

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
bash tests/adapter-contract/zsh_adapter_contract.sh
bash tests/compatibility/macos_profile.sh
bash tests/compatibility/terminal_matrix.sh
bash tests/compatibility/ssh_env_smoke.sh
bash tests/compatibility/plugin_manager_matrix.sh
bash tests/release/release_smoke.sh
bash tests/release/upgrade_rehearsal.sh
```

3. Full local workflow-equivalent CI (recommended before release tags or when GitHub Actions is unavailable):

```bash
bash scripts/ci/run_local_ci.sh
```

Fast local loop:

```bash
bash scripts/ci/run_local_ci.sh --quick
```

4. Run perf gate harness when relevant:

```bash
cargo run -p termlm-test --release --locked -- --suite tests/fixtures/termlm-test-suite.toml --mode all --provider local --perf-gates tests/perf/perf-gates.toml
```

## Pull Request Expectations

- Keep changes scoped and explain behavior changes clearly.
- Add or update tests for non-trivial behavior changes.
- Update docs for user-visible changes (`README`, `docs/`).
- Avoid force-push rewriting after active review unless requested.

## Commit Messages

- Use concise, imperative commit subjects.

## Reporting Bugs

Open an issue with:

- exact command(s)
- config snippet (redacted)
- `termlm status --verbose` output
- daemon log tail (`~/.local/state/termlm/termlm.log`)
- reproduction steps
