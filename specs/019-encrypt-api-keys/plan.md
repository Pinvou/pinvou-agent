# 实施计划：加密存储大模型 API Key

**分支**：`019-encrypt-api-keys` | **日期**：2026-06-26 | **规格**：`specs/019-encrypt-api-keys/spec.md`

**输入**：来自 `specs/019-encrypt-api-keys/spec.md` 的功能规格

## 概要

本功能解决当前大模型 API Key 以明文写入 `~/.pinvou3/settings.json` 的风险。实施路径限定在 `pinvou3-app` 层：保存模型配置时将 Key 写入当前系统用户的受保护凭据存储，普通配置文件只保留非敏感模型信息和凭据引用/状态；读取模型配置、构造 DeepSeek-TUI `DtConfig`、视觉模型配置、Pinvou 审查和 harness 子进程环境时按引用解析真实 Key；旧配置中的明文 Key 在加载/保存路径中自动迁移并从普通配置中移除。

方案不修改 DeepSeek-TUI 的 Engine、Session、Commands、MCP、Hooks 或流式底座。实现只把 pinvou3 自己维护的模型配置从“明文字段”改为“受保护凭据引用 + 运行时解析”，并保留环境变量 `DEEPSEEK_API_KEY` 的最高优先级。

## 技术上下文

**语言/版本**：Rust 1.88（Tauri 后端）、JavaScript/React 单文件前端、Tauri 2。

**主要依赖**：Tauri 2、现有 `pinvou3-app` bridge/settings 模块、系统凭据存储能力；优先使用 Rust `keyring` 生态或 DeepSeek-TUI 已引入的 `codewhale-secrets` 中的系统 keyring 封装，避免自研加密密钥管理。

**存储**：`~/.pinvou3/settings.json` 继续保存非敏感偏好、模型名称、preset、model、base_url、active_model_id；大模型 API Key 保存到系统用户级凭据存储。settings 中的 `SavedModel.api_key` 和 `advanced.custom_api_key` 迁移为不含完整明文的状态/引用。

**测试**：`cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml credential`、`cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml prefs`、`cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml`；按 `quickstart.md` 手动验证旧明文迁移、保存/替换/删除、设置页掩码展示、模型连接、日志无泄露。

**目标平台**：跨平台桌面应用；Windows 使用系统凭据管理器，macOS 使用系统钥匙串，Linux 使用 Secret Service。若目标系统凭据存储不可用，应用必须提示重新配置或修复环境，不自动把 Key 明文持久化到 settings。

**项目类型**：desktop-app，Tauri 后端 + WebView 设置界面。

**性能目标**：设置页加载和模型请求前解析凭据不应产生用户可感知卡顿；已保存 Key 的用户重启后 30 秒内可完成一次模型连接验证或请求尝试。

**约束**：不重写 DeepSeek-TUI 底座；不把 Key 写回明文配置；不把完整 Key 输出到日志、错误提示、诊断数据或 UI 状态；不扩大到搜索 API Key、反馈服务凭据或 MCP marketplace 配置字段；保留环境变量覆盖能力。

**规模/范围**：涉及 `pinvou3-app/src-tauri/src/bridge/prefs.rs`、`pinvou3-app/src-tauri/src/bridge/mod.rs`、`pinvou3-app/src-tauri/src/commands.rs`、`pinvou3-app/src-tauri/src/harness.rs`、`pinvou3-app/src-tauri/src/pinvou_review.rs`、`pinvou3-app/src/index.html`、`pinvou3-app/src/tauri-bridge.js` 以及新增 app 层凭据存储模块。

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。计划、研究、数据模型、契约、quickstart 和后续任务均使用中文描述，保留必要英文 crate/API/命令名。
- **DeepSeek-TUI 底座优先**：PASS。本计划只在 `pinvou3-app` 层改变 pinvou3 设置与凭据解析，不重写 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle 或 Compaction。
- **本地算力与数据边界**：PASS。功能只改变本地凭据落盘方式；不会新增外发数据，不改变用户配置的模型 endpoint 行为；明确 Key 生命周期和落盘边界。
- **小步高质量变更**：PASS。改动聚焦模型 API Key，排除搜索、反馈、MCP 等其它凭据；保留现有设置结构和环境变量覆盖策略。
- **可测试性与可验证交付**：PASS。定义了迁移、保存、删除、读取失败、日志脱敏和 UI 掩码的单测/手动验证路径。
- **可维护性与长期演进**：PASS。通过 Spec Kit artifacts 记录存储模型、契约和验证步骤；凭据存储封装在 app 层独立模块，便于后续扩展到其它凭据类型。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/019-encrypt-api-keys/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── model-credential-storage.md
│   └── settings-ui-credential-state.md
└── checklists/
    └── requirements.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src/
│   ├── index.html              # 设置页模型管理 UI：Key 掩码、替换、删除、状态提示
│   └── tauri-bridge.js         # 设置/模型命令桥接，避免把完整 Key 长期留在状态快照
└── src-tauri/
    ├── Cargo.toml              # 如需直接依赖 keyring/codewhale-secrets，在此声明
    └── src/
        ├── credential_store.rs # 新增：app 层凭据读写/删除/迁移封装
        ├── commands.rs         # get/list/save/update/test 命令接入凭据状态
        ├── bridge/
        │   ├── prefs.rs        # UserPrefs/SavedModel 脱敏持久化与旧明文迁移
        │   └── mod.rs          # api_key() 运行时解析受保护凭据
        ├── harness.rs          # 子进程环境注入继续用运行时解析后的 Key
        └── pinvou_review.rs    # 审查请求继续用 bridge.api_key()，不得记录完整 Key
```

**结构决策**：凭据存储属于 pinvou3 桌面 app 的用户配置边界，应放在 `pinvou3-app/src-tauri/src/credential_store.rs`，由 `prefs`、`commands` 和 `bridge` 调用。DeepSeek-TUI 已有 secret/keyring 能力可作为依赖或实现参考，但本 feature 不把 pinvou3 私有设置迁移逻辑下沉到 fork，避免污染底座。

## Phase 0：研究产物

生成 `specs/019-encrypt-api-keys/research.md`，关键决策：

- 使用系统凭据存储保存大模型 API Key，不自研对称加密密钥。
- `settings.json` 保存凭据引用/状态，禁止保存完整明文 Key。
- 旧版明文 Key 采用启动/加载时自动迁移，迁移成功后清空明文字段。
- `get_effective_model_config` / `list_models` 等返回给前端的数据默认脱敏，仅在用户新输入后保存。
- 环境变量 `DEEPSEEK_API_KEY` 继续最高优先级，且不写入凭据存储。

## Phase 1：设计产物

生成以下设计产物：

- `specs/019-encrypt-api-keys/data-model.md`：定义 `CredentialReference`、`CredentialState`、`SavedModelCredentialBinding`、`CredentialMigrationResult`、`EffectiveModelCredential`。
- `specs/019-encrypt-api-keys/contracts/model-credential-storage.md`：定义后端保存、读取、迁移、删除和失败处理契约。
- `specs/019-encrypt-api-keys/contracts/settings-ui-credential-state.md`：定义设置页展示、编辑、替换、删除和脱敏契约。
- `specs/019-encrypt-api-keys/quickstart.md`：定义实现后的单测、静态检查和手动验证步骤。
- `AGENTS.md`：当前 Spec Kit 引用更新为 `specs/019-encrypt-api-keys/plan.md`。

## Phase 1 复查

- **中文文档优先**：PASS。新增设计文档均为中文。
- **DeepSeek-TUI 底座优先**：PASS。设计未要求修改底座核心能力；如复用 `codewhale-secrets`，仅作为依赖，不改变底座行为。
- **本地算力与数据边界**：PASS。Key 存储和读取保持本机当前用户边界；不新增网络传输。
- **小步高质量变更**：PASS。范围限定模型 API Key；后续任务可按迁移、命令、UI、测试拆分。
- **可测试性与可验证交付**：PASS。契约和 quickstart 覆盖 FR-001 到 FR-010。
- **可维护性与长期演进**：PASS。凭据引用和状态模型为后续扩展其它凭据留出边界，但本次不实施。

**门禁结果**：PASS，无复杂度豁免。

## 复杂度追踪

无。当前设计不包含宪章违背项，不需要复杂度豁免。
