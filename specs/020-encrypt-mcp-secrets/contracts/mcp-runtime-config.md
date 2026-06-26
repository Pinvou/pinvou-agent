# 契约：MCP 运行配置

## 目标

`~/.pinvou3/bundle/mcp.json` 是 DeepSeek-TUI 底座读取的 MCP server 配置。该文件不得持久化真实 MCP API 密钥。

## 本地 MCP server

允许写入命令、参数和非敏感环境变量：

```json
{
  "servers": {
    "weather": {
      "command": "pythonw",
      "args": ["C:/Users/example/.pinvou3/bundle/mcp-servers/weather/server.py"],
      "env": {
        "PINVOU3_MCP_SECRET_REF_AMAP_KEY": "pinvou3-mcp-secret:mcp:weather:env:AMAP_KEY:v1"
      }
    }
  }
}
```

真实 `AMAP_KEY` 只能在运行期由 Pinvou 安全注入，或由 MCP server 根据凭据引用通过受控桥接取得；不得作为 `env` 明文保存。

## 远程 MCP server

禁止持久化真实 bearer header：

```json
{
  "servers": {
    "qcc-company": {
      "url": "https://agent.qcc.com/mcp/company/stream",
      "headers": {
        "Authorization": "Bearer 真实密钥"
      }
    }
  }
}
```

如果底座当前只支持静态 headers，Pinvou 不得用明文 header 兜底。必须改用安全注入路径，或在工具安装时返回缺失安全能力的明确错误。

## 验收规则

- `mcp.json` 中不得出现真实 API Key、真实 bearer token 或旧版 manifest 中的密钥值。
- 生成本地 server 配置时，敏感字段只能写凭据引用或留空并阻止启用。
- 生成远程 server 配置时，不得为了兼容旧逻辑写明文 `Authorization`。
- 错误信息必须包含工具名和缺失字段名，但不得包含密钥值。
