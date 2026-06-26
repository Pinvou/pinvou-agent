#!/usr/bin/env bash
# 在 Linux 上搭建 pinvou3 本地语音识别引擎(SenseVoice.cpp)+ 模型到 ~/.pinvou3/asr/。
#
# 这是 POC 验证过的搭建流程，也是将来「按需下载器」的基础：把同样的
# 引擎二进制 + gguf 模型 + shim 落到 ~/.pinvou3/asr/，app 设 PINVOU3_ASR_CMD
# 指向 shim 即可用本地语音输入。
#
# 用法:
#   scripts/asr/setup-sensevoice.sh [q4_k|q8_0]      # 量化档,默认 q4_k(174MB)
#   GGML_CUDA=ON scripts/asr/setup-sensevoice.sh     # GB10 等带 GPU 的机器开 CUDA 加速
#
# 依赖: git / gcc / g++ / make（apt install build-essential git）; ffmpeg(转码,建议)。
#       cmake 缺失会自动下预编译(免 root)。
set -euo pipefail

QUANT="${1:-q4_k}"
ASR_DIR="$HOME/.pinvou3/asr"
MODEL_FILE="sense-voice-small-${QUANT}.gguf"
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "[1/5] 检查依赖…"
for t in git gcc g++ make curl; do
  command -v "$t" >/dev/null || { echo "❌ 缺 $t — 请先: sudo apt install build-essential git curl"; exit 1; }
done
command -v ffmpeg >/dev/null || echo "⚠️  缺 ffmpeg(浏览器录音转码用) — 建议: sudo apt install ffmpeg"
if command -v cmake >/dev/null; then
  CMAKE=cmake
else
  echo "    cmake 缺失，下载预编译(免 root)…"
  curl -fsSL -o "$WORK/cmake.tgz" \
    "https://github.com/Kitware/CMake/releases/download/v3.30.5/cmake-3.30.5-linux-$(uname -m).tar.gz"
  tar xzf "$WORK/cmake.tgz" -C "$WORK"
  CMAKE="$WORK/$(ls "$WORK" | grep '^cmake-')/bin/cmake"
fi

echo "[2/5] 克隆 + 构建 SenseVoice.cpp（CUDA=${GGML_CUDA:-OFF}）…"
git clone --depth 1 https://github.com/lovemefan/SenseVoice.cpp "$WORK/sv"
git -C "$WORK/sv" submodule update --init --recursive
mkdir -p "$WORK/sv/build"
( cd "$WORK/sv/build" \
  && "$CMAKE" -DCMAKE_BUILD_TYPE=Release -DGGML_KOMPUTE=OFF -DGGML_VULKAN=OFF \
       -DGGML_CUDA="${GGML_CUDA:-OFF}" .. \
  && make -j"$(nproc)" sense-voice-main )

echo "[3/5] 安装引擎 → $ASR_DIR …"
mkdir -p "$ASR_DIR"
cp "$WORK/sv/build/bin/sense-voice-main" "$ASR_DIR/"

echo "[4/5] 下载模型 $MODEL_FILE（modelscope，国内可达）…"
curl -fsSL -o "$ASR_DIR/$MODEL_FILE" \
  "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/$MODEL_FILE"

echo "[5/5] 安装 shim …"
cp "$HERE/pinvou3-asr-shim.py" "$ASR_DIR/"
chmod +x "$ASR_DIR/pinvou3-asr-shim.py"

echo
echo "✅ 完成。引擎/模型/shim 已装到 $ASR_DIR"
echo
echo "接入 pinvou3（启动 app 时带环境变量）:"
echo "   PINVOU3_ASR_CMD=$ASR_DIR/pinvou3-asr-shim.py \\"
[ "$QUANT" = "q4_k" ] || echo "   SV_MODEL=$ASR_DIR/$MODEL_FILE \\"
echo "   ./pinvou3-app/run-dev.sh"
echo
echo "然后点麦克风录音即可。诊断日志: $ASR_DIR/shim.log"
