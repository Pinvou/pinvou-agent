# 契约：MCP 密钥迁移

## 目标

升级用户可能已经在 `manifest.json` 或 `mcp.json` 中残留明文密钥。新版应用必须迁移并清理这些旧数据。

## 输入范围

- `~/.pinvou3/bundle/mcp-servers/weather/manifest.json`
- `~/.pinvou3/bundle/mcp-servers/iwencai/manifest.json`
- `~/.pinvou3/bundle/mcp-servers/qcc/manifest.json`
- `~/.pinvou3/bundle/mcp.json`

## 识别规则

目标字段：

- `AMAP_KEY`
- `IWENCAI_API_KEY`
- `QCC_API_KEY`
- `Authorization: Bearer <secret>`，当所属 server 属于目标供应商时

## 迁移流程

1. 读取旧配置文件。
2. 检测目标字段是否存在非空明文值。
3. 为字段生成稳定凭据引用。
4. 如果凭据不存在，写入系统凭据存储。
5. 如果凭据已存在，不覆盖已有值，记录 skipped。
6. 从原文件移除明文值，改写为安全声明、凭据引用或删除敏感字段。
7. 写回文件时使用不带 BOM 的 UTF-8 JSON。

## 失败处理

- 凭据写入失败：不得清理唯一明文来源，返回脱敏错误，提示用户需要重新配置或修复系统凭据访问。
- JSON 解析失败：不得覆盖原文件，返回脱敏错误。
- 部分迁移成功：成功项必须清理明文，失败项保持可恢复并报告具体工具和字段。

## 验收规则

- 迁移日志和错误中不得包含真实密钥。
- 迁移后再次运行迁移必须幂等，不重复写入、不重复报错。
- 对同一供应商多个远程 server 共享的密钥，只保存一个凭据引用。
