# 实施计划：[FEATURE]

**分支**：`[###-feature-name]` | **日期**：[DATE] | **规格**：[link]

**输入**：来自 `/specs/[###-feature-name]/spec.md` 的功能规格

**说明**：本模板由 `/speckit-plan` 填充。计划必须遵守 `.specify/memory/constitution.md` 中的项目宪章。

## 概要

[从功能规格提取：核心用户价值 + 研究后确定的技术/文档/实现路径]

## 技术上下文

**语言/版本**：[例如 Rust 1.88、JavaScript、Python 3，或 NEEDS CLARIFICATION]

**主要依赖**：[例如 Tauri 2、DeepSeek-TUI、vLLM、本地系统工具，或 N/A]

**存储**：[例如 文件、settings.json、session/workflow 目录，或 N/A]

**测试**：[例如 cargo test、fork-guard、前端 smoke、文档占位扫描，或 NEEDS CLARIFICATION]

**目标平台**：[例如 Windows 桌面、Linux 桌面、跨平台桌面，或 NEEDS CLARIFICATION]

**项目类型**：[例如 desktop-app、documentation、tooling、workflow，或 NEEDS CLARIFICATION]

**性能目标**：[面向用户的可验证目标，或 N/A]

**约束**：[本地算力、DeepSeek-TUI 底座边界、中文文档、跨平台、隐私/数据边界等]

**规模/范围**：[涉及模块、文档、用户流程、迁移范围]

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：本计划、研究、数据模型、契约、quickstart、任务说明是否使用中文；英文是否仅用于必要术语/命令/API。
- **DeepSeek-TUI 底座优先**：是否避免重写 Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP、Hooks、Cycle、Compaction。
- **本地算力与数据边界**：是否尊重 GB10/Qwen/vLLM 基线，明确远端网络/模型/外部 API 的配置边界。
- **小步高质量变更**：是否限定改动范围，避免无关重构，遵循现有代码结构。
- **可测试性与可验证交付**：是否定义适合风险级别的验证命令、检查项或手动验收。
- **可维护性与长期演进**：是否更新相关文档、fork 记录、迁移风险和已知约束。
- **合并保全与用户裁决**：是否识别双方功能增量、保留本地能力；不可机械判定的取舍是否已暂停并请求用户决策。
- **禅道问题规则**：若工作来源于禅道 BUG，是否已读取 `.specify/memory/constitution-zentao.md`，记录 Bug ID、目标版本/`buildID`、双仓推送和状态回查步骤，并保护认证信息。

**门禁结果**：[PASS/FAIL；如 FAIL，说明违反项、理由、替代方案和缓解措施]

## 项目结构

### 文档（本 feature）

```text
specs/[###-feature]/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
└── tasks.md
```

### 源码（仓库根目录）

```text
[按当前 feature 的真实涉及范围填写。删除无关目录，不保留示例占位。]
```

**结构决策**：[说明选择此结构的原因，并引用真实目录]

## 复杂度追踪

> 仅当宪章检查存在需要解释的违反项时填写。

| 违反项 | 为什么必要 | 拒绝的更简单替代方案 |
|---|---|---|
| [例如：新增跨平台抽象层] | [当前需求原因] | [为什么直接局部判断不够] |
