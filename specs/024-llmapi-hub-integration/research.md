# 研究记录：Pinvou 接入 LLM API Hub 中转站

## 决策 1：通过现有 OpenAI-compatible 模型配置路径接入中转站

**Decision**：将 LLM API Hub 作为受控的 OpenAI-compatible 后端接入，最终仍由 DeepSeek-TUI Engine 处理会话、SSE、工具循环和 usage 事件。

**Rationale**：Pinvou 当前已有 `SavedModel`、`bridge::build_dt_config()`、`EnginePool` 和 `test_model_connection()`，可以把 `base_url=https://www.ma-xiao.com/llmapi/v1`、`provider=openai`、`api_key=<New API token>` 注入到底座，而不重写模型调用链。

**Alternatives considered**：
- 直接在 Pinvou 新写 `/chat/completions` 客户端：会重写 DeepSeek-TUI 已有 SSE、Session、ToolRegistry 和错误处理，违反底座优先原则。
- 让前端直连 LLM API Hub：会暴露 New API token，不满足安全要求。

## 决策 2：token 使用 `credential_store` 保存，绑定元数据不保存明文 token

**Decision**：为 LLM API Hub 新增独立凭证命名空间，例如 `pinvou3-llmapi-token` 和 `pinvou3-llmapi-admin`；绑定记录只保存 `CredentialReference`、New API 用户/token ID、额度快照和状态。

**Rationale**：项目已有 `codewhale-secrets` 封装和 `CredentialReference` 模式，支持 OS keyring 优先、文件回退，并已有脱敏测试。沿用该模式可以避免 token 进入 `settings.json`、前端状态或日志。

**Alternatives considered**：
- 在 SQLite 或 JSON 中加密保存 token：需要新增密钥管理和迁移策略，复杂度更高。
- 只存在内存中：重启后无法继续调用，不满足用户体验。

## 决策 3：首次使用采用本地幂等查询绑定状态机

**Decision**：新增“查询绑定任务”状态机，状态包括 `not_started`、`querying_user`、`creating_token`、`configuring_policy`、`ready`、`failed`、`disabled`。同一设备派生身份重复触发时复用已有中间状态并补偿完成后续步骤。

**Rationale**：用户要求首次使用不再由 Pinvou 自动创建后台账户，只查询是否存在后台账户。New API 用户查询成功但 token 创建失败、网络超时、应用重启等场景仍可能造成部分成功；状态机能防止重复创建多个 token，并避免创建 New API 用户。

**Alternatives considered**：
- 简单“查不到绑定就创建用户”：违背当前要求，且部分成功后容易产生重复用户或孤儿 token。
- 完全依赖 New API 幂等：资料未证明 New API 管理接口具备以设备派生身份为幂等键的能力。

## 决策 4：隔离 New API 管理接口到 adapter

**Decision**：新增 `llmapi_hub::adapter`，只向业务层暴露 `lookup_user`、`create_token`、`configure_policy`、`disable_token`、`get_usage_or_quota` 等语义方法；具体 HTTP 路径、字段和鉴权头只放在 adapter 内。

**Rationale**：中转站资料确认 New API v1.0.0-rc.20、OpenAI 兼容入口和后台管理能力，但未把管理接口细节固化到 Pinvou 规格中。adapter 隔离可降低 New API 升级和接口差异的影响。

**Alternatives considered**：
- 在 commands 或 bridge 中直接写 HTTP 请求：会把外部接口细节扩散到多个模块。
- 直接操作 New API PostgreSQL 数据库：耦合 New API 内部表结构，升级风险高，也绕过业务校验。

## 决策 5：Pinvou 额度展示以本地 usage 快照为主，New API 拒绝为准

**Decision**：每次模型响应包含 `usage` 时更新 Pinvou 侧 `quota_used`、`remaining` 和 `last_usage_at`；当 New API 返回额度不足或限流时，以 New API 结果为最终保护并同步状态。

**Rationale**：资料建议 Pinvou 展示额度快照，New API 负责真实限额、真实限流和真实拒绝。这样既保证用户体验，又避免 Pinvou 自己成为计费事实来源。

**Alternatives considered**：
- 每次打开额度页都实时查询 New API：更准确但依赖管理接口稳定性和额外延迟。
- 只展示 New API 错误、不展示额度：用户不可预期，不能满足规格。

## 决策 6：Pinvou 用户与设备绑定作为前置集成点

**Decision**：实现层提供身份解析入口，返回由当前设备 BIOS SN 派生的 `device_binding_id`，并直接使用该值作为 `pinvou_user_id`；只要 Windows 设备 BIOS SN 可读取且有效，即可用于查询绑定或继续使用中转站。

**Rationale**：当前阶段不再依赖独立 Pinvou 登录账号，使用设备绑定标识作为本地身份可以减少账号前置依赖，同时避免使用 Windows 用户名、机器名或 session ID 这类不稳定身份。

**Alternatives considered**：
- 只使用 Pinvou 用户 ID：当前阶段没有稳定的 Pinvou 登录账号来源，会引入额外前置依赖。
- 使用本机用户名或机器名：会把额度绑定到操作系统账号而非 Pinvou 用户，且稳定性和安全语义不足。
- 使用独立 Pinvou 用户 ID + 设备 ID：语义更完整，但需要先接入可靠 Pinvou 账号体系，本阶段暂不采用。
- 每个本地安装共享一个 token：无法做到用户级额度、限流和日志隔离。

## 决策 7：管理员能力最小化为状态、禁用和重试

**Decision**：本 feature 的管理员能力先覆盖用户绑定状态、失败原因、额度快照、启用/禁用、重试查询绑定；额度策略配置仅做默认策略配置和后续扩展点。

**Rationale**：查询绑定必须可恢复，但不应在一期内重做完整 New API 管理后台。真实用户、token、额度和日志仍由 New API 保持事实来源。

**Alternatives considered**：
- 在 Pinvou 中完整复刻 New API 管理后台：范围过大，违反小步变更。
- 不做管理员入口：查询绑定失败不可观测，不满足规格。

## 决策 8：本 feature 仅实现 Windows 桌面端

**Decision**：LLM API Hub 查询绑定、BIOS SN 设备绑定、模型调用接入、额度展示和管理员治理仅在 Windows 系统上实现；Linux 和其他非 Windows 平台本次不实现该功能。

**Rationale**：当前身份绑定依赖 Windows 设备 BIOS SN，用户明确要求本功能只在 Windows 系统上实现；收窄平台范围可以避免为 Linux 引入不一致的设备标识语义和额外兼容代码。

**Alternatives considered**：
- 同步实现 Linux：需要重新定义 Linux 设备绑定来源，容易偏离 BIOS SN 绑定口径并扩大本次交付范围。
- 在非 Windows 上复用机器名或本地 device_id：无法满足用户确认的 BIOS SN 绑定要求，安全语义不足。
