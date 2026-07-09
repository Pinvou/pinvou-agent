# 任务：Pinvou 接入 LLM API Hub 中转站

**输入**：`specs/024-llmapi-hub-integration/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/`

**测试**：本 feature 仅在 Windows 系统上实现，需要覆盖 Windows 设备绑定校验、查询绑定幂等、凭证脱敏、usage 累计和前端 smoke；测试任务放在对应实现任务之前。

**组织方式**：任务按用户故事分组，保证每个故事可独立实现和验证。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：用户故事阶段任务必须标记为 `[US1]`、`[US2]`、`[US3]` 或 `[US4]`。
- 描述中必须包含精确文件路径或验证命令。

## Phase 1: 准备（共享基础）

**目的**：确认当前实现入口、依赖和验证命令，避免改动 DeepSeek-TUI 底座。

- [X] T001 阅读并确认 `specs/024-llmapi-hub-integration/plan.md`、`specs/024-llmapi-hub-integration/spec.md`、`specs/024-llmapi-hub-integration/contracts/tauri-commands.md` 的实现边界
- [X] T002 检查当前 worktree 和相关入口文件 `pinvou3-app/src-tauri/src/lib.rs`、`pinvou3-app/src-tauri/src/commands.rs`、`pinvou3-app/src-tauri/src/bridge/mod.rs`，确认不覆盖用户未提交改动
- [X] T003 [P] 在 `pinvou3-app/src-tauri/Cargo.toml` 确认或补齐 `reqwest`、`serde`、`serde_json`、`tokio`、`rusqlite`、`sha2` 等 LLM API Hub 所需依赖
- [X] T004 [P] 在 `pinvou3-app/src-tauri/src/llmapi_hub/mod.rs` 建立模块骨架并声明 `adapter`、`identity`、`models`、`provisioning`、`store`、`usage`

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：完成所有用户故事共用的身份、存储、凭证和外部适配器基础。

**⚠️ CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T005 在 `pinvou3-app/src-tauri/src/lib.rs` 注册 `llmapi_hub` 模块但不改动 `DeepSeek-TUI/` 底座代码
- [X] T006 [P] 在 `pinvou3-app/src-tauri/src/llmapi_hub/models.rs` 定义 `LlmApiBinding`、`ProvisioningTask`、`LlmApiPolicy`、`LlmUsageSnapshot`、`LlmCallRecord`、`ProvisioningStatus`、`LlmApiErrorCode`
- [X] T007 [P] 在 `pinvou3-app/src-tauri/src/credential_store.rs` 增加 LLM API Hub token 和 New API admin credential 的 `CredentialReference` 构造方法
- [X] T008 [P] 在 `pinvou3-app/src-tauri/src/os/interface/system.rs` 增加仅供 Windows 实现使用的 BIOS SN 读取接口声明或在 Windows cfg 下暴露等价能力
- [X] T009 在 `pinvou3-app/src-tauri/src/os/windows/windows_system.rs` 实现 Windows BIOS SN 读取、规范化和错误脱敏
- [X] T010 在 `pinvou3-app/src-tauri/src/llmapi_hub/identity.rs` 为非 Windows 平台增加 `unsupported_platform` 早返回，避免修改 `pinvou3-app/src-tauri/src/os/linux/linux_system.rs`
- [X] T011 [P] 在 `pinvou3-app/src-tauri/src/llmapi_hub/identity.rs` 实现由 BIOS SN 派生 `device_binding_id`，并直接作为 `pinvou_user_id` 的解析结构，禁止使用 Windows 用户名、机器名或本地 session ID
- [X] T012 [P] 在 `pinvou3-app/src-tauri/src/llmapi_hub/store.rs` 实现本地绑定元数据读写层，确保 token 和 BIOS SN 明文不进入 `settings.json` 或普通日志
- [X] T013 [P] 在 `pinvou3-app/src-tauri/src/llmapi_hub/adapter.rs` 定义 New API 管理适配器 trait、请求/响应类型和错误分类映射
- [X] T014 在 `pinvou3-app/src-tauri/src/llmapi_hub/adapter.rs` 实现 `https://www.ma-xiao.com/llmapi` 管理接口 HTTP client 的配置读取和鉴权头注入
- [X] T015 在 `pinvou3-app/src-tauri/src/llmapi_hub/models.rs` 添加单元测试覆盖序列化结果不包含 token 明文和 BIOS SN 明文
- [X] T016 在 `pinvou3-app/src-tauri/src/llmapi_hub/identity.rs` 添加单元测试覆盖 BIOS SN 规范化、派生 `device_binding_id`、读取失败和绑定不一致错误
- [X] T017 在 `pinvou3-app/src-tauri/src/llmapi_hub/store.rs` 添加单元测试覆盖 `pinvou_user_id + device_binding_id` 组合唯一和绑定状态迁移

**检查点**：LLM API Hub 模块、设备绑定接口、凭证引用、存储模型和 adapter 骨架完成，可开始按用户故事实施。

---

## Phase 3: 用户故事 1 - 首次使用时查询并绑定中转站资源 (Priority: P1) 🎯 MVP

**目标**：在 Windows 系统上，用户首次使用 AI 助手时，后端只查询是否存在对应 New API 后台用户；存在时继续准备 token 并保存加密绑定，不存在时失败提示且不创建后台用户。

**独立测试**：使用无本地绑定但后台已存在的测试身份调用 `ensure_llmapi_binding`，验证查询用户、创建或取得 token、保存凭证、状态为 `ready`；后台不存在时验证不创建用户/token，且前端和日志不出现 token 或 BIOS SN 明文。

### 测试 / 验证

- [X] T018 [P] [US1] 在 `pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs` 添加查询绑定成功路径单元测试，使用 mock adapter 和 memory credential store
- [X] T019 [P] [US1] 在 `pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs` 添加 lookup_user 成功但 create_token 失败后的补偿重试单元测试
- [X] T020 [P] [US1] 在 `pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs` 添加设备未绑定、BIOS SN 读取失败、绑定不一致和非 Windows 平台时不得调用 adapter 的单元测试

### 实现

- [X] T021 [US1] 在 `pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs` 实现 `ensure_binding` 状态机，覆盖 `not_started`、`querying_user`、`creating_token`、`configuring_policy`、`ready`、`failed`、`disabled`
- [X] T022 [US1] 在 `pinvou3-app/src-tauri/src/llmapi_hub/adapter.rs` 实现 `lookup_user`、`create_token`、`configure_policy`，幂等键使用设备派生的 `pinvou_user_id + device_binding_id`
- [X] T023 [US1] 在 `pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs` 将 New API token 明文立即写入 `credential_store`，并只保存 `token_credential_ref`
- [X] T024 [US1] 在 `pinvou3-app/src-tauri/src/commands.rs` 增加 `ensure_llmapi_binding` Tauri 命令，返回 `ready`、`created`、`retryable` 和脱敏 message
- [X] T025 [US1] 在 `pinvou3-app/src-tauri/src/commands.rs` 增加 `get_llmapi_status` Tauri 命令的服务状态基础返回，不包含 token 和 BIOS SN 明文
- [X] T026 [US1] 在 `pinvou3-app/src-tauri/src/lib.rs` 将 `ensure_llmapi_binding` 和 `get_llmapi_status` 注册到 `tauri::generate_handler!`
- [X] T027 [US1] 在 `pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs` 统一映射 `unsupported_platform`、`device_not_bound`、`device_binding_failed`、`admin_credential_missing`、`provisioning_failed`、`service_unreachable`、`rate_limited`、`service_disabled`
- [X] T028 [US1] 运行 `cargo test -p pinvou3-tauri --lib llmapi_hub` 并记录失败项或修复结果

**检查点**：US1 可在 Windows 上独立演示首次查询并绑定；后台账户不存在、未绑定设备和非 Windows 平台不会创建 New API 用户或 token。

---

## Phase 4: 用户故事 2 - 使用 Pinvou 账号调用云端大模型 (Priority: P1)

**目标**：在 Windows 系统上，已开通用户发送 AI 消息时，Pinvou 后端取出对应 token，通过现有 OpenAI-compatible 路径调用 `https://www.ma-xiao.com/llmapi/v1`。

**独立测试**：使用已 ready 的绑定发送一条 AI 助手消息，验证 `chat` 进入 DeepSeek-TUI Engine 前使用 LLM API Hub 的 base_url、model 和 keyring token；绑定失败时不进入模型调用链。

### 测试 / 验证

- [ ] T029 [P] [US2] 在 `pinvou3-app/src-tauri/src/bridge/mod.rs` 添加测试覆盖 LLM API Hub 模型配置注入后 provider 为 `openai` 且 base_url 为 `https://www.ma-xiao.com/llmapi/v1`
- [ ] T030 [P] [US2] 在 `pinvou3-app/src-tauri/src/commands.rs` 添加 chat 前置校验测试，覆盖绑定失败时不调用 `EnginePool`

### 实现

- [X] T031 [US2] 在 `pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs` 增加读取 ready 绑定并返回 OpenAI-compatible 模型配置的方法
- [X] T032 [US2] 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 或 `pinvou3-app/src-tauri/src/bridge/mod.rs` 接入 LLM API Hub 的临时 `SavedModel`/token 配置，复用现有 `SavedModel` 和 `EnginePool` 路径
- [X] T033 [US2] 在 `pinvou3-app/src-tauri/src/commands.rs` 的 `chat` 命令发送到 `EnginePool` 前调用 LLM API Hub 绑定校验和模型配置注入
- [X] T034 [US2] 在 `pinvou3-app/src-tauri/src/commands.rs` 将中转站鉴权失败、额度不足、限流、服务不可达、上游错误映射为用户友好错误，不输出敏感凭证
- [ ] T035 [US2] 在 `pinvou3-app/src/tauri-bridge.js` 处理 `chat` 返回的 LLM API Hub 错误分类并展示非敏感提示，非 Windows 平台展示暂不支持
- [ ] T036 [US2] 运行 `cargo test -p pinvou3-tauri --lib bridge commands llmapi_hub` 并记录失败项或修复结果

**检查点**：US2 可独立演示已开通用户通过中转站完成云端模型对话。

---

## Phase 5: 用户故事 3 - 用户查看 AI 额度和用量 (Priority: P2)

**目标**：用户可查看 Pinvou 侧额度快照、已使用量、剩余量和最近更新时间；模型调用成功后自动更新 usage。

**独立测试**：模拟模型响应带 usage，调用完成后 `get_llmapi_status` 返回更新后的本月额度、已用和剩余；缺少 usage 的调用增加 `unmetered_call_count`。

### 测试 / 验证

- [X] T037 [P] [US3] 在 `pinvou3-app/src-tauri/src/llmapi_hub/usage.rs` 添加 usage 累计、剩余额度计算和缺失 usage 计数单元测试
- [ ] T038 [P] [US3] 在 `pinvou3-app/src-tauri/src/commands.rs` 添加 `get_llmapi_status` 额度字段序列化测试

### 实现

- [X] T039 [US3] 在 `pinvou3-app/src-tauri/src/llmapi_hub/usage.rs` 实现 `record_usage`、`record_unmetered_call`、`quota_snapshot` 方法
- [ ] T040 [US3] 在 `pinvou3-app/src-tauri/src/commands.rs` 接收或订阅模型调用 usage 信息并更新 `LlmUsageSnapshot`
- [X] T041 [US3] 在 `pinvou3-app/src-tauri/src/commands.rs` 扩展 `get_llmapi_status` 返回 `quota.period`、`limit_tokens`、`used_tokens`、`remaining_tokens`、`unmetered_call_count`、`last_synced_at`
- [X] T042 [US3] 在 `pinvou3-app/src/tauri-bridge.js` 增加 LLM API Hub 状态加载方法并缓存 quota 状态
- [ ] T043 [US3] 在 `pinvou3-app/src/index.html` 的 Windows 可用设置或账户区域增加 AI 额度快照展示，避免展示 New API 账号或 token
- [ ] T044 [US3] 运行 `cargo test -p pinvou3-tauri --lib llmapi_hub::usage commands` 并按 `specs/024-llmapi-hub-integration/quickstart.md` 执行额度展示验证

**检查点**：US3 可独立演示额度快照随模型调用更新。

---

## Phase 6: 用户故事 4 - 管理员管理中转站绑定策略和用户状态 (Priority: P2)

**目标**：管理员可查看用户绑定/设备绑定/额度/失败状态，并可重试查询绑定或禁用用户。

**独立测试**：管理员查询 overview 可定位指定用户状态；对失败用户重试后可继续完成绑定；禁用后该用户无法通过中转站调用模型。

### 测试 / 验证

- [ ] T045 [P] [US4] 在 `pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs` 添加管理员重试失败查询绑定的单元测试
- [X] T046 [P] [US4] 在 `pinvou3-app/src-tauri/src/llmapi_hub/store.rs` 添加启用/禁用状态持久化和查询过滤单元测试

### 实现

- [X] T047 [US4] 在 `pinvou3-app/src-tauri/src/commands.rs` 实现 `retry_llmapi_provisioning`，参数包含 `pinvouUserId` 和 `deviceBindingId`
- [X] T048 [US4] 在 `pinvou3-app/src-tauri/src/commands.rs` 实现 `set_llmapi_user_enabled`，本地禁用优先生效并尝试调用 adapter 禁用远端 token
- [X] T049 [US4] 在 `pinvou3-app/src-tauri/src/commands.rs` 实现 `get_llmapi_admin_overview`，返回设备绑定状态、服务状态、额度快照和脱敏失败原因
- [X] T050 [US4] 在 `pinvou3-app/src-tauri/src/lib.rs` 注册 `retry_llmapi_provisioning`、`set_llmapi_user_enabled`、`get_llmapi_admin_overview`
- [X] T051 [US4] 在 `pinvou3-app/src/tauri-bridge.js` 增加管理员 overview、重试和禁用调用封装
- [ ] T052 [US4] 在 `pinvou3-app/src/index.html` 增加 Windows 可用的管理员中转站状态入口，展示服务状态、设备绑定状态、额度和最近失败原因
- [ ] T053 [US4] 运行 `cargo test -p pinvou3-tauri --lib llmapi_hub commands` 并按 `specs/024-llmapi-hub-integration/quickstart.md` 执行禁用用户验证

**检查点**：US4 可独立演示管理员定位、重试和禁用中转站用户。

---

## Phase 7: 收尾与横切关注点

- [X] T054 [P] 更新 `specs/024-llmapi-hub-integration/quickstart.md`，记录实际命令、测试账号准备方式和未覆盖项
- [ ] T055 [P] 更新 `docs/` 下相关中文说明，补充 Windows-only LLM API Hub 中转站、设备绑定和敏感信息边界
- [ ] T056 运行 `cargo test -p pinvou3-tauri --lib` 并记录结果
- [X] T057 运行 `rg -n "sk-|Bearer [A-Za-z0-9]|DeepSeek API Key|New API token|SerialNumber|BIOS SN" "$env:USERPROFILE\\.pinvou3" pinvou3-app/src-tauri/src pinvou3-app/src`，确认无真实 token 或 BIOS SN 明文泄漏
- [ ] T058 运行 `npm run build` 或项目既有前端构建命令于 `pinvou3-app/package.json`，确认前端新增状态展示可编译
- [X] T059 检查 `DeepSeek-TUI/` 没有因本 feature 产生非必要改动，并在 `specs/024-llmapi-hub-integration/tasks.md` 记录如有例外
- [ ] T060 按 `specs/024-llmapi-hub-integration/quickstart.md` 在 Windows 上完成首次查询绑定、设备绑定失败、幂等补偿、额度展示、禁用用户和中转站入口验证，并在非 Windows 上验证 `unsupported_platform`

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事。
- US1 是 Windows MVP，并阻塞 US2、US3、US4 的完整联调。
- US2 依赖 US1 的 ready 绑定和 token 凭证。
- US3 依赖 US2 的模型调用 usage 入口，但可先用 mock usage 独立开发。
- US4 依赖 US1 的绑定状态和 store，但 UI 与命令封装可在 US3 前并行推进。

## 并行机会

- T003、T004 可并行。
- T006、T007、T008、T011、T012、T013 可在模块骨架后并行，但 T008/T009 仅面向 Windows 设备绑定实现。
- US1 中 T018、T019、T020 可并行编写测试。
- US2 中 T029、T030 可并行。
- US3 中 T037、T038 可并行。
- US4 中 T045、T046 可并行。
- 收尾阶段 T054、T055 可并行。

## 并行执行示例

```text
US1 测试并行：
- T018 在 pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs 添加成功路径测试
- T019 在 pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs 添加补偿重试测试
- T020 在 pinvou3-app/src-tauri/src/llmapi_hub/provisioning.rs 添加设备绑定失败测试
```

```text
US3 前后端并行：
- T039 在 pinvou3-app/src-tauri/src/llmapi_hub/usage.rs 实现 usage 快照
- T042 在 pinvou3-app/src/tauri-bridge.js 增加状态加载封装
- T043 在 pinvou3-app/src/index.html 增加额度展示 UI
```

## 实施策略

1. 先完成 Phase 1 和 Phase 2，确保 Windows 身份、设备绑定、store、adapter 和凭证边界稳定。
2. 先交付 US1 作为 MVP：首次使用只查询既有后台账户并完成本地绑定，且敏感信息不泄漏。
3. 再交付 US2，把现有 DeepSeek-TUI EnginePool 路径接到 LLM API Hub，不重写模型调用链。
4. 后续增量交付 US3 额度展示和 US4 管理员治理能力。
5. 每完成一个用户故事即运行对应测试和 quickstart 验证，不把风险堆到最后。
