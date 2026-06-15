# 实施计划：Windows 迁移与接手维护文档

**分支**：`001-windows-onboarding-docs` | **日期**：2026-06-15 | **规格**：[spec.md](./spec.md)

**输入**：来自 `/specs/001-windows-onboarding-docs/spec.md` 的功能规格

**说明**：本计划由 `/speckit-plan` 生成，并已按 `.specify/memory/constitution.md` 的项目宪章复核。

## 概要

交付一份面向 Windows 应用开发工程师的 pinvou3 项目接手与迁移维护文档，帮助新工程师快速理解项目调用流程、底座边界、依赖项目、Windows 迁移风险、维护红线与验证方式。技术路径是文档优先：基于当前仓库源码、现有设计文档和 Spec Kit 规格，建立可验收的文档结构、风险模型和快速使用指南，不在本阶段修改运行时代码。

## 技术上下文

**语言/版本**：Markdown 文档；仓库实现上下文包含 Rust 1.88、Tauri 2.0、JavaScript 静态前端、Python 工作流脚本。

**主要依赖**：仅依赖现有仓库资料：`pinvou3-app`、`DeepSeek-TUI` submodule、vLLM/Qwen3.6 运行假设、现有 `docs/`、`process.md`、`AGENTS.md` 和 Spec Kit 文件。

**存储**：文档文件位于 `docs/`，规划产物位于 `specs/001-windows-onboarding-docs/`。

**测试**：通过审阅、未解决占位符搜索、按规格 checklist 复核进行 Markdown/静态验证。运行时命令可作为后续验证参考，但本阶段为纯文档交付，不要求执行运行时测试。

**目标平台**：面向 Windows 迁移和维护的工程师文档；当前应用开发与打包仍偏 Linux。

**项目类型**：桌面应用仓库的文档型 feature。

**性能目标**：新工程师能在规格成功标准定义的时间内识别核心架构、至少 10 个 Windows 迁移风险和常见维护入口。

**约束**：必须保护 DeepSeek-TUI 底座边界；不得推荐重写已有底座能力；必须区分文档/规划与实际 Windows 代码迁移；必须反映当前项目状态和已下线方案。

**规模/范围**：一份主要接手文档，加 Spec Kit 规划产物：research、data-model、contracts、quickstart。本阶段不修改生产代码。

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。本 feature 的主要交付物和规划产物均为中文；英文仅保留必要命令、路径、API/工具名和模板字段。
- **DeepSeek-TUI 底座优先**：PASS。本 feature 不新增 runtime 代码，且文档明确禁止在 pinvou3 重写底座已有 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle、Compaction。
- **本地算力与数据边界**：PASS。文档以本地 GB10/Qwen3.6/vLLM 为默认基线，同时标注 Windows 迁移中的 endpoint、远程服务和用户数据目录边界。
- **小步高质量变更**：PASS。当前计划仅新增/更新文档和 Spec Kit artifacts，不做无关代码重构。
- **可测试性与可验证交付**：PASS。规格和 quickstart 定义了占位扫描、覆盖检查、风险表和后续验证命令。
- **可维护性与长期演进**：PASS。文档引用 `docs/fork-modifications.md`、`process.md`、`AGENTS.md` 和当前源码路径，并标注已下线方案。

门禁结果：PASS。本 feature 是纯文档交付，强化正式宪章约束，不新增运行时抽象。

## 项目结构

### 文档（本 feature）

```text
specs/001-windows-onboarding-docs/
├── plan.md              # 本文件
├── research.md          # Phase 0 输出
├── data-model.md        # Phase 1 输出
├── quickstart.md        # Phase 1 输出
├── contracts/           # Phase 1 输出
└── tasks.md             # Phase 2 输出，由 /speckit-tasks 生成
```

### 源码（仓库根目录）

```text
docs/
└── Windows迁移与维护接手手册.md

AGENTS.md
└── SPECKIT 当前计划指针

pinvou3-app/
├── src/
│   ├── index.html
│   └── tauri-bridge.js
└── src-tauri/
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── build.rs
    └── src/
        ├── lib.rs
        ├── commands.rs
        ├── engine.rs
        ├── engine_pool.rs
        ├── file_ingest.rs
        └── bridge/

DeepSeek-TUI/
└── crates/tui/            # submodule 初始化后的 Cargo path dependency

workflows/
└── sansheng-liubu/        # 文档引用的工作流源数据
```

**结构决策**：这是文档/规划型 feature。主要交付物位于 `docs/`；Spec Kit 产物位于 `specs/001-windows-onboarding-docs/`。源码目录仅作为事实引用，本计划不修改生产代码。

## 复杂度追踪

> 仅当宪章检查存在需要解释的违反项时填写。

无宪章违反项。

## Phase 0 研究摘要

见 [research.md](./research.md)。所有开放决策均已解决，无需额外用户澄清。

## Phase 1 设计摘要

见 [data-model.md](./data-model.md)、[quickstart.md](./quickstart.md) 和 [contracts/documentation-contract.md](./contracts/documentation-contract.md)。

## 设计后宪章复查

门禁结果：PASS。Phase 1 产物保持中文文档优先，采用文档契约而非运行时 API 契约，未新增底座重复实现，且保留了可验证交付和长期维护路径。
