# 实施计划：加密存储 MCP API 密钥

**分支**：`020-encrypt-mcp-secrets` | **日期**：2026-06-26 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/020-encrypt-mcp-secrets/spec.md` 的功能规格

**说明**：本计划遵守 `.specify/memory/constitution.md` 中的项目宪章。

## 概要

本 feature 修复内置 MCP 工具供应商密钥在客户端资源、用户目录 `manifest.json`、以及 `~/.pinvou3/bundle/mcp.json` 中明文落盘的问题。范围覆盖同花顺问财、企查查、高德天气，并为后续内置 MCP 工具提供同一套密钥声明、迁移、注入和缺失反馈机制。

实施边界放在 `pinvou3-app` 的 bundle、工具市场和凭据存储层：内置 manifest 不再保存真实密钥，只声明需要的敏感环境变量或 bearer 凭据；应用启动和工具安装时扫描旧版明文配置，将可迁移密钥写入系统凭据存储；生成新的 MCP 运行配置时不再持久化真实密钥。DeepSeek-TUI 继续作为 MCP client 底座，不重写 MCP client、ToolRegistry 或 engine 行为。

需要特别记录的安全取舍：纯本地客户端无法同时满足“包内携带统一产品密钥”“用户完全无感”“密钥不可从客户端提取”三件事。当前计划选择移除硬编码密钥并使用系统凭据保护已有或已配置密钥；如果未来必须保持产品统一密钥且新装零配置，应单独建设服务端代理或短期凭据下发机制。

## 技术上下文

**语言/版本**：Rust 2021（`pinvou3-app/src-tauri/Cargo.toml` 当前 `rust-version = "1.88"`）；JavaScript 前端保持现有 Tauri command 调用方式

**主要依赖**：Tauri 2、DeepSeek-TUI、现有 `bridge::bundle`、`bridge::marketplace`、`bridge::paths`、`credential_store`、`serde_json`、`keyring`；不新增 DeepSeek-TUI 依赖

**存储**：继续使用 `~/.pinvou3/bundle/mcp-servers/*/manifest.json`、`~/.pinvou3/bundle/mcp.json`、`~/.pinvou3/marketplace/installed.json`；真实密钥只存放在系统凭据存储；manifest 与 mcp.json 只保存非敏感声明、凭据引用或缺失状态

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace --lib`、`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml credential_store --lib`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、静态扫描内置资源和用户目录样例中是否残留目标密钥明文、Windows 手动 smoke 安装/启用同花顺/企查查/高德天气

**目标平台**：Windows 桌面为主要验收平台；Linux 桌面应保持兼容，不引入 Windows-only 默认路径或行为

**项目类型**：desktop-app

**性能目标**：应用启动时 MCP 密钥迁移和清理不应造成明显启动卡顿；正常用户目录下迁移与清理应在 1 秒内完成；工具安装仍保持用户可感知的即时反馈

**约束**：中文文档优先；不重写 DeepSeek-TUI MCP client；不在日志、错误、前端状态或诊断输出中暴露真实密钥；不把可逆加密密钥硬编码进客户端作为安全边界；不破坏已安装 MCP 工具的路由规则、工具列表和供应商 API 能力

**规模/范围**：涉及 `pinvou3-app/resources/mcp-servers/{weather,iwencai,qcc}/manifest.json`、`pinvou3-app/src-tauri/src/bridge/bundle.rs`、`pinvou3-app/src-tauri/src/bridge/marketplace.rs`、`pinvou3-app/src-tauri/src/credential_store.rs`、工具市场 Tauri commands、必要的前端错误反馈；新增本 feature 文档、契约和验证说明

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。计划、研究、数据模型、契约和 quickstart 均使用中文；仅保留 `MCP`、`API Key`、`manifest.json`、`mcp.json`、字段名和命令等必要英文术语。
- **DeepSeek-TUI 底座优先**：PASS。本 feature 不改 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP client、Hooks、Cycle 或 Compaction；只调整 pinvou 生成 MCP 配置前的密钥处理。
- **本地算力与数据边界**：PASS。变更聚焦本地配置与外部供应商 API 凭据边界，不引入远程模型或隐式搜索；明确真实密钥生命周期和落盘位置。
- **小步高质量变更**：PASS。改动集中在 MCP bundle/marketplace/credential 层，复用现有凭据存储和工具市场结构，避免无关重构。
- **可测试性与可验证交付**：PASS。计划包含单测、静态扫描和 Windows smoke；契约覆盖 manifest、mcp.json、迁移和错误反馈。
- **可维护性与长期演进**：PASS。通过数据模型和契约沉淀后续 MCP 工具复用方式，并记录“客户端无法安全内置统一密钥”的长期架构边界。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/020-encrypt-mcp-secrets/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── mcp-manifest-secrets.md
│   ├── mcp-runtime-config.md
│   └── mcp-secret-migration.md
└── checklists/
    └── requirements.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── resources/
│   └── mcp-servers/
│       ├── weather/manifest.json
│       ├── iwencai/manifest.json
│       └── qcc/manifest.json
└── src-tauri/
    └── src/
        ├── credential_store.rs
        ├── bridge/
        │   ├── bundle.rs
        │   ├── marketplace.rs
        │   └── paths.rs
        └── commands.rs
```

**结构决策**：MCP 工具市场已经由 `bridge::bundle` 解包内置资源，并由 `bridge::marketplace` 将 manifest 转成底座读取的 `mcp.json`。因此密钥声明、迁移和运行配置清理放在这些既有边界内最小化变更；真实密钥保存复用 `credential_store.rs`，避免新增独立加密实现。

## 复杂度追踪

> 仅当宪章检查存在需要解释的违反项时填写。

| 违反项 | 为什么必要 | 拒绝的更简单替代方案 |
|---|---|---|
| 无 | N/A | N/A |

## Phase 0：研究结论

见 [research.md](./research.md)。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- MCP manifest 密钥契约：[contracts/mcp-manifest-secrets.md](./contracts/mcp-manifest-secrets.md)
- MCP 运行配置契约：[contracts/mcp-runtime-config.md](./contracts/mcp-runtime-config.md)
- MCP 密钥迁移契约：[contracts/mcp-secret-migration.md](./contracts/mcp-secret-migration.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 复查

- **中文文档优先**：PASS。新增设计产物均为中文。
- **DeepSeek-TUI 底座优先**：PASS。设计仅影响 pinvou 写入/迁移 MCP 配置的前置层，不改底座 MCP client。
- **本地算力与数据边界**：PASS。真实密钥的存储、迁移和注入边界已在数据模型与契约中定义。
- **小步高质量变更**：PASS。实现路径复用已有 `CredentialStore`，仅扩展 MCP 工具配置生成流程。
- **可测试性与可验证交付**：PASS。quickstart 明确列出静态扫描、单测、旧配置迁移和 Windows smoke。
- **可维护性与长期演进**：PASS。契约定义了后续 MCP 工具加入敏感字段的方式，并记录服务端代理作为未来零配置统一密钥方案。
