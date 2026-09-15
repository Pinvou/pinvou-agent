$ErrorActionPreference = "Stop"

# Historical note: this script used to be the "sensitive dirs / dangerous
# commands" hard-deny firewall (the sudo segment existed only on the .sh
# side). Since foundation v0.9.3
# the model/execution surface only exposes the Bash tool, so segment 3
# (DANGEROUS_CMDS), gated on exec_shell*, silently stopped firing; those
# security segments have moved wholesale into the foundation execpolicy rule
# engine (typed Deny rules):
# pinvou3-app/src-tauri/src/features/assistant/safety_deny_rules.rs.
# This script keeps only the connector introspection-correction segment
# equivalent to deny_sensitive_paths.sh segment 5.
$toolName = if ($env:DEEPSEEK_TOOL_NAME) { $env:DEEPSEEK_TOOL_NAME } else { "unknown" }
$argsText = if ($env:DEEPSEEK_TOOL_ARGS) { $env:DEEPSEEK_TOOL_ARGS } else { "" }

# Skill-based connectors (WeCom/Feishu/DingTalk/Tencent Meeting) have no MCP
# schema, so a model calling list_mcp_resources* to introspect them is doomed
# to fail and misread it as "not connected". Deny the call and send the
# correction back: the upstream fold_tool_call_before_results takes the
# reason for an exit 2 only from the single-line stdout JSON
# {"decision":"deny","reason":...} (non-JSON stdout = generic copy). The copy
# deliberately never echoes the connector name nor enumerates skill/CLI names
# (disable-awareness audit, leak surface 2).
if ($toolName -eq "list_mcp_resources" -or $toolName -eq "list_mcp_resource_templates") {
    if ($argsText -match "wecom|weixin|wework|feishu|lark|dingtalk|dingding|dws|tmeet|tencent[\s_\-]?meeting|企微|企业微信|微信|飞书|钉钉|腾讯会议") {
        $denyJson = '{"decision":"deny","reason":"该名称不是 MCP server（无 MCP schema），无法用 list_mcp_resources 自省。若它是技能型连接器，请用 load_skill 加载其对应技能后按技能说明使用。连接状态以工具面板为准，自省失败不代表未连接。"}'
        # 经标准输出流写 UTF-8 无 BOM：上游按 UTF-8 解码 stdout 且 serde_json 拒绝
        # BOM 前缀；PS 5.1 控制台默认 ANSI(GBK)，WriteLine 会把中文转成乱码。
        # 不设 [Console]::OutputEncoding：无控制台句柄的宿主里 setter 会抛，
        # $ErrorActionPreference=Stop 下脚本退出 1 → 所有工具调用被 fail-closed。
        $stdout = New-Object System.IO.StreamWriter([Console]::OpenStandardOutput(), (New-Object System.Text.UTF8Encoding($false)))
        $stdout.WriteLine($denyJson)
        $stdout.Flush()
        exit 2
    }
}

exit 0
