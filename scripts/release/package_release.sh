#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Build a termlm release bundle.

Usage:
  scripts/release/package_release.sh --mode <no-models|with-models> --version <tag-or-version> --target <target-id> --out <dir>

Environment overrides:
  TERMLM_RELEASE_MODEL_DIR              Source directory for pre-downloaded model files
  TERMLM_RELEASE_MODEL_E4B_URL          Override E4B download URL
  TERMLM_RELEASE_MODEL_E4B_SHA256       Override E4B checksum validation (defaults to pinned value)
  TERMLM_RELEASE_EMBED_URL              Override embedding-model download URL
  TERMLM_RELEASE_EMBED_SHA256           Override embedding-model checksum validation
  TERMLM_RELEASE_INCLUDE_E2B=1          Include E2B variant in with-models bundle
  TERMLM_RELEASE_MODEL_E2B_URL          Optional E2B download URL
  TERMLM_RELEASE_MODEL_E2B_SHA256       Optional E2B checksum validation
  TERMLM_RELEASE_MODEL_CHUNK_SIZE       Chunk size for model assets (default: 1900m)
USAGE
}

MODE=""
VERSION=""
TARGET=""
OUT_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="${2:-}"
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --target)
      TARGET="${2:-}"
      shift 2
      ;;
    --out)
      OUT_DIR="${2:-}"
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

if [[ -z "$MODE" || -z "$VERSION" || -z "$TARGET" || -z "$OUT_DIR" ]]; then
  usage >&2
  exit 2
fi
if [[ "$MODE" != "no-models" && "$MODE" != "with-models" ]]; then
  echo "--mode must be no-models or with-models" >&2
  exit 2
fi
if [[ "$VERSION" =~ [Xx]\.[Yy]\.[Zz] || "$VERSION" == *"<"* || "$VERSION" == *">"* ]]; then
  echo "--version must be the concrete release tag/version, not a placeholder: ${VERSION}" >&2
  exit 2
fi
if [[ "$VERSION" == *[[:space:]/\\]* ]]; then
  echo "--version must not contain whitespace or path separators: ${VERSION}" >&2
  exit 2
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BUILD_DIR="${ROOT_DIR}/target/release"

if [[ ! -x "$BUILD_DIR/termlm-core" ]]; then
  echo "missing built binary: $BUILD_DIR/termlm-core" >&2
  echo "build release binaries first: cargo build -p termlm-client -p termlm-core --release --locked" >&2
  exit 1
fi
if [[ ! -x "$BUILD_DIR/termlm" && ! -x "$BUILD_DIR/termlm-client" ]]; then
  echo "missing built binary: $BUILD_DIR/termlm or $BUILD_DIR/termlm-client" >&2
  exit 1
fi

E4B_FILE="gemma-4-E4B-it-Q4_K_M.gguf"
E2B_FILE="gemma-4-E2B-it-Q4_K_M.gguf"
EMBED_FILE="bge-small-en-v1.5.Q4_K_M.gguf"

E4B_URL="${TERMLM_RELEASE_MODEL_E4B_URL:-https://huggingface.co/ggml-org/gemma-4-E4B-it-GGUF/resolve/main/${E4B_FILE}}"
E2B_URL="${TERMLM_RELEASE_MODEL_E2B_URL:-https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/${E2B_FILE}}"
EMBED_URL="${TERMLM_RELEASE_EMBED_URL:-https://huggingface.co/ChristianAzinn/bge-small-en-v1.5-gguf/resolve/main/${EMBED_FILE}}"
E4B_SHA256_DEFAULT="90ce98129eb3e8cc57e62433d500c97c624b1e3af1fcc85dd3b55ad7e0313e9f"
EMBED_SHA256_DEFAULT="d8c2e0e38bce043562bbc6f437c638c2538bfe02cadfe6476a01f906bfde6d40"

MODEL_SOURCE_DIR="${TERMLM_RELEASE_MODEL_DIR:-$HOME/.local/share/termlm/models}"
INCLUDE_E2B="${TERMLM_RELEASE_INCLUDE_E2B:-0}"
VERSION_STRIPPED="${VERSION#v}"

mkdir -p "$OUT_DIR"
STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGE_DIR"' EXIT
BUNDLE_ROOT="${STAGE_DIR}/termlm"
mkdir -p "$BUNDLE_ROOT/bin" "$BUNDLE_ROOT/plugins"

if [[ -x "$BUILD_DIR/termlm" ]]; then
  cp "$BUILD_DIR/termlm" "$BUNDLE_ROOT/bin/termlm"
else
  cp "$BUILD_DIR/termlm-client" "$BUNDLE_ROOT/bin/termlm"
fi
cp "$BUILD_DIR/termlm-core" "$BUNDLE_ROOT/bin/termlm-core"
if [[ -x "$BUILD_DIR/termlm-client" ]]; then
  cp "$BUILD_DIR/termlm-client" "$BUNDLE_ROOT/bin/termlm-client"
else
  cp "$BUNDLE_ROOT/bin/termlm" "$BUNDLE_ROOT/bin/termlm-client"
fi
chmod 0755 "$BUNDLE_ROOT/bin/termlm" "$BUNDLE_ROOT/bin/termlm-core" "$BUNDLE_ROOT/bin/termlm-client"

cp -R "$ROOT_DIR/plugins/zsh" "$BUNDLE_ROOT/plugins/zsh"
find "$BUNDLE_ROOT/plugins/zsh" -name .DS_Store -type f -delete
cp "$ROOT_DIR/scripts/release/install_bundle.sh" "$BUNDLE_ROOT/install.sh"
chmod 0755 "$BUNDLE_ROOT/install.sh"
cp "$ROOT_DIR/README.md" "$BUNDLE_ROOT/README.md"
cp "$ROOT_DIR/LICENSE" "$BUNDLE_ROOT/LICENSE"

ensure_model_file() {
  local filename="$1"
  local download_url="$2"
  local expected_sha="${3:-}"
  local dest_dir="$4"
  local source_candidate="${MODEL_SOURCE_DIR}/${filename}"
  local target_path="${dest_dir}/${filename}"

  mkdir -p "$dest_dir"
  if [[ -f "$source_candidate" ]]; then
    cp "$source_candidate" "$target_path"
  else
    echo "downloading model artifact ${filename} from ${download_url}"
    curl --fail --location --retry 3 --retry-delay 2 \
      --output "$target_path" \
      "$download_url"
  fi

  if [[ -n "$expected_sha" ]]; then
    local actual
    actual="$(shasum -a 256 "$target_path" | awk '{print $1}')"
    if [[ "$actual" != "$expected_sha" ]]; then
      echo "checksum mismatch for ${filename}: expected ${expected_sha} got ${actual}" >&2
      exit 1
    fi
  fi
}

sha256_file() {
  local path="$1"
  shasum -a 256 "$path" | awk '{print $1}'
}

append_checksum_file() {
  local path="$1"
  local name="$2"
  local checksum_path="${path}.sha256"
  shasum -a 256 "$path" | awk '{print $1 "  '"${name}"'"}' > "$checksum_path"
}

INCLUDES_MODELS=false
if [[ "$MODE" == "with-models" ]]; then
  INCLUDES_MODELS=true
  MODELS_META_DIR="$BUNDLE_ROOT/models"
  MODELS_CACHE_DIR="$STAGE_DIR/model-cache"
  mkdir -p "$MODELS_META_DIR" "$MODELS_CACHE_DIR"

  MODEL_CHUNK_SIZE="${TERMLM_RELEASE_MODEL_CHUNK_SIZE:-1900m}"

  model_files=("$E4B_FILE" "$EMBED_FILE")
  model_urls=("$E4B_URL" "$EMBED_URL")
  model_shas=("${TERMLM_RELEASE_MODEL_E4B_SHA256:-$E4B_SHA256_DEFAULT}" "${TERMLM_RELEASE_EMBED_SHA256:-$EMBED_SHA256_DEFAULT}")
  if [[ "$INCLUDE_E2B" == "1" ]]; then
    model_files+=("$E2B_FILE")
    model_urls+=("$E2B_URL")
    model_shas+=("${TERMLM_RELEASE_MODEL_E2B_SHA256:-}")
  fi

  MODELS_MANIFEST_PATH="$MODELS_META_DIR/models-manifest.json"
  {
    echo "{"
    echo "  \"schema_version\": 1,"
    echo "  \"version\": \"${VERSION_STRIPPED}\","
    echo "  \"target\": \"${TARGET}\","
    echo "  \"models\": ["
  } > "$MODELS_MANIFEST_PATH"

  model_idx=0
  model_count="${#model_files[@]}"
  while [[ "$model_idx" -lt "$model_count" ]]; do
    model_file="${model_files[$model_idx]}"
    model_url="${model_urls[$model_idx]}"
    model_sha_expected="${model_shas[$model_idx]}"

    ensure_model_file "$model_file" "$model_url" "$model_sha_expected" "$MODELS_CACHE_DIR"
    model_path="$MODELS_CACHE_DIR/$model_file"
    model_sha="$(sha256_file "$model_path")"
    model_size="$(wc -c < "$model_path" | tr -d '[:space:]')"

    safe_model_name="${model_file//[^A-Za-z0-9._-]/_}"
    chunk_prefix="termlm-${VERSION}-${TARGET}-model-${safe_model_name}.part-"
    rm -f "$OUT_DIR/${chunk_prefix}"*
    split -b "$MODEL_CHUNK_SIZE" -d -a 3 "$model_path" "$OUT_DIR/${chunk_prefix}"

    chunk_paths=()
    while IFS= read -r chunk_path; do
      chunk_paths+=("$chunk_path")
    done < <(find "$OUT_DIR" -maxdepth 1 -type f -name "${chunk_prefix}*" -print | sort)
    if [[ "${#chunk_paths[@]}" -eq 0 ]]; then
      echo "failed to create chunks for $model_file" >&2
      exit 1
    fi

    {
      echo "    {"
      echo "      \"filename\": \"${model_file}\","
      echo "      \"sha256\": \"${model_sha}\","
      echo "      \"size_bytes\": ${model_size},"
      echo "      \"chunks\": ["
    } >> "$MODELS_MANIFEST_PATH"

    chunk_idx=0
    chunk_count="${#chunk_paths[@]}"
    while [[ "$chunk_idx" -lt "$chunk_count" ]]; do
      chunk_path="${chunk_paths[$chunk_idx]}"
      chunk_name="$(basename "$chunk_path")"
      chunk_sha="$(sha256_file "$chunk_path")"
      chunk_size="$(wc -c < "$chunk_path" | tr -d '[:space:]')"
      append_checksum_file "$chunk_path" "$chunk_name"

      {
        echo "        {"
        echo "          \"index\": ${chunk_idx},"
        echo "          \"asset_name\": \"${chunk_name}\","
        echo "          \"sha256\": \"${chunk_sha}\","
        echo "          \"size_bytes\": ${chunk_size}"
      } >> "$MODELS_MANIFEST_PATH"
      if [[ "$chunk_idx" -lt $((chunk_count - 1)) ]]; then
        echo "        }," >> "$MODELS_MANIFEST_PATH"
      else
        echo "        }" >> "$MODELS_MANIFEST_PATH"
      fi

      chunk_idx=$((chunk_idx + 1))
    done

    {
      echo "      ]"
      echo -n "    }"
    } >> "$MODELS_MANIFEST_PATH"
    if [[ "$model_idx" -lt $((model_count - 1)) ]]; then
      echo "," >> "$MODELS_MANIFEST_PATH"
    else
      echo >> "$MODELS_MANIFEST_PATH"
    fi

    model_idx=$((model_idx + 1))
  done

  {
    echo "  ]"
    echo "}"
  } >> "$MODELS_MANIFEST_PATH"

  cat > "$MODELS_META_DIR/README.txt" <<'EOF'
This bundle uses release-attached model chunk assets.
Use install.sh without --skip-models to fetch, verify, and assemble models.
EOF
fi

cat > "$BUNDLE_ROOT/bundle-manifest.json" <<EOF
{
  "schema_version": 1,
  "version": "${VERSION_STRIPPED}",
  "target": "${TARGET}",
  "artifact_kind": "${MODE}",
  "includes_models": ${INCLUDES_MODELS}
}
EOF

ASSET_NAME="termlm-${VERSION}-${TARGET}-${MODE}.tar.gz"
ASSET_PATH="${OUT_DIR}/${ASSET_NAME}"
tar -C "$STAGE_DIR" -czf "$ASSET_PATH" termlm

append_checksum_file "$ASSET_PATH" "$ASSET_NAME"

echo "$ASSET_PATH"
