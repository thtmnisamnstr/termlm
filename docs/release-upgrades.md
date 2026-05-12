# termlm Release and Upgrade Contract

This document defines the release artifact formats and the behavior of `termlm upgrade`.

## Artifact Types Per Release

Each release publishes both artifacts for the supported target:

- `termlm-<version>-<target>-with-models.tar.gz` (default first install path)
- `termlm-<version>-<target>-no-models.tar.gz` (used by `termlm upgrade`)

Supported target in current upgrader/release contract:

- `darwin-arm64`

## Bundle Layout Contract

A valid bundle contains:

- `bin/termlm` (or `bin/termlm-client`)
- `bin/termlm-core`
- `bin/termlm-client` (compat shim path is still installed)
- `plugins/zsh/...`
- `install.sh`
- `bundle-manifest.json`

`bundle-manifest.json` shape:

```json
{
  "schema_version": 1,
  "version": "0.1.0-alpha",
  "target": "darwin-arm64",
  "artifact_kind": "no-models",
  "includes_models": false
}
```

## with-models Packaging Model

The with-models bundle ships model metadata and model chunk references. Model bytes are published
as separate release assets:

- `models/models-manifest.json` in bundle
- release chunk assets: `termlm-<version>-<target>-model-<filename>.part-<nnn>`
- per-asset `*.sha256` and consolidated `SHA256SUMS`

Default model set:

- `gemma-4-E4B-it-Q4_K_M.gguf`
- `bge-small-en-v1.5.Q4_K_M.gguf`

Optional:

- `gemma-4-E2B-it-Q4_K_M.gguf` when `TERMLM_RELEASE_INCLUDE_E2B=1`

Default checksum posture in `scripts/release/package_release.sh`:

- E4B and embedding artifacts are checksum-validated by default
- E2B checksum must be supplied when included (`TERMLM_RELEASE_MODEL_E2B_SHA256`)

## install.sh Behavior

Inside a release bundle:

```bash
./install.sh
./install.sh --skip-models
```

Install paths (overridable):

- binaries: `~/.local/bin` via `TERMLM_INSTALL_BIN_DIR`
- shared dir: `~/.local/share/termlm` via `TERMLM_INSTALL_SHARE_DIR`
- models: `~/.local/share/termlm/models` via `TERMLM_MODELS_DIR`

For chunked models, `install.sh` resolves the release tag from `bundle-manifest.json` or
`TERMLM_RELEASE_TAG` and downloads model chunks from GitHub release assets, verifies each chunk,
assembles the final model file, and verifies the final model checksum.

The installer emits periodic progress for:

- model chunk downloads (bytes + percent when available)
- runtime/index readiness (`index_progress`, chunk counts, provider health)

Install completion semantics:

- `with-models` bundle install completes only after:
  - model chunk download/assembly (unless `--skip-models`)
  - daemon bootstrap is healthy
  - index bootstrap reaches complete/idle at `100%`
- `no-models` bundle install completes only after:
  - embedding model bootstrap (if missing)
  - index bootstrap reaches complete/idle at `100%`
  - verification that the embedding GGUF exists locally
  - temporary embed-only bootstrap daemon is stopped before installer exit

`no-models` install intentionally does **not** fetch the local inference GGUF during install.

Readiness wait controls:

- `TERMLM_INSTALL_WAIT_FOR_READY=0` to skip readiness wait
- `TERMLM_INSTALL_READY_TIMEOUT_SECS` (default `900`)
- `TERMLM_INSTALL_READY_POLL_SECS` (default `2`)

Readiness failure diagnostics:

- installer prints the last observed `termlm status --verbose` payload when available
- installer tails daemon logs on failure
- repeated `status --verbose` timeouts fail fast with diagnostics instead of waiting silently
- installer readiness failures stop the daemon instance started for bootstrap and remove temporary bootstrap config files

For first-time installs, repository bootstrap helper:

```bash
scripts/install.sh
```

Bootstrap installer hardening:

- requires `curl`, `python3`, and `shasum`
- verifies `SHA256SUMS` before extraction
- performs safe archive extraction (rejects absolute/parent paths and link/device entries)

## `termlm upgrade` Behavior

`termlm upgrade` performs:

1. query GitHub latest release API
2. select platform `no-models` asset
3. require and download `SHA256SUMS`
4. verify bundle checksum
5. extract to temporary directory with path safety checks
6. validate payload structure and `bundle-manifest.json`
7. reject any bundle that includes models or non-`no-models` manifest kind
8. send best-effort daemon shutdown
9. atomically install binaries/plugin
10. write install receipt to `~/.local/share/termlm/install-receipt.json`
11. delete temporary artifacts before process exit

Environment controls:

- `TERMLM_GITHUB_REPO` (default `thtmnisamnstr/termlm`)
- `TERMLM_GITHUB_TOKEN`/`GITHUB_TOKEN` for API/auth rate-limit handling
- `TERMLM_GITHUB_API_BASE` (testing override for release API base; default `https://api.github.com/repos`)
- `TERMLM_GITHUB_DOWNLOAD_BASE` (testing override for GitHub release asset downloads; default `https://github.com`)
- `TERMLM_MODEL_DOWNLOAD_RETRIES` (default `3`)
- `TERMLM_MODEL_DOWNLOAD_TIMEOUT_SECS` (default `300`)
- `TERMLM_INSTALL_BIN_DIR`
- `TERMLM_INSTALL_SHARE_DIR`
- `TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS=1` (testing override, not recommended for production)

## Packaging Commands

### Local environment cleanup
If you want to start with a completely fresh environment, uninstall termlm and run these commands:
1. `rm -rf ~/.local/state/termlm`
2. `rm -rf ~/.local/share/termlm`
3. `rm -rf ~/.config/termlm`

### Packaging

```bash
VERSION=v0.1.0-alpha
cargo build -p termlm-client -p termlm-core --release --locked
scripts/release/package_release.sh --mode no-models --version "$VERSION" --target darwin-arm64 --out dist
scripts/release/package_release.sh --mode with-models --version "$VERSION" --target darwin-arm64 --out dist
cat dist/*.sha256 > dist/SHA256SUMS
```

## Manual Release Build and Upload (When GitHub Actions Is Unavailable)

Run from repository root:

```bash
rm -rf dist
mkdir -p dist
VERSION=v0.1.0-alpha

cargo build -p termlm-client -p termlm-core --release --locked

scripts/release/package_release.sh \
  --mode no-models \
  --version "$VERSION" \
  --target darwin-arm64 \
  --out dist

scripts/release/package_release.sh \
  --mode with-models \
  --version "$VERSION" \
  --target darwin-arm64 \
  --out dist

find dist -maxdepth 1 -type f -name '*.sha256' -print | sort | while IFS= read -r f; do
  cat "$f"
done > dist/SHA256SUMS
```

If you apply signing/notarization, regenerate `SHA256SUMS` after signing.

Validate release artifacts locally before publishing:

```bash
bash tests/release/release_smoke.sh
bash tests/release/upgrade_rehearsal.sh
```

Publish by uploading all files in `dist/` to the GitHub Release for the same tag:

- `termlm-<version>-darwin-arm64-no-models.tar.gz` and `.sha256`
- `termlm-<version>-darwin-arm64-with-models.tar.gz` and `.sha256`
- all `termlm-<version>-darwin-arm64-model-*.part-*` chunk assets and each `.sha256`
- `SHA256SUMS`

Do not omit model chunk assets for with-models releases.

Optional codesign/notary hardening for public releases:

```bash
scripts/release/sign_and_notarize.sh --dist dist --identity "Developer ID Application: Your Name (TEAMID)"
```

For local CI/dev smoke validation, use ad-hoc signing:

```bash
scripts/release/sign_and_notarize.sh --dist dist --identity "-"
```

To include notarization evidence, configure a notary profile first and pass it:

```bash
xcrun notarytool store-credentials termlm-notary --apple-id "<apple-id>" --password "<app-password>" --team-id "<team-id>"
scripts/release/sign_and_notarize.sh --dist dist --identity "Developer ID Application: Your Name (TEAMID)" --notary-profile termlm-notary
```

After signing/notarization, regenerate `SHA256SUMS` from `dist/*.sha256` before publishing.

CI release workflow:

- `.github/workflows/release.yml`
- triggers on tag push (`v*`) and manual dispatch (`workflow_dispatch`)
- rejects placeholder tags such as `vX.Y.Z`
- builds/validates/packages release artifacts, uploads `dist/*` as a workflow artifact bundle, and publishes/clobbers the same files on the GitHub Release for the tag
- optional codesign/notary lane is auto-enabled when all relevant secrets are present:
  - `APPLE_SIGNING_CERT_P12_BASE64`
  - `APPLE_SIGNING_CERT_PASSWORD`
  - `APPLE_CODESIGN_IDENTITY`
  - `APPLE_NOTARY_APPLE_ID`
  - `APPLE_NOTARY_APP_PASSWORD`
  - `APPLE_NOTARY_TEAM_ID`
- Optional future hardening: GitHub artifact provenance attestation for `dist/*` release assets.

## Local Upgrade Rehearsal

End-to-end local rehearsal (no external release dependency):

```bash
bash tests/release/upgrade_rehearsal.sh
```

This validates:

- first install from `with-models` bundle
- `termlm upgrade` selection of `no-models` bundle
- checksum verification via `SHA256SUMS`
- install receipt fields (`artifact_kind=no-models`, `includes_models=false`)
- model preservation and temp artifact cleanup
