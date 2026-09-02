#!/usr/bin/env bash
# pinvou3 skill-based-connector MCP introspection-correction hook
#
# Historical note: this script used to be the "sensitive dirs / dangerous
# commands / sudo" hard-deny firewall (5 rule segments). Since foundation
# v0.9.3 the model/execution surface only exposes the Bash tool, so segments
# 3 (DANGEROUS_CMDS) and 4 (sudo block), gated on `$TOOL == "exec_shell"*`,
# silently stopped firing; segments 1/2 (path/filename substrings) still fired
# but their full-ARGS substring matching had a large false-positive surface.
# Those four segments have moved wholesale into the foundation execpolicy rule
# engine (typed Deny rules; evaluated after the ToolCallBefore hook and before
# approval; nested subagents do not pass through it yet — see the
# safety_deny_rules module docs):
# pinvou3-app/src-tauri/src/features/assistant/safety_deny_rules.rs.
# This script keeps only segment 5 — the connector introspection correction.
# Its tool names (list_mcp_resources*) are unchanged and it is advisory
# feedback, not dangerous-command policy.
#
# CodeWhale spawns this script on the ToolCallBefore event and passes the tool
# call arguments via environment variables. A hard-deny must **exit 2** (the
# v0.8.60 Hooks v2 contract, #3026/#3049): turn_loop.rs
# fold_tool_call_before_results only accepts exit_code==2 or the stdout JSON
# {"decision":"deny"}; exit 1 is treated as passthrough (ALLOW).

set -uo pipefail

ARGS="${DEEPSEEK_TOOL_ARGS:-}"
TOOL="${DEEPSEEK_TOOL_NAME:-unknown}"

# 5) 技能型连接器被误当 MCP 自省：企微/飞书/钉钉/腾讯会议是「技能型连接器」
#    （无 MCP schema），模型却可能对它们调 list_mcp_resources / list_mcp_resource_templates
#    去自省能力 → 必然失败 → 误判「没连上」，甚至谎称缺技能。这里拦掉并把纠正回传：
#    fold_tool_call_before_results 在 exit 2 时只从 stdout 的 JSON {"decision":"deny",
#    "reason":...} 取 reason 喂回模型（非 JSON stdout = passthrough，reason 落为通用
#    文案），所以必须输出单行 JSON，引导模型改用 load_skill。
#    取代原 bundle/instructions.md 常驻那条软纪律：零常驻 prompt + 现场硬反馈对小模型更准。
#    文案刻意不回显连接器名、不列举技能/CLI 名：模型问一个不应连带知道全部，
#    且对「已禁用」的连接器不确认其存在（disable 感知审计，泄漏面 2）。
if [[ "$TOOL" == "list_mcp_resources" || "$TOOL" == "list_mcp_resource_templates" ]]; then
    # 关键词覆盖模型可能传的各种写法:英文 wecom/weixin/wework、中文全称「企业微信」
    # (注意「企微」子串不含在「企业微信」里,必须显式列全称)、feishu/lark/飞书、
    # 以及 dingtalk/dingding/dws/钉钉、tmeet/tencent meeting/腾讯会议。
    if [[ "$ARGS" =~ (wecom|weixin|wework|feishu|lark|dingtalk|dingding|dws|tmeet|tencent[[:space:]_-]?meeting|企微|企业微信|微信|飞书|钉钉|腾讯会议) ]]; then
        echo '{"decision":"deny","reason":"该名称不是 MCP server（无 MCP schema），无法用 list_mcp_resources 自省。若它是技能型连接器，请用 load_skill 加载其对应技能后按技能说明使用。连接状态以工具面板为准，自省失败不代表未连接。"}'
        exit 2
    fi
fi

exit 0
