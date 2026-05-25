#!/usr/bin/env bash
# H3C Deck Audit · 一键全跑(A → B → C → F → E)
# D 内容铁律由调用方 LLM 走 reference/content_redlines.md
# F 浏览器渲染审查(R1 新增)·可用 --skip-render 跳过(chromium 启动慢约 5s)
#
# Usage:
#   bash run_all.sh <path-to-HTML_Deck> [--skip-build] [--skip-render]

set -uo pipefail

DECK_ROOT="${1:-}"
if [[ -z "${DECK_ROOT}" ]]; then
  echo "Usage: bash run_all.sh <path-to-HTML_Deck> [--skip-build] [--skip-render]" >&2
  exit 2
fi
SKIP_BUILD=0
SKIP_RENDER=0
for arg in "$@"; do
  case "$arg" in
    --skip-build)  SKIP_BUILD=1 ;;
    --skip-render) SKIP_RENDER=1 ;;
  esac
done

SCRIPTS_DIR="$(cd "$(dirname "$0")" && pwd)"

print_banner() {
  echo
  echo "════════════════════════════════════════════════════════════"
  echo " $1"
  echo "════════════════════════════════════════════════════════════"
}

OVERALL=0

print_banner "A · 视觉规范扫描 audit_visual.py"
python3 "${SCRIPTS_DIR}/audit_visual.py" "${DECK_ROOT}"
A=$?
[[ $A -gt $OVERALL ]] && OVERALL=$A

print_banner "B · 结构与连贯扫描 audit_structure.py"
python3 "${SCRIPTS_DIR}/audit_structure.py" "${DECK_ROOT}"
B=$?
[[ $B -gt $OVERALL ]] && OVERALL=$B

print_banner "C · 演示截图清洁度扫描 audit_assets.py"
python3 "${SCRIPTS_DIR}/audit_assets.py" "${DECK_ROOT}"
C=$?
[[ $C -gt $OVERALL ]] && OVERALL=$C

if [[ $SKIP_RENDER -eq 0 ]]; then
  print_banner "F · 浏览器渲染审查 audit_render.py(chromium-headless)"
  python3 "${SCRIPTS_DIR}/audit_render.py" "${DECK_ROOT}"
  F=$?
  [[ $F -gt $OVERALL ]] && OVERALL=$F
else
  echo
  echo "(--skip-render · 跳过 chromium 渲染审查)"
fi

if [[ $SKIP_BUILD -eq 0 ]]; then
  print_banner "E · 重 build mega 单文件 rebuild_mega.sh"
  bash "${SCRIPTS_DIR}/rebuild_mega.sh" "${DECK_ROOT}"
  E=$?
  [[ $E -gt $OVERALL ]] && OVERALL=$E
else
  echo
  echo "(--skip-build · 跳过 mega.html 重 build)"
fi

print_banner "汇总"
echo "  A 视觉规范:     退出码 $A"
echo "  B 结构连贯:     退出码 $B"
echo "  C 截图清洁:     退出码 $C"
[[ $SKIP_RENDER -eq 0 ]] && echo "  F 浏览器渲染:   退出码 ${F:-skipped}"
[[ $SKIP_BUILD -eq 0 ]] && echo "  E mega rebuild: 退出码 ${E:-skipped}"
echo
echo "  D 内容铁律:    需 LLM 走 reference/content_redlines.md(8 条 checklist)"
echo
echo "  退出码: 0=全过 / 1=有 WARN / 2=有 FAIL"
echo
exit $OVERALL
