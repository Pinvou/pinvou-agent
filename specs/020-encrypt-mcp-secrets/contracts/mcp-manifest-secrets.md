# 契约：MCP Manifest 密钥声明

## 目标

内置 MCP manifest 描述工具需要哪些敏感凭据，但不得保存真实 API Key。

## 适用文件

- `pinvou3-app/resources/mcp-servers/weather/manifest.json`
- `pinvou3-app/resources/mcp-servers/iwencai/manifest.json`
- `pinvou3-app/resources/mcp-servers/qcc/manifest.json`
- 启动后写入的 `~/.pinvou3/bundle/mcp-servers/*/manifest.json`

## 允许结构

```json
{
  "id": "weather",
  "env": {},
  "secret_env": [
    {
      "key": "AMAP_KEY",
      "provider": "amap",
      "required": true
    }
  ]
}
```

远程 bearer 场景：

```json
{
  "id": "qcc",
  "servers": [
    { "name": "qcc-company", "url": "https://example.invalid/mcp/company/stream" }
  ],
  "secret_headers": [
    {
      "header": "Authorization",
      "scheme": "Bearer",
      "source_key": "QCC_API_KEY",
      "provider": "qcc",
      "required": true
    }
  ]
}
```

## 禁止结构

```json
{
  "env": {
    "AMAP_KEY": "真实密钥"
  }
}
```

```json
{
  "headers": {
    "Authorization": "Bearer 真实密钥"
  }
}
```

## 验收规则

- manifest 中不得出现真实供应商密钥。
- 敏感字段必须以声明形式存在，声明中只包含字段名、供应商、是否必需、注入目标等非敏感信息。
- `config_fields` 如承载用户输入密钥，必须标记为 secret，并在保存时进入受保护凭据位置。
- 写入用户目录的 manifest 必须与内置资源一致遵守本契约。
