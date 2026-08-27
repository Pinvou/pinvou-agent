#!/usr/bin/env bash
# pinvou3 抸能型连接器 MCP 自省纠正 hook
#
# 历史说明：本脚本曾是「敏感目录/危险命令/sudo」硬拦截防火墙（5 段规则）。
# 自底座 v0.9.3 起模型/执行面只暴露 Bash 工具，第 3 段（DANGEROUS_CMDS）与
# 第 4 段（sudo 拦截）按 `$TOOL == "exec_shell"*` 门控已静默失效；第 1/2 段
# （路径/文件名子串）虽仍生效但按 ARGS 子串匹配误伤面大。这四段已整体迁移至
# 底座 execpolicy 规则引擎（typed Deny 规则，先于审批/hook 执行，覆盖嵌套
# 子代理）：pinvou3-app/src-tauri/src/features/assistant/safety_deny_rules.rs。
# 本脚本只保留第 5 段——连接器自省纠正，其工具名（list_mcp_resources*）未变
# 且属于引导性反馈，不是危险命令策略。
#
# CodeWhale 在 ToolCallBefore 事件 spawn 这个脚本，通过环境变量传入工具
# 调用参数。需要 hard-deny 时必须 **exit 2**（v0.8.60 Hooks v2 契约，
# #3026/#3049）：turn_loop.rs fold_tool_call_before_results 只认 exit_code==2
# 或 stdout JSON {"decision":"deny"}；exit 1 会被当作 passthrough(ALLOW)。

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
