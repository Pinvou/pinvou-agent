# 契约：Pinvou 前端与 Tauri 后端命令

本文描述本 feature 需要新增或扩展的 Tauri 命令。本功能仅在 Windows 系统上实现；非 Windows 系统不得查询绑定或调用中转站，如命令被调用应返回 `unsupported_platform`。字段名以实现时 serde 输出为准，前端不得接收 New API token 明文。

## get_llmapi_status

获取当前设备派生身份的中转站开通、额度和可用状态。

**调用**：

```ts
invoke("get_llmapi_status")
```

**响应**：

```json
{
  "pinvou_user_id": "dev_abcd1234",
  "device_binding_status": "bound",
  "enabled": true,
  "provisioning_status": "ready",
  "quota": {
    "period": "2026-07",
    "limit_tokens": 1000000,
    "used_tokens": 120000,
    "remaining_tokens": 880000,
    "unmetered_call_count": 0,
    "last_synced_at": "2026-07-08T10:20:30Z"
  },
  "last_call_status": "success",
  "last_error_code": null,
  "last_error_message": null
}
```

**错误**：

- `not_logged_in`：保留错误分类；当前设备身份方案通常不使用。
- `unsupported_platform`：当前系统不是 Windows，本功能不可用。
- `device_not_bound`：当前设备未通过可选绑定校验。
- `device_binding_failed`：BIOS SN 读取失败或与绑定记录不一致。
- `service_disabled`：查询绑定策略关闭或该用户被禁用。
- `unavailable`：本地状态读取失败。

## ensure_llmapi_binding

确保当前设备派生身份拥有可用中转站绑定。通常由发送 AI 消息前的后端路径调用，前端也可在打开 AI 助手时预热。

**调用**：

```ts
invoke("ensure_llmapi_binding")
```

**响应**：

```json
{
  "status": "ready",
  "created": true,
  "retryable": false,
  "message": "AI 服务已开通"
}
```

**错误**：

- `not_logged_in`
- `unsupported_platform`
- `device_not_bound`
- `device_binding_failed`
- `admin_credential_missing`
- `provisioning_failed`
- `service_unreachable`
- `rate_limited`
- `service_disabled`

**约束**：

- 响应不得包含 New API token。
- 同一设备派生身份重复调用必须幂等。

## retry_llmapi_provisioning

管理员对指定设备派生身份重试查询绑定。

**调用**：

```ts
invoke("retry_llmapi_provisioning", {
  "pinvouUserId": "dev_abcd1234",
  "deviceBindingId": "dev_abcd1234"
})
```

**响应**：

```json
{
  "pinvou_user_id": "dev_abcd1234",
  "device_binding_status": "bound",
  "status": "ready",
  "retryable": false
}
```

**错误**：

- `permission_denied`
- `user_not_found`
- `provisioning_failed`
- `service_unreachable`

## set_llmapi_user_enabled

管理员启用或禁用指定设备派生身份的 AI 服务。

**调用**：

```ts
invoke("set_llmapi_user_enabled", {
  "pinvouUserId": "dev_abcd1234",
  "enabled": false
})
```

**响应**：

```json
{
  "pinvou_user_id": "dev_abcd1234",
  "enabled": false
}
```

**行为**：

- 禁用后 Pinvou 后端不得继续使用该用户 token 调用中转站。
- 如 New API 管理接口可用，应同步禁用对应 token。
- 同步失败时本地禁用仍应生效，并记录管理员可见错误。

## get_llmapi_admin_overview

管理员查看用户开通概览。

**调用**：

```ts
invoke("get_llmapi_admin_overview", {
  "query": "",
  "status": "failed",
  "limit": 50,
  "offset": 0
})
```

**响应**：

```json
{
  "items": [
    {
      "pinvou_user_id": "dev_abcd1234",
      "device_binding_status": "bound",
      "enabled": true,
      "provisioning_status": "failed",
      "newapi_user_id": "1001",
      "newapi_token_id": null,
      "quota_used_tokens": 0,
      "quota_limit_tokens": 1000000,
      "last_error_code": "service_unreachable",
      "last_error_message": "New API 管理接口暂不可用",
      "updated_at": "2026-07-08T10:20:30Z"
    }
  ],
  "total": 1
}
```

**约束**：

- 列表不得包含 token 明文。
- 列表不得包含 BIOS SN 明文；如需展示设备信息，仅展示脱敏后的设备绑定状态或标识。
- 错误摘要必须脱敏。

## chat 命令接入约束

现有 `chat(...)` 命令在发送到 `EnginePool` 前应确保：

1. 已由当前设备 BIOS SN 派生 `device_binding_id`，并将其作为 `pinvou_user_id`。
2. 若当前模型策略选择 LLM API Hub，则调用 `ensure_llmapi_binding` 的后端逻辑。
3. 绑定 `ready && enabled` 且当前设备绑定标识一致后，当前 session 的 engine 使用 LLM API Hub 的 `base_url`、`model` 和 keyring 中 token。
4. 设备绑定失败或中转站绑定失败时返回用户友好的错误分类，不进入 DeepSeek-TUI 模型调用链。
