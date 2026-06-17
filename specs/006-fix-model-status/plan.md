# 实施计划：修复大模型状态监控显示

**分支**：`008-fix-model-status` | **日期**：2026-06-16 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/006-fix-model-status/spec.md` 的功能规格

**说明**：本计划遵守 `.specify/memory/constitution.md` 中的项目宪章。

## 概要

当前系统监控页的大模型状态仍以 `vllm` 命名，并由 `monitor::vllm_base_url()` / `monitor::vllm_configured_model()` 独立推导监控目标。实际聊天推理由 `Pinvou3Bridge::base_url()` / `model()` / `provider()` 决定，两套逻辑可能漂移：当用户配置远端模型时，监控页仍可能以本地 VLLM 的状态和指标口径表达，导致状态显示不正确或本地指标空值误导用户。

本 feature 将“大模型状态监控”改为按当前实际模型配置选择监控目标：远端模型检测远端目标的可用性、鉴权和模型信息；本地模型检测本地 OpenAI-compatible 目标，并在本地指标可用时继续展示上下文长度、队列、KV 命中率、首字延迟、吞吐和 token 统计。实现范围限制在 `pinvou3-app` 的配置桥接、监控采样、Tauri 命令和前端系统监控展示，不改 DeepSeek-TUI 底座，不负责启动或管理模型服务。

## 技术上下文

**语言/版本**：Rust 2021（`pinvou3-app/src-tauri/Cargo.toml` 当前 `rust-version = "1.88"`）、JavaScript 前端桥接和单页 UI

**主要依赖**：Tauri 2、现有 `monitor.rs`、`commands.rs`、`bridge::Pinvou3Bridge` 配置逻辑、OpenAI-compatible 模型列表响应、本地 vLLM metrics

**存储**：读取现有 `~/.pinvou3/settings.json` 和环境变量；不新增持久化配置

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml monitor --lib`、`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml bridge --lib`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、系统监控页手动 smoke

**目标平台**：Windows 桌面优先，保持 Linux 桌面兼容

**项目类型**：desktop-app

**性能目标**：监控页刷新时，大模型状态检测失败应在 5 秒内给出可理解状态；远端或本地目标不可用不得阻塞 GPU、系统内存和应用信息展示

**约束**：中文文档优先；不改 DeepSeek-TUI 底座；不自动启动、停止或安装本地模型服务；不混入 GPU、系统内存、版本更新栏修复；远端模型状态和本地运行指标必须分开展示；保持小步变更并复用现有配置来源

**规模/范围**：涉及 `pinvou3-app/src-tauri/src/bridge/mod.rs`、`pinvou3-app/src-tauri/src/monitor.rs`、`pinvou3-app/src-tauri/src/commands.rs`、`pinvou3-app/src-tauri/src/harness.rs`、`pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src/index.html`

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。本计划和后续研究、数据模型、契约、quickstart 均使用中文；仅保留必要命令、路径和 API 字段。
- **DeepSeek-TUI 底座优先**：PASS。只修改 `pinvou3-app` 的配置桥接、监控采样和 UI 展示，不重写或修改底座 Engine、Session、Commands、MCP 等能力。
- **本地算力与数据边界**：PASS。本地模型继续作为重要场景；远端模型状态仅在用户当前配置指向远端时检测，不新增隐式外发。
- **小步高质量变更**：PASS。核心方案是复用 `Pinvou3Bridge` 的实际配置推导，并扩展现有监控快照结构，不做无关重构。
- **可测试性与可验证交付**：PASS。计划覆盖配置目标一致性、远端鉴权/不可达、本地非模型服务、指标适用性和编译检查。
- **可维护性与长期演进**：PASS。通过契约文档明确“大模型状态”和“本地 VLLM 指标”的边界，降低后续状态误判。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/006-fix-model-status/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── model-monitor-contract.md
└── checklists/
    └── requirements.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src/
│   ├── index.html
│   └── tauri-bridge.js
└── src-tauri/
    └── src/
        ├── bridge/
        │   └── mod.rs
        ├── commands.rs
        ├── harness.rs
        └── monitor.rs
```

**结构决策**：模型状态监控属于 Tauri app 的系统监控能力，不属于 DeepSeek-TUI 底座。计划将“当前实际模型目标”的推导集中在 `bridge` 层，`monitor` 只负责按目标类型采样状态和本地指标，`commands` 负责把统一目标传入监控，前端负责把远端状态和本地指标分开展示。

## 复杂度追踪

无需填写。当前方案符合既有 app 层职责和宪章边界。

## Phase 0：研究结论

见 [research.md](./research.md)。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- 监控契约：[contracts/model-monitor-contract.md](./contracts/model-monitor-contract.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 复查

- **中文文档优先**：PASS。所有新增文档均为中文。
- **DeepSeek-TUI 底座优先**：PASS。无底座改动计划。
- **本地算力与数据边界**：PASS。只检测用户当前配置的模型目标；远端目标检测由配置驱动。
- **小步高质量变更**：PASS。范围集中在配置目标、监控状态和前端展示语义。
- **可测试性与可验证交付**：PASS。契约覆盖本地、远端、鉴权失败、连接失败、非模型服务、模型不匹配和指标不适用场景。
- **可维护性与长期演进**：PASS。大模型目标类型、状态、诊断和指标适用性有文档可追踪。
