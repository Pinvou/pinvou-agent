# 实施计划：Pinvou 接入 LLM API Hub 中转站

**分支**：`024-llmapi-hub-integration` | **日期**：2026-07-08 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/024-llmapi-hub-integration/spec.md` 的功能规格。

## 概要

Pinvou 需要在 Windows 系统上，于用户首次使用 AI 助手时，后端基于当前设备 BIOS SN 派生 `device_binding_id`，并直接使用该值作为 `pinvou_user_id` 确认本地身份，只查询该身份是否已有 New API 后台账户；若存在则准备 token、加密保存为本地绑定，并把模型调用路由到 `https://www.ma-xiao.com/llmapi/v1`，若不存在则提示服务未开通且不创建后台账户。实现上不重写 DeepSeek-TUI 的 Engine、Session、SSE 或工具循环，而是在 `pinvou3-app` 的 Rust bridge 层新增 LLM API Hub 查询绑定模块，复用现有 `credential_store` 保存敏感 token，复用 `SavedModel`/`EnginePool` 的 OpenAI-compatible 调用路径完成模型交互；非 Windows 平台本次不实现该功能。

## 技术上下文

**语言/版本**：Rust 1.88、JavaScript、Tauri 2、PowerShell/Bash 脚本。

**主要依赖**：Tauri 2、DeepSeek-TUI/codewhale-tui、codewhale-secrets、reqwest、serde/serde_json、tokio、rusqlite、现有 `EnginePool` 和 `bridge::prefs`。

**存储**：`~/.pinvou3/settings.json` 保存非敏感配置和绑定元数据；`codewhale-secrets` 保存 New API 用户 token 和 New API 管理凭证；如需要结构化查询，新增 `~/.pinvou3/llmapi-hub.sqlite` 或复用现有轻量 JSON 存储，但 token 和 BIOS SN 明文仍不落普通文件。

**测试**：`cargo test -p pinvou3-tauri --lib`；新增模块单测覆盖设备绑定校验、查询绑定幂等、凭证不序列化、错误脱敏、usage 累计；前端 smoke 覆盖首次使用、设备绑定失败、额度显示、管理员禁用/重试；中转站联调使用测试 token 和测试用户。

**目标平台**：仅 Windows 桌面；Linux 和其他非 Windows 平台本次不实现 LLM API Hub 中转站接入。中转站服务为远端 HTTPS。

**项目类型**：desktop-app，带本地 Rust 后端、WebView 前端和 DeepSeek-TUI bridge。

**性能目标**：已绑定用户不增加明显首 token 延迟；首次查询绑定在正常中转站管理接口下 95% 场景可在一次用户请求内完成；usage 快照 95% 在调用完成后 5 秒内更新。

**约束**：不得把 New API token、DeepSeek API Key 或 BIOS SN 明文暴露给前端、settings 明文、普通日志或导出数据；不得重写 DeepSeek-TUI 的 Engine/SSE/Session；外部网络调用必须显式配置并可关闭；当前入口使用 `https://www.ma-xiao.com/llmapi/v1`，后续可切换独立子域。

**规模/范围**：涉及 Windows 桌面端的 `pinvou3-app/src-tauri/src/credential_store.rs`、`bridge/prefs.rs`、`bridge/mod.rs`、`commands.rs`、`engine_pool.rs` 接线、前端设置/状态展示和新模块 `llmapi_hub`；不包含 Linux/非 Windows 实现、New API 服务端部署、Nginx、证书、DeepSeek 渠道配置或 New API 本体二次开发。

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：本计划、研究、数据模型、契约和 quickstart 均使用中文，保留 API 字段和命令英文原文。
- **DeepSeek-TUI 底座优先**：只在 pinvou3-app bridge 层决定模型配置和 token 获取，不重写 Engine、ToolRegistry、SSE、Session、MCP、Hooks、Cycle 或 Compaction。
- **本地算力与数据边界**：默认本地模型能力仍保留；远端 LLM API Hub 是用户/产品明确配置的云端路径，token 只在后端和 keyring 使用，BIOS SN 只用于派生设备绑定标识。
- **小步高质量变更**：新增独立 `llmapi_hub` 模块和最小 bridge 接线，避免改 DeepSeek-TUI fork；前端只在 Windows 可用路径增加状态、额度和管理员操作入口。
- **可测试性与可验证交付**：查询绑定、幂等、凭证脱敏、usage 更新和错误分类都有单测或 smoke 验证路径。
- **可维护性与长期演进**：管理接口适配隔离在 adapter 中，New API 升级或入口迁移只影响配置和 adapter，不扩散到会话/引擎层。

**门禁结果**：PASS。该功能确实引入远端模型服务，但来自明确产品需求，并通过后端绑定、keyring、错误脱敏和可关闭配置控制数据边界。

## 项目结构

### 文档（本 feature）

```text
specs/024-llmapi-hub-integration/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── tauri-commands.md
│   └── llmapi-hub-adapter.md
└── tasks.md
```

### 源码（仓库根目录）

```text
pinvou3-app/src-tauri/src/
├── llmapi_hub/
│   ├── mod.rs
│   ├── store.rs
│   ├── provisioning.rs
│   ├── adapter.rs
│   └── usage.rs
├── credential_store.rs
├── bridge/
│   ├── mod.rs
│   └── prefs.rs
├── commands.rs
└── engine_pool.rs

pinvou3-app/src/
├── index.html
└── tauri-bridge.js
```

**结构决策**：LLM API Hub 的绑定、查询既有后台账户、管理接口适配和 usage 统计放入独立 `llmapi_hub` 模块；`bridge` 只读取“当前用户应使用的模型配置/token”，`commands` 只暴露 Tauri 命令，避免把中转站细节散落到聊天、会话和前端状态代码中。

## 复杂度追踪

| 违反项 | 为什么必要 | 拒绝的更简单替代方案 |
|---|---|---|
| 引入远端 LLM API Hub | 用户明确要求 Pinvou 通过中转站与大模型交互，并由 New API 负责真实额度、限流和日志 | 继续只使用本地 vLLM 或让用户手动配置第三方 API Key，无法满足统一中转和后台资源管理 |
| 新增查询绑定状态机 | 用户要求首次使用只查询既有后台账户，同时必须处理部分成功、重试和幂等 | 首次失败直接报错给用户或手动修库，会造成重复 token 和不可恢复状态 |
| 新增管理员状态/重试能力 | 查询绑定失败需要可观测和可恢复，避免用户侧只看到不可用 | 仅写日志不提供状态查询，排查成本高且不符合规格 |

## Phase 0 输出

见 [research.md](./research.md)。所有规划期未知点已落为决策：复用现有 OpenAI-compatible 模型路径、使用 keyring 保存 token、以 adapter 隔离 New API 管理接口、以本地绑定状态机保证幂等。

## Phase 1 输出

- [data-model.md](./data-model.md)
- [contracts/tauri-commands.md](./contracts/tauri-commands.md)
- [contracts/llmapi-hub-adapter.md](./contracts/llmapi-hub-adapter.md)
- [quickstart.md](./quickstart.md)

## 宪章复查

**复查结果**：PASS。设计产物没有新增 DeepSeek-TUI fork 改动；远端 API 被限制在 Windows 端 `llmapi_hub` adapter 和 bridge 配置路径；token 明文只允许在内存中短暂存在并经 `credential_store` 存取；BIOS SN 明文不得进入普通存储和日志；验证路径覆盖 Windows 设备绑定、查询绑定、幂等、额度、错误脱敏和前端展示。
