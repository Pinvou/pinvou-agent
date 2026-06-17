# 实施计划：Windows 全功能测试参考清单

**分支**：`010-update-test-docs` | **日期**：2026-06-17 | **规格**：[spec.md](./spec.md)

**输入**：来自用户修正后的目标：“不是升级功能需求文档，而是整个应用在 Windows 下的全部功能列表，给测试参考”。

## 概要

本 feature 不新增运行时代码，而是基于当前 Windows 应用的 UI、Tauri 命令边界和现有交互，整理一份面向测试人员的全功能需求与回归参考。文档覆盖启动导航、聊天会话、设置与模型、附件与产物、Plan/工具交互、专家卡与技能、工作流、监控、工具市场、Windows 更新、依赖检查和多语言文案。

## 技术上下文

**语言/版本**：Markdown 文档；参考现有 Tauri 2 桌面应用、Rust 命令层和前端 JavaScript UI 状态。

**主要依赖**：Spec Kit 文档结构；当前 `pinvou3-app` 前端 UI；当前 `src-tauri` 命令注册；Windows 系统集成能力。

**存储**：不新增运行时存储；文档位于 `specs/008-update-test-docs/`。

**测试**：文档质量清单；需求到验收场景的可追踪检查；后续可从本目录派生手工测试用例。

**目标平台**：Windows 桌面端。

**项目类型**：Documentation / QA planning。

**性能目标**：测试人员可在 30 分钟内理解 Windows 主要功能域，并据此拆分冒烟、回归和专项测试。

**约束**：中文文档优先；不修改运行时代码；不重新定义 DeepSeek-TUI 底座能力；功能描述以当前 UI 可观察行为为准。

**规模/范围**：覆盖 `specs/008-update-test-docs/` 内规格、计划、研究、数据模型、UI 合约和 quickstart；`AGENTS.md` 指针保留到当前 plan。

## 宪章检查

- **中文文档优先**：PASS。核心规格、计划、研究、模型、合约和 quickstart 均为中文。
- **DeepSeek-TUI 底座优先**：PASS。本 feature 只整理 pinvou3 Windows UI 功能列表，不复制或重写 Engine、ToolRegistry、Session、MCP、Hooks 等底座能力。
- **本地算力与数据边界**：PASS。文档描述本地应用和本地模型连接行为，不引入新的远程模型或数据处理机制。
- **小步高质量变更**：PASS。变更限定在 Spec Kit 文档产物，运行时代码不变。
- **可测试性与可验证交付**：PASS。规格按用户故事、验收场景、边界情况和成功标准组织。
- **可维护性与长期演进**：PASS。功能清单按测试域拆分，便于后续随 UI 演进增补。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/008-update-test-docs/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── windows-app-test-contract.md
├── checklists/
│   └── requirements.md
└── spec.md
```

### 源码参考（不修改）

```text
pinvou3-app/
├── src/
│   ├── index.html              # Windows UI 入口、页面结构、主要交互文案
│   └── tauri-bridge.js         # 前端状态、Tauri 命令调用和页面事件
└── src-tauri/src/
    ├── lib.rs                  # Tauri 命令注册清单
    ├── updater.rs              # 更新命令编排
    └── os/windows/             # Windows 系统集成、更新、权限和系统能力
```

**结构决策**：文档是测试参考，不改变应用结构；源码路径仅作为功能来源说明。

## 复杂度追踪

无宪章违背项，无需复杂度豁免。

## Phase 0：研究输出

研究结论记录在 [research.md](./research.md)，覆盖：

- 以当前 UI 和命令注册作为功能清单事实来源。
- 按测试域组织，而不是按代码文件组织。
- Windows 平台专项风险：路径、权限、外部工具、MSI、WebView、系统打开。
- 更新功能只是全功能清单中的一个模块。
- 文档面向测试人员，避免暴露实现细节。

## Phase 1：设计与合约输出

- 数据模型：[data-model.md](./data-model.md)
- UI 测试合约：[contracts/windows-app-test-contract.md](./contracts/windows-app-test-contract.md)
- 快速开始：[quickstart.md](./quickstart.md)
- Agent 上下文：`AGENTS.md` 已指向本计划文件

## Phase 1 宪章复查

- **中文文档优先**：PASS。新增或修正文档均为中文。
- **DeepSeek-TUI 底座优先**：PASS。只描述可观察行为，不新增底座实现。
- **本地算力与数据边界**：PASS。测试范围限定本地 Windows 应用和既有外部服务。
- **小步高质量变更**：PASS。仅文档产物变更。
- **可测试性与可验证交付**：PASS。合约和 quickstart 可直接用于测试执行。
- **可维护性与长期演进**：PASS。模块化清单便于后续随 UI 更新维护。

**复查结果**：PASS。
