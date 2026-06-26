# 数据模型：加密存储大模型 API Key

## CredentialReference

**含义**：普通配置文件中保存的非敏感凭据引用，用于定位系统凭据存储中的真实 Key。

**字段**：
- `service`：凭据命名空间，例如 `pinvou3-model-api-key`。
- `account`：凭据账号/条目名，建议由模型配置 id 派生，例如 `model:<model_id>`。
- `version`：引用格式版本，初始为 `1`，用于未来迁移。

**校验规则**：
- 不得包含完整 API Key。
- `account` 必须稳定，模型改名不应导致凭据丢失。
- 删除模型时必须同步删除对应凭据，除非该凭据仍被其它模型引用。

## CredentialState

**含义**：设置页和后端流程可安全展示的凭据状态。

**取值**：
- `missing`：没有保存 Key。
- `configured`：已保存受保护 Key。
- `env_override`：当前由环境变量覆盖。
- `needs_migration`：检测到旧明文 Key，尚未完成迁移。
- `unavailable`：系统凭据存储不可用或读取失败，需要用户处理。

**状态转换**：
- `missing` + 用户保存新 Key -> `configured`
- `configured` + 用户删除 Key -> `missing`
- `configured` + 环境变量存在 -> `env_override`
- 旧明文配置启动 -> `needs_migration` -> 迁移成功 -> `configured`
- 读取失败 -> `unavailable`

## SavedModelCredentialBinding

**含义**：模型配置与凭据引用之间的关系。它替代 `SavedModel.api_key` 的明文持久化角色。

**字段**：
- `model_id`：对应 `SavedModel.id`。
- `credential_ref`：可选 `CredentialReference`。
- `credential_state`：当前安全状态。
- `has_secret`：布尔值，供前端快速展示“已配置”。

**关系**：
- 一个 `SavedModel` 最多绑定一个模型 API Key。
- 多个模型可以共享相同 base_url/model，但凭据默认按 model id 独立保存，避免误删和意外共享。

## CredentialMigrationResult

**含义**：旧明文 Key 迁移过程的结果。

**字段**：
- `migrated_count`：成功迁移的 Key 数量。
- `skipped_count`：空 Key、本地模型无 Key 等无需迁移项数量。
- `failed_model_ids`：迁移失败的模型 id 列表，不包含 Key 值。
- `settings_sanitized`：是否已将普通配置中的明文移除。

**约束**：
- 失败结果不得包含完整 Key。
- 成功迁移后必须让后续 `UserPrefs::save()` 写出脱敏配置。

## EffectiveModelCredential

**含义**：运行时构造模型请求时使用的真实凭据解析结果，仅保留在内存中。

**字段**：
- `source`：`env`、`protected_store`、`local_default`、`missing`。
- `api_key`：可选真实 Key，仅在后端内存中使用，不返回给前端默认状态。
- `state`：对应 `CredentialState`。

**约束**：
- `api_key` 不得写入日志、错误信息、诊断包或前端持久状态。
- `DEEPSEEK_API_KEY` 存在时，`source = env` 且优先使用该值。
