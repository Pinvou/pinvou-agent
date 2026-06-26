# 任务：加密存储大模型 API Key

**输入**：`specs/019-encrypt-api-keys/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/`、`quickstart.md`

**测试策略**：本 feature 涉及敏感凭据落盘与迁移，所有用户故事都包含实现前验证任务。优先用 Rust 单元测试覆盖凭据存储、配置迁移和脱敏逻辑，再用 quickstart 手动验证 UI、重启、连接测试和泄漏扫描。

**组织方式**：任务按用户故事拆分，保证每个故事可独立实现和验收。P1 故事先交付 MVP，P2/P3 在此基础上增量完善。

## Phase 1: 准备（共享基础）

**目的**：确认当前分支、设计边界、现有设置流和验证命令，避免覆盖用户已有改动。

- [X] T001 阅读 `specs/019-encrypt-api-keys/plan.md` 并确认本 feature 只修改 `pinvou3-app` 层、不改写 `DeepSeek-TUI/` 底座
- [X] T002 检查 `E:\Pinvou\pinvou3` 的 git worktree 状态并记录与本 feature 相关的已修改文件
- [X] T003 [P] 对照 `specs/019-encrypt-api-keys/contracts/model-credential-storage.md` 梳理后端命令与凭据存储契约
- [X] T004 [P] 对照 `specs/019-encrypt-api-keys/contracts/settings-ui-credential-state.md` 梳理前端设置页的凭据状态与编辑意图
- [X] T005 [P] 确认 `pinvou3-app/src-tauri/Cargo.toml` 中可复用的系统 keyring 依赖来源和测试运行命令

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立所有故事共享的凭据抽象、序列化模型和脱敏边界。

**⚠️ CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T006 在 `pinvou3-app/src-tauri/Cargo.toml` 中声明或确认 `credential_store` 所需的系统 keyring 相关依赖
- [X] T007 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中定义 `CredentialReference`、`CredentialState`、`CredentialStore` trait 与系统 keyring 实现骨架
- [X] T008 [P] 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中添加测试用内存凭据存储和脱敏错误类型
- [X] T009 在 `pinvou3-app/src-tauri/src/lib.rs` 中注册 `credential_store` 模块并保持现有模块导出风格
- [X] T010 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 中扩展 `SavedModel` 的凭据引用/状态字段并保留旧 `api_key` 字段的反序列化兼容入口
- [X] T011 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 中确保 `UserPrefs::save()` 写出的普通 settings 不序列化完整明文 API Key
- [X] T012 [P] 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中添加 `redact_secret`/`is_secret_like` 等通用脱敏辅助函数

**检查点**：基础模型、凭据抽象、模块注册和 settings 脱敏序列化边界已具备，后续故事可以并行但需避免同文件冲突。

---

## Phase 3: 用户故事 1 - 新配置的 API Key 不再明文落盘 (Priority: P1) MVP

**目标**：用户保存新的大模型 API Key 后，应用仍可正常调用模型，但 `~/.pinvou3/settings.json` 不再包含完整明文 Key。

**独立测试**：在干净环境新增一个带测试 Key 的模型配置，保存并重启；模型请求可使用该 Key，同时 settings 文件与后端返回给 UI 的配置中没有完整 Key。

### 测试 / 验证

- [X] T013 [P] [US1] 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中添加保存、读取、删除凭据且不在错误中暴露 Secret 的单元测试
- [X] T014 [P] [US1] 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 中添加新模型保存后序列化 JSON 不包含完整 `api_key` 的单元测试
- [X] T015 [P] [US1] 在 `pinvou3-app/src-tauri/src/commands.rs` 中添加 `save_model`/`list_models` 返回脱敏凭据状态的命令级测试或测试辅助

### 实现

- [X] T016 [US1] 在 `pinvou3-app/src-tauri/src/commands.rs` 中改造 `save_model`，将新输入的 API Key 写入 `CredentialStore` 并只保存凭据引用/状态
- [X] T017 [US1] 在 `pinvou3-app/src-tauri/src/commands.rs` 中改造 `update_settings` 和 `save_settings_and_restart`，统一剥离前端提交中的完整明文 API Key
- [X] T018 [US1] 在 `pinvou3-app/src-tauri/src/bridge/mod.rs` 中改造 `Pinvou3Bridge::api_key()`，按 `DEEPSEEK_API_KEY`、受保护凭据、本地默认值的顺序解析运行时 Key
- [X] T019 [US1] 在 `pinvou3-app/src-tauri/src/commands.rs` 中改造 `get_effective_model_config` 和 `list_models`，返回 `credential_state` 而不是完整 API Key
- [X] T020 [US1] 在 `pinvou3-app/src-tauri/src/harness.rs` 中确认子进程环境只接收运行时解析后的 Key 且不写入 settings
- [X] T021 [US1] 在 `pinvou3-app/src-tauri/src/pinvou_review.rs` 中确认审查请求继续通过 bridge 解析 Key 且不记录完整 Key

**检查点**：US1 可独立演示：新增 Key、重启、请求可用、settings 无完整明文 Key。

---

## Phase 4: 用户故事 2 - 已有明文 Key 可平滑迁移 (Priority: P1)

**目标**：旧版本 settings 中的明文 API Key 在首次加载/保存时自动迁移到受保护存储，用户无需重新配置。

**独立测试**：准备包含 `advanced.saved_models[*].api_key` 或 `advanced.custom_api_key` 的旧 settings，启动新版本后确认 Key 可用、settings 被脱敏、重复启动不重复迁移。

### 测试 / 验证

- [X] T022 [P] [US2] 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 中添加旧 `saved_models[*].api_key` 迁移为凭据引用的单元测试
- [X] T023 [P] [US2] 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 中添加旧 `advanced.custom_api_key` 迁移到默认模型凭据的单元测试
- [X] T024 [P] [US2] 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 中添加迁移幂等性测试，重复加载不会重新写入明文 Key

### 实现

- [X] T025 [US2] 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 中实现 `UserPrefs::load()` 的旧明文 Key 检测与迁移调度
- [X] T026 [US2] 在 `pinvou3-app/src-tauri/src/bridge/prefs.rs` 中实现迁移成功后的 settings 脱敏回写和 `CredentialMigrationResult` 记录
- [X] T027 [US2] 在 `pinvou3-app/src-tauri/src/commands.rs` 中让迁移后的 `get_settings` 返回安全凭据状态并避免把旧明文 Key 发给前端

**检查点**：US2 可独立演示：旧 settings 自动迁移、无需重新配置、重复重启无明文回写。

---

## Phase 5: 用户故事 3 - 用户可安全查看、替换和删除 Key (Priority: P2)

**目标**：设置页默认只显示安全状态；用户可明确替换或删除 Key，保存后旧 Key 不再可用。

**独立测试**：打开设置页看不到完整 Key；替换 Key 后模型使用新 Key；删除 Key 后请求提示缺少凭据且旧 Key 不再被使用。

### 测试 / 验证

- [X] T028 [P] [US3] 在 `pinvou3-app/src/index.html` 中添加或记录设置页手动验证点：已配置、未配置、环境变量覆盖、不可用状态均不显示完整 Key
- [X] T029 [P] [US3] 在 `pinvou3-app/src-tauri/src/commands.rs` 中添加替换 Key 与删除 Key 的命令级测试或测试辅助

### 实现

- [X] T030 [US3] 在 `pinvou3-app/src/index.html` 中改造 `ModelEditor`，使用保留、替换、删除三种明确意图管理 API Key 输入
- [X] T031 [US3] 在 `pinvou3-app/src/tauri-bridge.js` 中调整模型保存 payload，传递凭据编辑意图而不是长期持有完整 Key
- [X] T032 [US3] 在 `pinvou3-app/src-tauri/src/commands.rs` 中实现 Key 替换和删除意图，删除时同步清理 `CredentialStore` 条目
- [X] T033 [US3] 在 `pinvou3-app/src-tauri/src/commands.rs` 中调整 `test_model_connection`，支持使用新输入 Key 或已保存受保护 Key 进行连接测试
- [X] T034 [US3] 在 `pinvou3-app/src/index.html` 中更新设置页状态展示文案，区分未配置、已配置、环境变量覆盖、凭据不可用和需要重新配置

**检查点**：US3 可独立演示：UI 不泄露完整 Key，替换/删除动作清晰且后端行为一致。

---

## Phase 6: 用户故事 4 - 异常情况下不泄露凭据 (Priority: P3)

**目标**：凭据存储不可用、读取失败、保存失败时，应用给出可恢复提示，不把 Key 写回明文、不在日志/错误/UI 中泄露完整 Key。

**独立测试**：模拟凭据存储失败，模型请求和设置页都只显示恢复提示；settings、日志、错误信息中没有完整测试 Key。

### 测试 / 验证

- [X] T035 [P] [US4] 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中添加凭据读取失败、保存失败和删除失败的脱敏错误测试
- [X] T036 [P] [US4] 在 `pinvou3-app/src-tauri/src/commands.rs` 中添加命令错误返回不包含完整 Key 的测试或测试辅助
- [X] T037 [P] [US4] 在 `specs/019-encrypt-api-keys/quickstart.md` 中补充或核对失败路径验证步骤对应的实际命令

### 实现

- [X] T038 [US4] 在 `pinvou3-app/src-tauri/src/credential_store.rs` 中统一凭据错误转换，确保错误 message 只含状态和恢复建议
- [X] T039 [US4] 在 `pinvou3-app/src-tauri/src/bridge/mod.rs` 中处理受保护 Key 不可读场景，返回缺少凭据/需重新配置的安全状态
- [X] T040 [US4] 在 `pinvou3-app/src-tauri/src/commands.rs` 中统一命令错误脱敏，避免 `save_model`、`update_settings`、`test_model_connection` 泄露用户输入 Key
- [X] T041 [US4] 在 `pinvou3-app/src/index.html` 中显示凭据不可用或需重新配置的恢复提示，不展示完整 Key 或异常原文中的敏感片段

**检查点**：US4 可独立演示：失败路径可恢复、无明文回写、无日志/错误/UI 泄露。

---

## Phase 7: 收尾与横切关注点

- [X] T042 [P] 运行 `cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml credential` 并在 `specs/019-encrypt-api-keys/quickstart.md` 对应章节记录结果或偏差
- [X] T043 [P] 运行 `cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml prefs` 并在 `specs/019-encrypt-api-keys/quickstart.md` 对应章节记录结果或偏差
- [X] T044 运行 `cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml` 并修复 `pinvou3-app/src-tauri/` 下与本 feature 相关的编译问题
- [ ] T045 按 `specs/019-encrypt-api-keys/quickstart.md` 执行新 Key 保存、旧 Key 迁移、替换、删除、环境变量覆盖和泄漏扫描验证
- [X] T046 检查 `pinvou3-app/src-tauri/src/bridge/prefs.rs`、`pinvou3-app/src-tauri/src/commands.rs`、`pinvou3-app/src/index.html` 中是否仍有长期保存完整 API Key 的状态或日志输出
- [X] T047 检查 `DeepSeek-TUI/` 未被本 feature 修改，并在实现总结中说明仍复用底座能力而非重造 Engine/Session/Commands

---

## 依赖与执行顺序

- Phase 1 无依赖，先完成以确认边界与现有改动。
- Phase 2 阻塞所有用户故事，必须先完成凭据抽象、settings 序列化边界和模块注册。
- US1 是 MVP，依赖 Phase 2，完成后即可独立验证新增 Key 不明文落盘。
- US2 依赖 Phase 2，可在 US1 后实施；它复用同一凭据模型，重点覆盖旧明文迁移。
- US3 依赖 US1 的保存/读取契约，完成后提供安全的查看、替换、删除体验。
- US4 依赖前面故事的错误边界，最后集中收敛异常路径和泄漏防护。
- Phase 7 在全部故事完成后执行，用于整体验证和回归检查。

## 并行机会

- Phase 1 的 T003、T004、T005 可并行，因为分别阅读不同契约或依赖文件。
- Phase 2 的 T008、T012 可在 T007 的类型草案完成后并行补充测试存储和脱敏辅助。
- US1 的 T013、T014、T015 可并行编写测试，随后串行完成 T016 到 T021 的实现整合。
- US2 的 T022、T023、T024 可并行覆盖不同旧配置输入。
- US3 的 T028 与 T029 可并行准备 UI 和命令验证，T030/T031 前后端 payload 需协调。
- US4 的 T035、T036、T037 可并行覆盖存储、命令和 quickstart 失败路径。
- Phase 7 的 T042、T043 可并行运行，T044 应在测试后统一处理编译问题。

## 实施策略

1. 先完成 Phase 1 和 Phase 2，建立不泄露明文 Key 的基础边界。
2. 交付 US1 作为 MVP：新增 Key 保存到受保护存储，settings 和 UI 返回值不含完整 Key。
3. 继续完成 US2，确保老用户升级时无需重新配置且旧明文被清除。
4. 增量完成 US3，补齐设置页的安全管理体验。
5. 最后完成 US4 和 Phase 7，把失败路径、日志脱敏、quickstart 验证收尾。
