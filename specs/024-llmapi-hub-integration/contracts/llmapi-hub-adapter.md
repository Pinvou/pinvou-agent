# 契约：LLM API Hub / New API 管理适配器

本文描述 Pinvou Windows 后端内部 adapter 的语义契约。用户查询按 QuantumNous/new-api 标准管理接口实现；token 代创建/读取明文 token 不是标准 New API 管理接口能力，需要中转站侧提供专用管理端点。非 Windows 平台本次不实现该 adapter 调用链。

## Adapter 配置

```json
{
  "chat_base_url": "https://www.ma-xiao.com/llmapi/v1",
  "admin_base_url": "https://www.ma-xiao.com/llmapi",
  "admin_credential_ref": {
    "service": "pinvou3-llmapi-admin",
    "account": "newapi:admin",
    "version": 1
  },
  "default_model_allowlist": ["deepseek-v4-flash"],
  "default_quota_limit_tokens": 1000000,
  "default_rpm_limit": 60
}
```

**约束**：

- `chat_base_url` 面向 OpenAI-compatible 调用。
- `admin_base_url` 面向 New API 管理能力，默认拼接 `/api/user/search` 查询用户。
- 管理凭证不得写入普通配置明文；凭证内容需包含 New API 管理员 `user_id` 和 `access_token`，请求时发送 `Authorization: Bearer <access_token>` 与 `New-Api-User: <user_id>`。

## lookup_user

为设备派生身份查询既有 New API 用户；不得在该步骤创建后台用户。

**输入**：

```json
{
  "pinvou_user_id": "dev_abcd1234",
  "device_binding_id": "dev_abcd1234",
  "idempotency_key": "pinvou:dev_abcd1234:dev_abcd1234"
}
```

**输出**：

```json
{
  "newapi_user_id": "1001",
  "exists": true
}
```

**要求**：

- adapter 应调用 `GET /api/user/search?keyword=<pinvou_user_id>&p=1&size=20` 查询 New API 用户，并在返回列表中按 `username == pinvou_user_id` 做精确匹配。
- 若 New API 后台不存在对应用户，adapter 必须返回 `user_not_found`，不得自动创建用户。
- 不得把 BIOS SN 明文写入 New API 用户资料、请求日志或错误信息。

## create_token

为 New API 用户创建或查找一个 Pinvou 专用 token。

**当前 New API 标准源码限制**：`/api/token` 路由使用 `UserAuth`，只能为当前 access token 所属用户管理 token，不能由管理员直接为任意 `newapi_user_id` 创建或读取明文 token。因此 Pinvou 不调用标准 `/api/token` 猜测实现，必须配置 `PINVOU3_LLMAPI_CREATE_TOKEN_ENDPOINT` 指向中转站侧新增的专用管理端点。

**输入**：

```json
{
  "newapi_user_id": "1001",
  "pinvou_user_id": "dev_abcd1234",
  "device_binding_id": "dev_abcd1234",
  "idempotency_key": "pinvou:dev_abcd1234:dev_abcd1234:default-token"
}
```

**输出**：

```json
{
  "newapi_token_id": "2001",
  "token_plaintext": "sk-xxxxxxxx",
  "created": true
}
```

**要求**：

- `token_plaintext` 只能返回给调用方一次，并必须立即写入 `credential_store`。
- 日志和错误不得打印 `token_plaintext`。
- 重试时如无法再次取得旧 token 明文，应创建新 token 并禁用旧 token，或进入管理员可恢复失败状态。

## configure_policy

为 New API 用户或 token 设置额度、模型权限和限流。

**输入**：

```json
{
  "newapi_user_id": "1001",
  "newapi_token_id": "2001",
  "quota_limit_tokens": 1000000,
  "model_allowlist": ["deepseek-v4-flash"],
  "rpm_limit": 60
}
```

**输出**：

```json
{
  "applied": true,
  "quota_limit_tokens": 1000000
}
```

**要求**：

- 若 New API 使用额度单位不是 tokens，adapter 负责转换或记录当前不可转换原因。
- 模型白名单应采用 New API 后台当前可识别的模型名。

## disable_token

禁用用户对应 New API token。

**输入**：

```json
{
  "newapi_user_id": "1001",
  "newapi_token_id": "2001",
  "reason": "disabled_by_pinvou_admin"
}
```

**输出**：

```json
{
  "disabled": true
}
```

**要求**：

- 本地禁用优先，即使远端禁用失败，Pinvou 也不得继续使用该 token。
- 远端失败需记录脱敏错误，供管理员重试。

## get_usage_or_quota

从 New API 同步用户额度或调用消耗，作为 Pinvou 本地快照的校准来源。

**输入**：

```json
{
  "newapi_user_id": "1001",
  "newapi_token_id": "2001",
  "period": "2026-07"
}
```

**输出**：

```json
{
  "quota_limit_tokens": 1000000,
  "quota_used_tokens": 120000,
  "quota_remaining_tokens": 880000,
  "source": "newapi"
}
```

**要求**：

- 若 New API 暂不支持精确 tokens 查询，Pinvou 可继续使用本地 usage 累计。
- 同步失败不得阻塞已开通用户发起模型调用。

## OpenAI-compatible 模型调用

模型调用不由 adapter 直接实现，仍走 DeepSeek-TUI Engine。adapter 只提供：

```json
{
  "provider": "openai",
  "base_url": "https://www.ma-xiao.com/llmapi/v1",
  "model": "deepseek-v4-flash",
  "token_credential_ref": {
    "service": "pinvou3-llmapi-token",
    "account": "llmapi:dev_abcd1234:dev_abcd1234",
    "version": 1
  }
}
```

**错误分类映射**：

| 上游情况 | Pinvou 错误分类 |
|---|---|
| 管理接口 401/403 | `admin_auth_failed` |
| token 调用 401/403 | `auth_failed` |
| 额度不足 | `quota_exceeded` |
| 429 | `rate_limited` |
| 超时或 DNS/TLS 失败 | `service_unreachable` |
| 5xx | `upstream_error` |
| 解析失败 | `unknown` |
