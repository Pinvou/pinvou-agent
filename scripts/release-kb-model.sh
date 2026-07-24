#!/usr/bin/env bash
# Package the optional bge-m3 model and upload it to the public model release.
set -euo pipefail

MODEL_SRC="${PINVOU3_KB_EMBED_MODEL_DIR:-$HOME/models/bge-m3}"
TAG="${1:-kb-model-v1}"
STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT

ONNX=""
for candidate in "$MODEL_SRC/onnx/model_int8.onnx" "$MODEL_SRC/model.onnx"; do
  if [ -f "$candidate" ]; then
    ONNX="$candidate"
    break
  fi
done
if [ -z "$ONNX" ]; then
  echo "model.onnx was not found under $MODEL_SRC" >&2
  exit 1
fi

cp "$ONNX" "$STAGING/model.onnx"
for file in tokenizer.json config.json special_tokens_map.json tokenizer_config.json; do
  if [ ! -f "$MODEL_SRC/$file" ]; then
    echo "Missing model file: $file" >&2
    exit 1
  fi
  cp "$MODEL_SRC/$file" "$STAGING/$file"
done

TARBALL="$(dirname "$STAGING")/bge-m3.tar.gz"
tar czf "$TARBALL" -C "$STAGING" .
sha256sum "$TARBALL"
gh release view "$TAG" >/dev/null
gh release upload "$TAG" "$TARBALL" --clobber
echo "Uploaded bge-m3.tar.gz to GitHub Release $TAG"
