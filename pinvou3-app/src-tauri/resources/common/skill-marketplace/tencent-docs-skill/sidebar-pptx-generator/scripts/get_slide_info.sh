#!/bin/bash
# 腾讯文档 PPT 状态归并器（纯 jq，无网络、无 MCP 客户端依赖）。
#
# 原版脚本内部用 mcporter 直连 MCP；在 Pinvou 中 MCP 工具由对话工具循环执行，
# Token 不落 shell。本脚本改为消费「预先取好的 MCP 响应」：
# 调用方（模型）先用 mcp_tdoc-slide_* 工具取数，再把响应按序喂给 stdin：
#
#   第 1 行：slide_get_info 响应           → {"slide_count":N,"w_pt":N,"h_pt":N,...}
#   第 2 行：slide_get_design 响应         → {"exists":bool,"design_md":"...","updated_at":"..."}
#   第 3..N 行：每页 slide_get_page_info 响应（可选，用于 content_page_count 统计）
#
# 用法：
#   <调用工具取数> ; printf '%s\n' "$INFO" "$DESIGN" ${PAGE_INFOS[@]} | bash scripts/get_slide_info.sh
#   # 权限校验（可选）：check_access 响应作为额外首行时用 --with-access，格式 {"granted_actions":["VIEW",...]}
#
# 输出：单行 JSON {"action":"write_design_md"|"proceed_next"|"ask_user","reason":"...",...}

set -euo pipefail

WITH_ACCESS=0
[[ "${1:-}" == "--with-access" ]] && WITH_ACCESS=1

# 读入所有输入行
LINES=()
while IFS= read -r line; do
    [[ -n "$line" ]] && LINES+=("$line")
done

IDX=0
EMPTY_JSON='{}'
if [[ "$WITH_ACCESS" -eq 1 ]]; then
    ACCESS_JSON="${LINES[$IDX]:-$EMPTY_JSON}"
    IDX=$((IDX + 1))
else
    ACCESS_JSON=""
fi

INFO_JSON="${LINES[$IDX]:-$EMPTY_JSON}"
IDX=$((IDX + 1))
DESIGN_JSON="${LINES[$IDX]:-$EMPTY_JSON}"
IDX=$((IDX + 1))
PAGE_JSONS=("${LINES[@]:$IDX}")

SLIDE_COUNT=$(echo "$INFO_JSON" | jq -r '.slide_count // 0')
W_PT=$(echo "$INFO_JSON" | jq -r '.w_pt // 0')
H_PT=$(echo "$INFO_JSON" | jq -r '.h_pt // 0')

# ── 权限判定 ──
# slide_count=0 但尺寸有效 → 大概率缺 VIEW 权限而非真空 PPT。
if [[ "$SLIDE_COUNT" -eq 0 && "$W_PT" -gt 0 && "$H_PT" -gt 0 ]]; then
    HAS_VIEW="false"
    if [[ -n "$ACCESS_JSON" ]]; then
        HAS_VIEW=$(echo "$ACCESS_JSON" | jq -r '(.granted_actions // []) | map(select(. == "VIEW")) | length > 0' 2>/dev/null || echo "false")
    fi
    if [[ "$HAS_VIEW" != "true" ]]; then
        jq -n --arg reason "permission_denied" --argjson sc "$SLIDE_COUNT" --argjson w "$W_PT" --argjson h "$H_PT" \
            '{action:"ask_user",reason:$reason,slide_count:$sc,w_pt:$w,h_pt:$h,hint:"当前账号无 VIEW 权限，请分享文档或下载到本地后重试"}'
        exit 0
    fi
fi

if [[ "$SLIDE_COUNT" -eq 0 || "$W_PT" -eq 0 || "$H_PT" -eq 0 ]]; then
    jq -n --arg reason "ppt_is_empty" --argjson sc "$SLIDE_COUNT" --argjson w "$W_PT" --argjson h "$H_PT" \
        '{action:"write_design_md",reason:$reason,slide_count:$sc,w_pt:$w,h_pt:$h}'
    exit 0
fi

CONTENT_PAGE_COUNT=0
for PAGE_JSON in "${PAGE_JSONS[@]:-}"; do
    [[ -z "$PAGE_JSON" ]] && continue
    HAS_CONTENT=$(echo "$PAGE_JSON" | jq '
        [.shapes // [] | .[] | select((.text // "") | gsub("[\\s\\r\\n]"; "") != "")] | length > 0
    ' 2>/dev/null || echo "false")
    [[ "$HAS_CONTENT" == "true" ]] && CONTENT_PAGE_COUNT=$((CONTENT_PAGE_COUNT + 1))
done

if [[ "$CONTENT_PAGE_COUNT" -eq 0 ]]; then
    jq -n --arg reason "ppt_content_is_empty" --argjson sc "$SLIDE_COUNT" --argjson w "$W_PT" --argjson h "$H_PT" --argjson cpc "$CONTENT_PAGE_COUNT" \
        '{action:"write_design_md",reason:$reason,slide_count:$sc,w_pt:$w,h_pt:$h,content_page_count:$cpc}'
    exit 0
fi

DESIGN_EXISTS=$(echo "$DESIGN_JSON" | jq -r '.exists // false')
DESIGN_MD=$(echo "$DESIGN_JSON" | jq -r '.design_md // ""')

if [[ "$DESIGN_EXISTS" == "false" || -z "$DESIGN_MD" || "$DESIGN_MD" == '""' ]]; then
    jq -n --arg reason "design_is_empty" --argjson sc "$SLIDE_COUNT" --argjson w "$W_PT" --argjson h "$H_PT" --argjson cpc "$CONTENT_PAGE_COUNT" --argjson de "$DESIGN_EXISTS" \
        '{action:"write_design_md",reason:$reason,slide_count:$sc,w_pt:$w,h_pt:$h,content_page_count:$cpc,design_exists:$de}'
    exit 0
fi

DESIGN_MD_LEN=${#DESIGN_MD}
UPDATED_AT=$(echo "$DESIGN_JSON" | jq -r '.updated_at // "0"')

jq -n --argjson sc "$SLIDE_COUNT" --argjson w "$W_PT" --argjson h "$H_PT" --argjson cpc "$CONTENT_PAGE_COUNT" --argjson de "$DESIGN_EXISTS" --argjson dml "$DESIGN_MD_LEN" --arg ua "$UPDATED_AT" \
    '{action:"proceed_next",reason:"design_exists",slide_count:$sc,w_pt:$w,h_pt:$h,content_page_count:$cpc,design_exists:$de,design_md_length:$dml,updated_at:$ua}'
