# 数据模型：加密存储 MCP API 密钥

## MCP 工具清单

表示内置或工具市场 MCP 工具的非敏感元数据。

**字段**
- `id`：工具唯一标识，例如 `weather`、`iwencai`、`qcc`。
- `name`：用户可见名称。
- `command` / `args`：本地 MCP server 启动声明；远程 MCP 工具可为空。
- `servers`：远程 MCP server 列表，包含服务名和 URL。
- `mcp_tools`：工具名称列表或前缀展示信息。
- `secret_env`：敏感环境变量声明列表，不包含真实值。
- `secret_headers`：敏感请求头声明列表，不包含真实值。
- `config_fields`：用户可配置字段，敏感字段必须标记为 secret。
- `routing_rules` / `tool_table_entries`：模型路由提示和工具表展示文本。

**验证规则**
- 清单文件不得包含真实 API Key、bearer token 或可直接调用供应商 API 的秘密。
- `secret_env` 与 `secret_headers` 中的每个条目必须能映射到稳定的 MCP 密钥凭据。
- 非敏感 `env` 仍可保留，但不得包含 `_API_KEY`、`TOKEN`、`SECRET`、`KEY` 等敏感命名且带真实值的字段。

## MCP 密钥凭据

表示某个 MCP 工具或供应商所需的敏感访问令牌。

**字段**
- `provider`：供应商或能力域，例如 `amap`、`iwencai`、`qcc`。
- `tool_id`：关联工具 id。
- `target`：密钥注入目标，例如环境变量或 bearer header。
- `name`：目标字段名，例如 `AMAP_KEY`、`IWENCAI_API_KEY`、`QCC_API_KEY`。
- `credential_ref`：持久化凭据引用，包含 service、account、version。
- `state`：`missing`、`configured`、`needs_migration`、`unavailable`。
- `source`：密钥来源，例如旧配置迁移、用户配置、部署预置。

**关系**
- 一个 MCP 工具可以有多个 MCP 密钥凭据。
- 多个远程 server 可以共享同一个供应商凭据，例如企查查多个远程 server 共享 `QCC_API_KEY`。
- 一个凭据引用只对应一个真实秘密。

**状态转换**
- `missing` -> `configured`：用户配置、部署预置或旧配置迁移成功。
- `needs_migration` -> `configured`：检测到旧版明文并迁移成功。
- `needs_migration` -> `unavailable`：检测到旧版明文但系统凭据写入失败。
- `configured` -> `missing`：系统凭据被外部删除。

## MCP 运行配置

表示写入 `~/.pinvou3/bundle/mcp.json` 的底座运行配置。

**字段**
- `servers`：底座可见 MCP server 映射。
- `command` / `args`：本地 server 启动命令和参数。
- `env`：非敏感环境变量或凭据引用，不包含真实密钥。
- `headers`：非敏感请求头或凭据引用，不包含真实 bearer token。

**验证规则**
- 不得持久化真实 API Key 或 `Authorization: Bearer <secret>`。
- 对于本地 MCP server，运行前必须能把所需凭据安全注入子进程环境，或者返回明确缺失错误。
- 对于远程 MCP server，不得把 bearer header 明文写入长期配置；若底座无法动态注入，应禁止安装并提示需要安全配置路径。

## 密钥迁移记录

表示旧版明文配置的处理结果。

**字段**
- `tool_id`：被迁移工具 id。
- `credential_ref`：迁移后的凭据引用。
- `source_path`：被清理的旧配置文件路径。
- `source_field`：旧配置中的字段名。
- `status`：`migrated`、`skipped`、`failed`。
- `message`：脱敏后的说明。
- `updated_at`：迁移处理时间。

**验证规则**
- 迁移记录不得包含真实密钥。
- 同一凭据重复迁移时不得覆盖已有有效凭据，除非用户明确要求替换。
- 迁移失败时不得把明文复制到新的配置位置。
