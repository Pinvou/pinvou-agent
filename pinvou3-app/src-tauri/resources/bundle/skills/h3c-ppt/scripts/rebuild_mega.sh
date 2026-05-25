#!/usr/bin/env bash
# h3c-ppt skill · 一键重 build mega 单文件(skill 自带工具,项目无需 _tools/)
# 1) 备份当前 slides/
# 2) inline_images.py(把 assets 图片 base64 嵌入 inline/slides/*.html)
# 3) build_mega.py(把所有 inline slides 压成单文件 mega.html)
#
# Usage:
#   bash rebuild_mega.sh <path-to-HTML_Deck>

set -euo pipefail

DECK_ROOT="${1:-}"
if [[ -z "${DECK_ROOT}" ]]; then
  echo "❌ 缺参数。Usage: bash rebuild_mega.sh <path-to-HTML_Deck>" >&2
  exit 2
fi
DECK_ROOT="$(cd "${DECK_ROOT}" && pwd)"
SLIDES_DIR="${DECK_ROOT}/slides"
INLINE_DIR="$(dirname "${DECK_ROOT}")/$(basename "${DECK_ROOT}")_inline"

# Skill 自带工具(同目录,不依赖项目 _tools/)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INLINE_TOOL="${SCRIPT_DIR}/inline_images.py"
BUILD_TOOL="${SCRIPT_DIR}/build_mega.py"

if [[ ! -d "${SLIDES_DIR}" ]]; then
  echo "❌ 未找到 ${SLIDES_DIR}" >&2; exit 2
fi
if [[ ! -f "${INLINE_TOOL}" ]] || [[ ! -f "${BUILD_TOOL}" ]]; then
  echo "❌ skill 损坏:scripts/ 内缺 inline_images.py 或 build_mega.py" >&2; exit 2
fi

echo "deck root: ${DECK_ROOT}"
echo "build tools (skill-bundled): ${SCRIPT_DIR}"
echo

# ── Step 1: 备份 ──
TS="$(date +%Y%m%d-%H%M)"
BAK_DIR="${DECK_ROOT}/slides.bak.${TS}"
if [[ -d "${BAK_DIR}" ]]; then
  echo "⚠️  备份目录已存在 ${BAK_DIR},跳过备份"
else
  echo "▶ Step 1/3 备份 slides/ → slides.bak.${TS}"
  cp -r "${SLIDES_DIR}" "${BAK_DIR}"
  echo "  ✓ 备份完成 ($(ls "${BAK_DIR}" | wc -l) 个文件)"
fi
echo

# ── Step 2: inline ──
echo "▶ Step 2/3 inline_images.py(图片 base64 内联)"
if ! python3 "${INLINE_TOOL}" "${DECK_ROOT}" 2>&1 | tail -5; then
  echo "❌ inline_images.py 失败" >&2; exit 1
fi
echo

# ── Step 3: build_mega ──
echo "▶ Step 3/3 build_mega.py(合并单文件)"
if ! python3 "${BUILD_TOOL}" "${DECK_ROOT}" 2>&1 | tail -3; then
  echo "❌ build_mega.py 失败" >&2; exit 1
fi
echo

# ── 报告 ──
MEGA="${INLINE_DIR}/mega.html"
if [[ -f "${MEGA}" ]]; then
  SZ=$(du -h "${MEGA}" | cut -f1)
  echo "✅ 单文件已生成"
  echo "   路径: ${MEGA}"
  echo "   体积: ${SZ}"
else
  echo "❌ 未生成 ${MEGA}" >&2; exit 1
fi
