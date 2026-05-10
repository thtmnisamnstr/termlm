#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Codesign release bundle binaries and optionally notarize archives.

Usage:
  scripts/release/sign_and_notarize.sh --dist <dir> --identity <developer-id> [--notary-profile <name>]

Options:
  --dist <dir>             Dist directory containing termlm-*.tar.gz bundles
  --identity <name>        Developer ID Application identity for codesign
  --notary-profile <name>  Optional keychain profile name for `xcrun notarytool`; enables notarization
  -h, --help               Show help
USAGE
}

DIST_DIR=""
SIGN_IDENTITY=""
NOTARY_PROFILE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dist)
      DIST_DIR="${2:-}"
      shift 2
      ;;
    --identity)
      SIGN_IDENTITY="${2:-}"
      shift 2
      ;;
    --notary-profile)
      NOTARY_PROFILE="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$DIST_DIR" || -z "$SIGN_IDENTITY" ]]; then
  usage >&2
  exit 2
fi

if [[ ! -d "$DIST_DIR" ]]; then
  echo "dist directory does not exist: $DIST_DIR" >&2
  exit 1
fi

if [[ -n "$NOTARY_PROFILE" ]] && ! command -v xcrun >/dev/null 2>&1; then
  echo "xcrun is required for notarization" >&2
  exit 1
fi

TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termlm-sign.XXXXXX")"
trap 'rm -rf "$TMP_ROOT"' EXIT

sign_executable() {
  local bin_path="$1"
  if [[ "$SIGN_IDENTITY" == "-" ]]; then
    codesign --force --sign - "$bin_path"
  else
    codesign --force --timestamp --options runtime --sign "$SIGN_IDENTITY" "$bin_path"
  fi
  codesign --verify --strict --verbose=2 "$bin_path"
}

write_archive_checksum() {
  local archive_path="$1"
  local asset_name
  asset_name="$(basename "$archive_path")"
  shasum -a 256 "$archive_path" | awk '{print $1 "  '"${asset_name}"'"}' > "${archive_path}.sha256"
}

processed=0
while IFS= read -r archive_path; do
  [[ -n "$archive_path" ]] || continue
  processed=$((processed + 1))

  archive_name="$(basename "$archive_path")"
  stage_dir="$TMP_ROOT/${archive_name%.tar.gz}"
  mkdir -p "$stage_dir"

  tar -xzf "$archive_path" -C "$stage_dir"
  payload_root="$stage_dir/termlm"
  if [[ ! -d "$payload_root" ]]; then
    echo "archive missing payload root termlm/: $archive_name" >&2
    exit 1
  fi

  if [[ ! -d "$payload_root/bin" ]]; then
    echo "archive missing bin/ directory: $archive_name" >&2
    exit 1
  fi

  while IFS= read -r executable; do
    [[ -n "$executable" ]] || continue
    sign_executable "$executable"
  done < <(find "$payload_root/bin" -maxdepth 1 -type f -perm -u=x -print | sort)

  tar -C "$stage_dir" -czf "$archive_path" termlm
  write_archive_checksum "$archive_path"

  if [[ -n "$NOTARY_PROFILE" ]]; then
    zip_path="$TMP_ROOT/${archive_name%.tar.gz}.zip"
    ditto -c -k --sequesterRsrc --keepParent "$payload_root" "$zip_path"
    notary_out="${archive_path}.notary.json"
    xcrun notarytool submit "$zip_path" \
      --keychain-profile "$NOTARY_PROFILE" \
      --wait \
      --output-format json > "$notary_out"
  fi

done < <(find "$DIST_DIR" -maxdepth 1 -type f -name 'termlm-*.tar.gz' -print | sort)

if [[ "$processed" -eq 0 ]]; then
  echo "no release bundles found in $DIST_DIR" >&2
  exit 1
fi

echo "processed $processed release bundles"
