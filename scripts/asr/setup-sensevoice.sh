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
SENSEVOICE_COMMIT="c78e6919351ac83255e96de46169f518097f1ef3"
CMAKE_VERSION="3.30.5"
ASR_DIR="$HOME/.pinvou3/asr"
MODEL_FILE="sense-voice-small-${QUANT}.gguf"
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

case "$QUANT" in
  q4_k) MODEL_SHA256="c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b" ;;
  q8_0) MODEL_SHA256="f92beb119d07e42a96e3fbe6fbbb172910026f26b724c2b10fd75654c23d6912" ;;
  *) echo "❌ 不支持的量化档: $QUANT（仅支持 q4_k / q8_0）"; exit 2 ;;
esac

echo "[1/5] 检查依赖…"
for t in git gcc g++ make curl sha256sum; do
  command -v "$t" >/dev/null || { echo "❌ 缺 $t — 请先: sudo apt install build-essential git curl"; exit 1; }
done
command -v ffmpeg >/dev/null || echo "⚠️  缺 ffmpeg(浏览器录音转码用) — 建议: sudo apt install ffmpeg"
if command -v cmake >/dev/null; then
  CMAKE=cmake
else
  echo "    cmake 缺失，下载预编译(免 root)…"
  case "$(uname -m)" in
    aarch64) CMAKE_SHA256="da7dead2c92c1747b40d506d7f7d68590f5bab175316d2e7af73e48a2e417e48" ;;
    x86_64) CMAKE_SHA256="f747d9b23e1a252a8beafb4ed2bc2ddf78cff7f04a8e4de19f4ff88e9b51dc9d" ;;
    *) echo "❌ cmake 自动下载暂不支持架构: $(uname -m)"; exit 1 ;;
  esac
  curl --retry 4 --retry-all-errors -fsSL -o "$WORK/cmake.tgz" \
    "https://github.com/Kitware/CMake/releases/download/v${CMAKE_VERSION}/cmake-${CMAKE_VERSION}-linux-$(uname -m).tar.gz"
  printf '%s  %s\n' "$CMAKE_SHA256" "$WORK/cmake.tgz" | sha256sum --check --status
  tar xzf "$WORK/cmake.tgz" -C "$WORK"
  CMAKE="$WORK/$(ls "$WORK" | grep '^cmake-')/bin/cmake"
fi

echo "[2/5] 克隆 + 构建 SenseVoice.cpp（CUDA=${GGML_CUDA:-OFF}）…"
git init -q "$WORK/sv"
git -C "$WORK/sv" remote add origin https://github.com/lovemefan/SenseVoice.cpp
git -C "$WORK/sv" fetch --depth 1 origin "$SENSEVOICE_COMMIT"
git -C "$WORK/sv" checkout -q --detach FETCH_HEAD
test "$(git -C "$WORK/sv" rev-parse HEAD)" = "$SENSEVOICE_COMMIT"
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
curl --retry 4 --retry-all-errors -fsSL -o "$WORK/$MODEL_FILE" \
  "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/$MODEL_FILE"
printf '%s  %s\n' "$MODEL_SHA256" "$WORK/$MODEL_FILE" | sha256sum --check --status
install -m 0644 "$WORK/$MODEL_FILE" "$ASR_DIR/$MODEL_FILE"

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
