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
  "version": "0.1.0",
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

For first-time installs, repository bootstrap helper:

```bash
scripts/install.sh
```

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
- `TERMLM_INSTALL_BIN_DIR`
- `TERMLM_INSTALL_SHARE_DIR`
- `TERMLM_UPGRADE_ALLOW_MISSING_CHECKSUMS=1` (testing override, not recommended for production)

## Packaging Commands

```bash
cargo build -p termlm-client -p termlm-core --release --locked
scripts/release/package_release.sh --mode no-models --version vX.Y.Z --target darwin-arm64 --out dist
scripts/release/package_release.sh --mode with-models --version vX.Y.Z --target darwin-arm64 --out dist
cat dist/*.sha256 > dist/SHA256SUMS
```

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
- optional codesign/notary lane is auto-enabled when all relevant secrets are present:
  - `APPLE_SIGNING_CERT_P12_BASE64`
  - `APPLE_SIGNING_CERT_PASSWORD`
  - `APPLE_CODESIGN_IDENTITY`
  - `APPLE_NOTARY_APPLE_ID`
  - `APPLE_NOTARY_APP_PASSWORD`
  - `APPLE_NOTARY_TEAM_ID`
- GitHub artifact provenance attestation is generated for `dist/*` release assets
