# 数据模型：Pinvou 接入 LLM API Hub 中转站

## LlmApiBinding

Windows 系统上本地设备派生身份、设备绑定标识与 New API 后台资源的绑定关系。

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | string/uuid | 本地绑定记录 ID |
| `pinvou_user_id` | string | 与 `device_binding_id` 相同的本地设备派生身份 |
| `device_binding_id` | string | 由当前设备 BIOS SN 规范化后派生的设备绑定标识 |
| `bios_sn_hash` | string nullable | BIOS SN 的哈希或脱敏校验值，不保存明文 SN |
| `newapi_user_id` | integer/string nullable | New API 后台用户 ID |
| `newapi_token_id` | integer/string nullable | New API token ID |
| `token_credential_ref` | CredentialReference nullable | 指向 keyring 中的 New API token |
| `provisioning_status` | ProvisioningStatus | 查询绑定状态 |
| `enabled` | boolean | 管理员是否允许该用户使用 AI 服务 |
| `quota_limit_tokens` | integer nullable | Pinvou 展示用额度上限快照 |
| `quota_used_tokens` | integer | Pinvou 展示用已用 tokens |
| `quota_remaining_tokens` | integer nullable | Pinvou 展示用剩余 tokens |
| `last_usage_at` | datetime nullable | 最近一次 usage 更新 |
| `last_call_status` | string nullable | 最近一次模型调用状态 |
| `last_error_code` | string nullable | 最近一次错误分类 |
| `last_error_message` | string nullable | 脱敏后的管理员可见错误摘要 |
| `created_at` | datetime | 创建时间 |
| `updated_at` | datetime | 更新时间 |

**关系**：
- 一个 `pinvou_user_id + device_binding_id` 组合最多一个有效 `LlmApiBinding`，当前实现中两者取值相同。
- 一个绑定最多指向一个 New API 用户和一个当前有效 token。
- token 明文只存在 `token_credential_ref` 指向的凭证存储中。
- BIOS SN 明文不得保存到绑定记录、普通日志或前端状态。

**验证规则**：
- `pinvou_user_id` 和 `device_binding_id` 必填且组合唯一，当前实现中 `pinvou_user_id = device_binding_id`。
- 该绑定仅在 Windows 系统上创建；非 Windows 系统不得创建 `LlmApiBinding`。
- `enabled=false` 时不得继续调用模型。
- `provisioning_status=ready` 时必须存在 `newapi_user_id`、`newapi_token_id` 和 `token_credential_ref`。
- 写入日志或前端返回时不得包含 token 明文。
- 当前设备绑定标识与绑定记录不一致时，不得继续调用模型。

## ProvisioningTask

首次使用查询绑定流程的可恢复状态。

| 字段 | 类型 | 说明 |
|---|---|---|
| `pinvou_user_id` | string | 任务所属本地设备派生身份 |
| `device_binding_id` | string | 任务所属设备绑定标识 |
| `status` | ProvisioningStatus | 当前阶段 |
| `attempt_count` | integer | 已尝试次数 |
| `last_attempt_at` | datetime nullable | 最近一次尝试时间 |
| `next_retry_after` | datetime nullable | 建议下次重试时间 |
| `idempotency_key` | string | 对同一本地设备派生身份和设备绑定标识稳定的幂等键 |
| `partial_newapi_user_id` | integer/string nullable | 已查询到但尚未完成绑定的 New API 用户 ID |
| `partial_newapi_token_id` | integer/string nullable | 已创建但尚未完成绑定的 token ID |
| `last_error_code` | string nullable | 失败分类 |
| `last_error_message` | string nullable | 脱敏错误摘要 |

**状态枚举**：

- `not_started`
- `querying_user`
- `creating_token`
- `configuring_policy`
- `ready`
- `failed`
- `disabled`

**状态转换**：

```text
not_started -> querying_user -> creating_token -> configuring_policy -> ready
querying_user -> failed
creating_token -> failed
configuring_policy -> failed
failed -> querying_user/creating_token/configuring_policy（按部分成功状态补偿）
ready -> disabled
disabled -> ready（管理员重新启用）
```

**验证规则**：
- 同一 `pinvou_user_id + device_binding_id` 同一时间只能有一个查询绑定任务运行；当前实现中两者取值相同。
- 重试必须优先复用 `partial_newapi_user_id` 和 `partial_newapi_token_id`。
- `attempt_count` 达到策略上限后进入 `failed`，等待管理员重试。

## LlmApiPolicy

查询绑定并准备 token 时应用到 New API 的默认策略。

| 字段 | 类型 | 说明 |
|---|---|---|
| `default_quota_limit_tokens` | integer | 默认月度额度 |
| `default_rpm_limit` | integer nullable | 默认每分钟请求限制 |
| `default_model_allowlist` | string[] | 默认可用模型 |
| `default_group` | string nullable | New API 用户组或分组 |
| `enabled` | boolean | 是否允许查询绑定 |
| `updated_at` | datetime | 最近更新时间 |

**验证规则**：
- `enabled=false` 时首次使用应返回“AI 服务暂未开通/不可用”，不得创建 New API 用户。
- `default_model_allowlist` 不为空。
- 额度和限流值必须大于 0。

## LlmUsageSnapshot

Pinvou 展示用额度快照，可内嵌在 `LlmApiBinding` 或单独存储。

| 字段 | 类型 | 说明 |
|---|---|---|
| `pinvou_user_id` | string | 本地设备派生身份 |
| `device_binding_id` | string | 设备绑定标识 |
| `period` | string | 统计周期，例如 `2026-07` |
| `quota_limit_tokens` | integer | 本周期额度 |
| `prompt_tokens` | integer | 累计输入 tokens |
| `completion_tokens` | integer | 累计输出 tokens |
| `total_tokens` | integer | 累计总 tokens |
| `unmetered_call_count` | integer | 缺少 usage 的调用次数 |
| `last_synced_at` | datetime nullable | 最近同步时间 |

**验证规则**：
- `total_tokens >= prompt_tokens + completion_tokens` 或使用 New API 返回的总量口径。
- 缺少 usage 的调用不得伪造 token 数，应增加 `unmetered_call_count`。

## LlmCallRecord

最小化模型调用审计记录，默认不保存完整 prompt/response。

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | string/uuid | 调用记录 ID |
| `pinvou_user_id` | string | 本地设备派生身份 |
| `device_binding_id` | string | 设备绑定标识 |
| `session_id` | string nullable | Pinvou 会话 ID |
| `model` | string | 使用的模型名 |
| `status` | string | `success`、`failed`、`rate_limited`、`quota_exceeded` 等 |
| `prompt_tokens` | integer nullable | 输入 tokens |
| `completion_tokens` | integer nullable | 输出 tokens |
| `total_tokens` | integer nullable | 总 tokens |
| `error_code` | string nullable | 错误分类 |
| `error_message` | string nullable | 脱敏错误摘要 |
| `started_at` | datetime | 开始时间 |
| `finished_at` | datetime nullable | 结束时间 |

**验证规则**：
- 不保存完整 prompt/response。
- `error_message` 必须经过脱敏。
- 成功且带 usage 的调用应更新 `LlmUsageSnapshot`。

## LlmApiAdminCredential

New API 管理接口凭证引用。

| 字段 | 类型 | 说明 |
|---|---|---|
| `base_url` | string | 管理接口基础地址 |
| `credential_ref` | CredentialReference | keyring 中的管理凭证 |
| `credential_state` | CredentialState | 凭证状态 |
| `updated_at` | datetime | 最近更新时间 |

**验证规则**：
- 管理凭证不得进入前端明文。
- 管理凭证缺失时查询绑定返回可恢复失败，不应影响已完成绑定用户继续调用。
