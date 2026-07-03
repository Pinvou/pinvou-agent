#!/bin/bash
# 发布知识库 embedding 模型(bge-m3)到 OTA 源 —— **一次性 / 模型变更才跑**。
# 模型不再随 deb 打包(见 release-deb.sh §1.5),客户端在知识库页按需下载部署。
# 流程: 组装 → 打 tar.gz → 算 sha256/size → 上传 → 打印常量供回填到
#       pinvou3-app/src-tauri/src/knowledge/model_download.rs(MODEL_SHA256 / MODEL_TARGZ_SIZE)。
#
# 用法: ./scripts/release-kb-model.sh
#   源目录默认 ~/models/bge-m3,可用 PINVOU3_KB_EMBED_MODEL_DIR 覆盖。
#   需含 int8 onnx(onnx/model_int8.onnx 或 model.onnx)+ 4 个 tokenizer 文件。
set -euo pipefail

SERVER="admin@8.218.49.20"
REMOTE_DIR="/var/www/pinvou3/kb-model"
BASE_URL="https://www.ma-xiao.com/pinvou3/kb-model"
MODEL_SRC="${PINVOU3_KB_EMBED_MODEL_DIR:-$HOME/models/bge-m3}"

# ── 1. 定位 int8 onnx(容错两种布局)+ 校验 tokenizer 齐全 ──
ONNX=""
for c in "$MODEL_SRC/onnx/model_int8.onnx" "$MODEL_SRC/model.onnx"; do
  [ -f "$c" ] && ONNX="$c" && break
done
if [ -z "$ONNX" ]; then
  echo "❌ 找不到模型 onnx(查过 onnx/model_int8.onnx 与 model.onnx)。" >&2
  echo "   设 PINVOU3_KB_EMBED_MODEL_DIR 指向 bge-m3 目录,或放到 ~/models/bge-m3" >&2
  exit 1
fi
for f in tokenizer.json config.json special_tokens_map.json tokenizer_config.json; do
  [ -f "$MODEL_SRC/$f" ] || { echo "❌ 缺 tokenizer 文件: $f" >&2; exit 1; }
done

# ── 2. 组装干净 staging(model.onnx 在根 + 4 个 tokenizer)→ 打 tar.gz ──
STAGE=$(mktemp -d)
TARBALL=$(mktemp --suffix=.tar.gz)
trap 'rm -rf "$STAGE" "$TARBALL"' EXIT
cp "$ONNX" "$STAGE/model.onnx"
cp "$MODEL_SRC"/tokenizer.json "$MODEL_SRC"/config.json \
   "$MODEL_SRC"/special_tokens_map.json "$MODEL_SRC"/tokenizer_config.json "$STAGE/"
# 文件在归档根 → 客户端解压进 model_dir 即得(对齐 model_download.rs extract_targz)。
echo "=== 打包 tar.gz ==="
tar czf "$TARBALL" -C "$STAGE" .

SHA256=$(sha256sum "$TARBALL" | awk '{print $1}')
SIZE=$(stat -c%s "$TARBALL")
echo "  sha256 = $SHA256"
echo "  size   = $SIZE bytes ($(numfmt --to=iec "$SIZE" 2>/dev/null || echo "$SIZE"))"

# ── 3. 上传到 OTA 源 ──
ssh "$SERVER" "mkdir -p $REMOTE_DIR"
rsync -avz --progress "$TARBALL" "$SERVER:$REMOTE_DIR/bge-m3.tar.gz"
ssh "$SERVER" "chmod 644 $REMOTE_DIR/bge-m3.tar.gz"

echo "=== 发布完成: $BASE_URL/bge-m3.tar.gz ==="
echo
echo "👉 回填到 pinvou3-app/src-tauri/src/knowledge/model_download.rs(随本次模型变更同 PR):"
echo "   const MODEL_SHA256: &str = \"$SHA256\";"
echo "   const MODEL_TARGZ_SIZE: u64 = $SIZE;"
echo
echo "（线上抽查）"
curl -fsSI "$BASE_URL/bge-m3.tar.gz" | grep -i "content-length\|HTTP/" || echo "(线上验证失败,检查 nginx)"
