# 任务：DeepSeek-TUI 源码职责分析

**输入**：`specs/003-deepseek-tui-analysis/` 下的设计文档

**前置条件**：plan.md、spec.md、research.md、data-model.md、contracts/analysis-document.md、quickstart.md

**测试**：本 feature 为文档交付，验证任务以文档契约检查、源码证据覆盖检查、调用链覆盖检查和 worktree 范围检查为主。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。

## Phase 1: 准备（共享基础）

**目的**：确认当前 feature 上下文、文档契约和不改业务代码边界。

- [X] T001 阅读 `specs/003-deepseek-tui-analysis/plan.md` 并确认宪章检查结果为 PASS
- [X] T002 阅读 `specs/003-deepseek-tui-analysis/contracts/analysis-document.md` 并提取最终文档必备章节清单
- [X] T003 [P] 执行 `git status --short --branch` 并记录本 feature 允许修改范围到 `specs/003-deepseek-tui-analysis/tasks.md`
- [X] T004 [P] 确认目标文档路径 `docs/DeepSeek-TUI源码职责分析.md` 是否已存在并记录处理策略到 `specs/003-deepseek-tui-analysis/tasks.md`

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：完成源码证据采集框架和文档骨架，供所有用户故事复用。

**CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T005 扫描 `DeepSeek-TUI/Cargo.toml` 和 `DeepSeek-TUI/crates/`，整理工作区 crate 清单到 `docs/DeepSeek-TUI源码职责分析.md`
- [X] T006 [P] 扫描 `pinvou3-app/src-tauri/src/` 中的 `deepseek_tui::` 调用点，整理接入证据清单到 `docs/DeepSeek-TUI源码职责分析.md`
- [X] T007 [P] 扫描 `AGENTS.md`、`.specify/memory/constitution.md` 和 `specs/003-deepseek-tui-analysis/research.md`，整理项目边界原则到 `docs/DeepSeek-TUI源码职责分析.md`
- [X] T008 根据 `specs/003-deepseek-tui-analysis/contracts/analysis-document.md` 创建 `docs/DeepSeek-TUI源码职责分析.md` 的章节骨架

**检查点**：源码证据框架和目标文档骨架完成，可以开始按用户故事实施。

---

## Phase 3: 用户故事 1 - 建立底座职责全景 (Priority: P1) MVP

**目标**：让维护者能在 10 分钟内理解 DeepSeek-TUI 的模块全景、底座职责和 pinvou3 复用边界。

**独立测试**：阅读 `docs/DeepSeek-TUI源码职责分析.md` 的总览和源码全景章节后，能够列出至少 8 类 DeepSeek-TUI 核心能力及其在 pinvou3 中的复用方式。

### 测试 / 验证

- [X] T009 [P] [US1] 对照 `specs/003-deepseek-tui-analysis/data-model.md` 检查 `docs/DeepSeek-TUI源码职责分析.md` 是否包含“源码模块”和“底座能力”两类实体
- [X] T010 [P] [US1] 对照 `specs/003-deepseek-tui-analysis/contracts/analysis-document.md` 检查 `docs/DeepSeek-TUI源码职责分析.md` 是否包含“阅读导向”“一句话定位”“源码全景”“不要重复造轮子的能力”章节

### 实现

- [X] T011 [US1] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写“阅读导向”和“一句话定位”章节
- [X] T012 [US1] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写 `DeepSeek-TUI/Cargo.toml` 工作区全景和主要 crate 职责
- [X] T013 [US1] 在 `docs/DeepSeek-TUI源码职责分析.md` 中标注 DeepSeek-TUI 能力的当前直接接入、间接依赖和当前未直接接入状态
- [X] T014 [US1] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写“不要重复造轮子的能力”章节，覆盖 Engine、ToolRegistry、流式事件、Session、SkillRegistry、Commands、MCP、Hooks、Cycle、Compaction
- [X] T015 [US1] 在 `docs/DeepSeek-TUI源码职责分析.md` 中补充至少 12 个源码证据点并确保每个证据点包含仓库相对路径或关键类型

**检查点**：US1 可独立阅读和验证，形成 DeepSeek-TUI 底座职责全景。

---

## Phase 4: 用户故事 2 - 追踪关键调用链 (Priority: P2)

**目标**：让维护者能从 pinvou3-app 用户操作或启动流程追踪到 DeepSeek-TUI 底座能力、事件回传和会话落盘。

**独立测试**：依据文档中的调用链，维护者能定位发送消息、启动初始化、session、技能、工具、工作流等问题对应的源码区域。

### 测试 / 验证

- [X] T016 [P] [US2] 对照 `specs/003-deepseek-tui-analysis/contracts/analysis-document.md` 检查 `docs/DeepSeek-TUI源码职责分析.md` 是否至少包含 6 条关键调用链
- [X] T017 [P] [US2] 使用 `rg "deepseek_tui::core::engine|EngineConfig|SessionManager|SkillRegistry|SpawnSubAgent" pinvou3-app/src-tauri/src` 抽查 `docs/DeepSeek-TUI源码职责分析.md` 中的调用链证据

### 实现

- [X] T018 [US2] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写 `pinvou3-app/src-tauri/src/lib.rs` 到 bridge/engine 初始化的启动调用链
- [X] T019 [US2] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写 `pinvou3-app/src-tauri/src/bridge/mod.rs` 构造 `EngineConfig` 的配置调用链
- [X] T020 [US2] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写 `pinvou3-app/src-tauri/src/commands.rs` 和 `pinvou3-app/src-tauri/src/engine.rs` 的用户消息发送与事件转译调用链
- [X] T021 [US2] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写 `pinvou3-app/src-tauri/src/bridge/sessions.rs` 与 DeepSeek-TUI `SessionManager` 的会话管理调用链
- [X] T022 [US2] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写 `SkillRegistry`、工具发现和 MCP/Hooks 相关的接入边界
- [X] T023 [US2] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写 workflow 子任务、`SpawnSubAgent` 和工作流状态回传调用链

**检查点**：US2 可独立验证，文档能用于追踪关键运行路径。

---

## Phase 5: 用户故事 3 - 沉淀维护注意事项 (Priority: P3)

**目标**：沉淀合并、升级、Windows 构建运行、打包和底座边界相关的维护风险与排查入口。

**独立测试**：维护者能够依据文档完成一次合并后冒烟检查，并识别子模块版本不匹配、构建产物占用、路径异常或底座 API 字段变化等问题。

### 测试 / 验证

- [X] T024 [P] [US3] 对照 `specs/003-deepseek-tui-analysis/quickstart.md` 检查 `docs/DeepSeek-TUI源码职责分析.md` 是否包含至少 8 条维护风险或检查项
- [X] T025 [P] [US3] 执行 `git submodule status --recursive` 并确认 `docs/DeepSeek-TUI源码职责分析.md` 包含子模块版本不匹配的识别方式

### 实现

- [X] T026 [US3] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写 Windows 与维护注意事项，覆盖子模块版本、`Cargo.lock`、Rust 工具链和 release exe 进程占用
- [X] T027 [US3] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写用户目录路径、session/artifact/settings 数据边界和打包产物路径注意事项
- [X] T028 [US3] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写按问题类型排查章节，覆盖白屏/闪退、编译失败、会话异常、工具不可用、技能不可用、工作流异常、模型配置异常
- [X] T029 [US3] 在 `docs/DeepSeek-TUI源码职责分析.md` 中编写合并后冒烟检查步骤，包含 `cargo build --release --manifest-path pinvou3-app/src-tauri/Cargo.toml` 和 release exe 启动观察

**检查点**：US3 可独立验证，文档能支持后续维护风险排查。

---

## Phase 6: 收尾与横切关注点

- [X] T030 [P] 对照 `specs/003-deepseek-tui-analysis/contracts/analysis-document.md` 检查 `docs/DeepSeek-TUI源码职责分析.md` 的必备章节完整性
- [X] T031 [P] 对照 `specs/003-deepseek-tui-analysis/quickstart.md` 执行文档验收清单并在 `docs/DeepSeek-TUI源码职责分析.md` 中更新验收清单
- [X] T032 [P] 使用 `rg "NEEDS CLARIFICATION|\\[FEATURE|\\[###|TODO" docs/DeepSeek-TUI源码职责分析.md specs/003-deepseek-tui-analysis` 检查文档占位符
- [X] T033 执行 `git status --short` 并确认除 `docs/DeepSeek-TUI源码职责分析.md` 和 `specs/003-deepseek-tui-analysis/` 外无业务代码改动

## 实施记录

- T003：执行 `git status --short --branch`，确认本 feature 实施前已有 `.specify/feature.json`、`AGENTS.md` 和 `specs/003-deepseek-tui-analysis/` 变更。实现阶段允许新增/更新 `docs/DeepSeek-TUI源码职责分析.md` 与 `specs/003-deepseek-tui-analysis/tasks.md`，不修改业务代码。
- T004：目标文档 `docs/DeepSeek-TUI源码职责分析.md` 实施前不存在，处理策略为新建中文维护文档。
- T025：`git submodule status --recursive` 显示 `DeepSeek-TUI` 当前检出 `1161bc786d85e56e07d8526a1af657fcac170cbe`，未见 `-` 或 `+` 前缀。
- T032：占位符检查以最终文档和核心规格工件为准，排除 `tasks.md` 自身命令文本造成的匹配。
- T033：最终确认仅涉及文档/Spec Kit 工件与既有流程文件变更；未修改 `pinvou3-app/` 或 `DeepSeek-TUI/` 业务源码。

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 依赖 Phase 1，且阻塞所有用户故事。
- US1 是 MVP，建议先完成；US2 和 US3 在 Phase 2 后可并行，但最终文档需统一整合。
- Phase 6 依赖 US1、US2、US3 完成。

## 并行机会

- T003、T004 可并行，因为只读取状态并记录策略。
- T006、T007 可并行，因为分别扫描 pinvou3-app 接入点和项目规则。
- US1 中 T009、T010 可并行验证，T012、T013 可与 T014 在同一文档不同章节分工推进，但合并时需统一术语。
- US2 中 T016、T017 可并行验证；T018 到 T023 可由不同人员按调用链章节并行编写。
- US3 中 T024、T025 可并行验证；T026、T027、T028 可按风险类别并行编写。
- T030、T031、T032 可并行执行，T033 应最后执行。

## 实施策略

1. 先完成 Phase 1 和 Phase 2，建立证据清单和文档骨架。
2. 先交付 US1 作为 MVP，让维护者获得底座职责全景。
3. 继续交付 US2，把全景扩展为可追踪调用链。
4. 最后交付 US3，沉淀 Windows 与长期维护风险。
5. 每完成一个用户故事就按该故事的独立测试标准验证，不把所有检查堆到最后。
