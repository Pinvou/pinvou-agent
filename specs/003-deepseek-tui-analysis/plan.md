# 实施计划：DeepSeek-TUI 源码职责分析

**分支**：`003-deepseek-tui-analysis` | **日期**：2026-06-15 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/003-deepseek-tui-analysis/spec.md` 的功能规格

## 概要

本 feature 的目标是产出一份面向 pinvou3 Windows 应用维护者的中文源码分析文档，回答“DeepSeek-TUI 在当前项目中都做了什么”。计划不修改运行时代码，而是通过源码扫描、pinvou3-app 接入点追踪和维护风险归纳，形成可验证的职责全景、关键调用链、底座边界和后续排查路径。

## 技术上下文

**语言/版本**：中文文档；源码证据涉及 Rust 2024 / Rust 1.88+、JavaScript、Tauri 2、Spec Kit Markdown。

**主要依赖**：DeepSeek-TUI 子模块、pinvou3-app Tauri/Rust bridge、Spec Kit 文档工件、现有 AGENTS.md 项目规则。

**存储**：文档写入 `docs/`；计划与设计工件写入 `specs/003-deepseek-tui-analysis/`；不新增运行时数据存储。

**测试**：文档结构检查、源码证据点核对、调用链覆盖检查、中文输出检查、`git status` 确认无业务代码误改。

**目标平台**：Windows 桌面维护场景为主，同时说明 DeepSeek-TUI 作为跨平台底座的源码边界。

**项目类型**：documentation / source-analysis。

**性能目标**：新维护者阅读 10 分钟内能建立 DeepSeek-TUI 职责全景；文档至少覆盖 12 个源码证据点、6 条关键调用链和 8 条维护风险检查项。

**约束**：中文文档优先；不得重写 DeepSeek-TUI 底座能力；不得修改业务代码；分析以当前仓库检出的子模块和 pinvou3-app 接入方式为准；必须区分上游通用能力、fork 专用改动和 pinvou3-app 适配层。

**规模/范围**：覆盖 `DeepSeek-TUI/` 工作区主要 crate、`pinvou3-app/src-tauri/src/` 中对 deepseek_tui 的调用、现有项目规则与 Windows 维护注意事项；不覆盖 DeepSeek-TUI 全部历史、未接入实验能力或外部部署细节。

## 宪章检查
*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。计划、研究、模型、契约、quickstart 和最终文档均使用中文；英文仅保留 crate、类型、命令和路径。
- **DeepSeek-TUI 底座优先**：PASS。本 feature 只分析底座能力和复用边界，不重新实现 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle、Compaction。
- **本地算力与数据边界**：PASS。文档需说明本地 vLLM / OpenAI-compatible 配置、session/artifact/settings 等数据边界，但不引入外部服务。
- **小步高质量变更**：PASS。范围限定为 Spec Kit 工件、AGENTS 指针和最终 docs 文档，不改业务代码。
- **可测试性与可验证交付**：PASS。成功标准已量化，设计中定义文档契约和 quickstart 验收步骤。
- **可维护性与长期演进**：PASS。目标文档专门沉淀底座职责、版本风险、子模块检查和后续排查路径。

**门禁结果**：PASS，无需复杂度追踪豁免。

## 项目结构

### 文档（本 feature）
```text
specs/003-deepseek-tui-analysis/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── analysis-document.md
└── tasks.md
```

### 源码（仓库根目录）
```text
DeepSeek-TUI/
├── Cargo.toml
├── crates/
│   ├── agent/
│   ├── app-server/
│   ├── cli/
│   ├── config/
│   ├── core/
│   ├── execpolicy/
│   ├── hooks/
│   ├── mcp/
│   ├── protocol/
│   ├── release/
│   ├── secrets/
│   ├── state/
│   ├── tools/
│   ├── tui/
│   ├── tui-core/
│   └── whaleflow/
├── docs/
├── integrations/
├── npm/
└── extensions/

pinvou3-app/
└── src-tauri/src/
    ├── bridge/
    ├── engine.rs
    ├── engine_pool.rs
    ├── commands.rs
    ├── workflow_migrate.rs
    └── lib.rs

docs/
└── DeepSeek-TUI源码职责分析.md
```

**结构决策**：计划工件放在当前 feature 目录；最终交付放在 `docs/`，符合项目长期维护资料位置。源码分析以 `DeepSeek-TUI/` 工作区为底座证据，以 `pinvou3-app/src-tauri/src/` 为接入证据。

## 复杂度追踪

无宪章违背项；不需要复杂度豁免。

## Phase 0：研究输出

研究文件：[research.md](./research.md)

研究目标：
- 确定 DeepSeek-TUI 源码分析的分层口径。
- 确定 pinvou3-app 接入链路的追踪范围。
- 确定最终文档的证据粒度、维护风险粒度和验收方式。

## Phase 1：设计输出

- 数据模型：[data-model.md](./data-model.md)
- 文档契约：[contracts/analysis-document.md](./contracts/analysis-document.md)
- 快速开始：[quickstart.md](./quickstart.md)

## Phase 1 后宪章复查

- **中文文档优先**：PASS。所有新增计划工件为中文主体。
- **DeepSeek-TUI 底座优先**：PASS。设计产物强调复用与边界，不产生替代实现。
- **本地算力与数据边界**：PASS。文档契约要求说明本地配置与数据目录边界。
- **小步高质量变更**：PASS。产物仍限定在文档与 Spec Kit 范围。
- **可测试性与可验证交付**：PASS。quickstart 明确验收清单和最小检查命令。
- **可维护性与长期演进**：PASS。契约要求包含后续排查路径和维护风险清单。

**复查结果**：PASS。
