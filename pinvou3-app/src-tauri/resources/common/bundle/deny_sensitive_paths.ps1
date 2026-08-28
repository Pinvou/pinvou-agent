$ErrorActionPreference = "Stop"

# 历史说明：本脚本曾是「敏感目录/危险命令」硬拦截防火墙（sudo 段只在 .sh 侧）。
# 自底座 v0.9.3 起
# 模型/执行面只暴露 Bash 工具，第 3 段（DANGEROUS_CMDS）按 exec_shell* 门控已静默
# 失效；这些安全段已整体迁移至底座 execpolicy 规则引擎（typed Deny 规则）：
# pinvou3-app/src-tauri/src/features/assistant/safety_deny_rules.rs。
# 本脚本只保留与 deny_sensitive_paths.sh 规则 5 对等的连接器自省纠正段。
$toolName = if ($env:DEEPSEEK_TOOL_NAME) { $env:DEEPSEEK_TOOL_NAME } else { "unknown" }
$argsText = if ($env:DEEPSEEK_TOOL_ARGS) { $env:DEEPSEEK_TOOL_ARGS } else { "" }

# 技能型连接器（企微/飞书/钉钉/腾讯会议）无 MCP schema，模型调 list_mcp_resources*
# 自省必然失败并误判「没连上」。拦掉并把纠正回传：上游 fold_tool_call_before_results
# 在 exit 2 时只从 stdout 的单行 JSON {"decision":"deny","reason":...} 取 reason 喂回
# 模型（非 JSON = 通用文案），文案刻意不回显连接器名、不列举技能/CLI 名（disable
# 感知审计，泄漏面 2）。
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
