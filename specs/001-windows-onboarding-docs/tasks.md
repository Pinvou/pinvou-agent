# 任务：Windows 迁移与接手维护文档

**输入**：`/specs/001-windows-onboarding-docs/` 下的设计文档

**前置条件**：plan.md、spec.md、research.md、data-model.md、contracts/documentation-contract.md、quickstart.md

**测试**：本 feature 是文档交付，不要求新增自动化测试；任务中包含文档契约验证、占位扫描和人工验收清单。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3。
- 每个任务描述都包含精确文件路径。

## Phase 1: 准备（共享基础）

**目的**：确认文档 feature 的当前产物、宪章门禁和验证入口。

- [X] T001 阅读 `specs/001-windows-onboarding-docs/spec.md`，提取 P1/P2/P3 用户故事和验收标准
- [X] T002 阅读 `specs/001-windows-onboarding-docs/plan.md`，确认宪章检查、项目结构和文档范围
- [X] T003 [P] 阅读 `specs/001-windows-onboarding-docs/contracts/documentation-contract.md`，整理文档必须覆盖的章节清单
- [X] T004 [P] 阅读 `docs/Windows迁移与维护接手手册.md`，标记与 contract、spec 成功标准不一致的段落

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：统一语言、契约和验证基线，避免后续用户故事在不同标准下更新文档。

**CRITICAL**：本阶段完成前，不应开始用户故事正文更新。

- [X] T005 将 `specs/001-windows-onboarding-docs/research.md` 中可中文表达的标题、说明、决策、理由和备选方案改为中文
- [X] T006 将 `specs/001-windows-onboarding-docs/data-model.md` 中可中文表达的实体、字段、关系和验证规则改为中文
- [X] T007 将 `specs/001-windows-onboarding-docs/contracts/documentation-contract.md` 中可中文表达的契约标题、规则和验收项改为中文
- [X] T008 将 `specs/001-windows-onboarding-docs/quickstart.md` 中可中文表达的操作说明和预期结果改为中文
- [X] T009 在 `specs/001-windows-onboarding-docs/quickstart.md` 中补充按宪章验证中文文档优先的扫描命令

**检查点**：Spec Kit 设计产物语言一致，后续用户故事可以独立更新主文档。

---

## Phase 3: 用户故事 1 - 快速理解项目全貌 (Priority: P1)

**目标**：让新接手的 Windows 应用工程师能快速理解项目定位、目录职责、调用流程、底座边界和数据目录。

**独立测试**：工程师阅读 `docs/Windows迁移与维护接手手册.md` 后，能说明 pinvou3-app、DeepSeek-TUI、vLLM、前端 bridge、Rust command、EnginePool、session/workflow 的关系，并指出哪些能力不能在 pinvou3 重造。

### 验证

- [X] T010 [P] [US1] 对照 `specs/001-windows-onboarding-docs/contracts/documentation-contract.md` 检查 `docs/Windows迁移与维护接手手册.md` 是否覆盖项目定位、仓库结构、聊天链路、前端事件和后端启动流程
- [X] T011 [P] [US1] 对照 `AGENTS.md` 检查 `docs/Windows迁移与维护接手手册.md` 是否完整复述 DeepSeek-TUI 底座边界和推荐扩展点

### 实现

- [X] T012 [US1] 更新 `docs/Windows迁移与维护接手手册.md` 的“一句话定位”和“当前仓库结构”章节，确保 `pinvou3-app/`、`DeepSeek-TUI/`、`docs/`、`workflows/`、`scripts/` 职责清晰
- [X] T013 [US1] 更新 `docs/Windows迁移与维护接手手册.md` 的“主调用流程”章节，补齐从 `tauri-bridge.js` 到 `commands.rs`、`engine_pool.rs`、`engine.rs`、DeepSeek-TUI、vLLM、`chat:*` 事件回传的顺序
- [X] T014 [US1] 更新 `docs/Windows迁移与维护接手手册.md` 的“前端通信与事件”章节，明确多 session 事件按 `session_id` 分流的维护意义
- [X] T015 [US1] 更新 `docs/Windows迁移与维护接手手册.md` 的“配置、目录与持久化”章节，列清 `~/.pinvou3/` 下 settings、bundle、sessions、workspace、artifacts、workflows、updates 的用途
- [X] T016 [US1] 在 `docs/Windows迁移与维护接手手册.md` 中增加 US1 人工验收清单，问题覆盖至少 8 个核心模块职责和底座不可重造能力

**检查点**：US1 可独立验收；读者不需要阅读源码也能复述主架构和调用链。

---

## Phase 4: 用户故事 2 - 识别 Windows 迁移工作面 (Priority: P2)

**目标**：让 Windows 工程师能从文档中列出当前 Linux 偏置、系统依赖、打包更新、路径和本地模型相关迁移风险。

**独立测试**：工程师阅读 `docs/Windows迁移与维护接手手册.md` 后，能够列出至少 10 项 Windows 迁移风险，并为每项标出涉及模块和建议处理方向。

### 验证

- [X] T017 [P] [US2] 对照 `specs/001-windows-onboarding-docs/contracts/documentation-contract.md` 检查 `docs/Windows迁移与维护接手手册.md` 的 Windows 风险表是否至少包含 10 项风险
- [X] T018 [P] [US2] 对照 `pinvou3-app/src-tauri/src/file_ingest.rs` 检查 `docs/Windows迁移与维护接手手册.md` 是否覆盖 `which`、`python3`、`soffice`、`pdftotext`、`tesseract`、`7z`、`pkexec apt` 风险

### 实现

- [X] T019 [US2] 更新 `docs/Windows迁移与维护接手手册.md` 的“附件 ingestion 流程”章节，明确每类附件依赖的外部工具及 Windows 处理方向
- [X] T020 [US2] 更新 `docs/Windows迁移与维护接手手册.md` 的“安装、更新、打包现状”章节，明确 `.deb`、`apt`、`pkexec`、WebKitGTK、WebView2、Windows installer 的迁移分歧
- [X] T021 [US2] 更新 `docs/Windows迁移与维护接手手册.md` 的“Windows 迁移风险清单”，确保 P0 风险包含 submodule、bundle target、`HOME`、`which`、`pkexec/apt`、`.deb updater`
- [X] T022 [US2] 更新 `docs/Windows迁移与维护接手手册.md` 的“本地模型与 256K 约束”章节，说明 `qwen36_35b_256k`、`DEEPSEEK_BASE_URL`、远程 GB10/vLLM 和 Windows 网络/防火墙注意点
- [X] T023 [US2] 在 `docs/Windows迁移与维护接手手册.md` 中增加 US2 人工验收清单，要求读者能输出至少 10 项迁移风险和对应模块

**检查点**：US2 可独立验收；读者可以拿风险表直接进入 Windows 迁移排期。

---

## Phase 5: 用户故事 3 - 支撑后续迭代维护 (Priority: P3)

**目标**：让工程师能通过文档定位常见维护需求的修改入口、验证方式、fork 同步注意事项和已下线方案。

**独立测试**：工程师收到新增领域 agent、接外部 API、同步 DeepSeek-TUI、修改附件解析、迁移打包更新等需求时，能在 5 分钟内定位推荐修改入口和验证方式。

### 验证

- [X] T024 [P] [US3] 对照 `docs/fork-modifications.md` 检查 `docs/Windows迁移与维护接手手册.md` 是否覆盖 fork 主题、fork guard、system prompt diff、工具集合盘点和动态工具激活风险
- [X] T025 [P] [US3] 对照 `process.md` 检查 `docs/Windows迁移与维护接手手册.md` 是否标注品悟 v2、多 subagent fan-out、Plan/YOLO、h3c-ppt phased skill 等已下线或搁置状态

### 实现

- [X] T026 [US3] 更新 `docs/Windows迁移与维护接手手册.md` 的“常见维护任务该改哪里”表，覆盖 SKILL、slash command、MCP、settings、UI、session、附件、workflow、fork 同步、底座 bug 修复
- [X] T027 [US3] 更新 `docs/Windows迁移与维护接手手册.md` 的“验证建议”章节，区分文档验证、Windows 迁移前基线验证、Rust 层验证、底座 fork 验证和 Windows 原生 smoke
- [X] T028 [US3] 更新 `docs/Windows迁移与维护接手手册.md` 的“已下线或易误读的历史方案”章节，确保不把 archived 方案描述为推荐实现路径
- [X] T029 [US3] 在 `docs/Windows迁移与维护接手手册.md` 中增加 US3 人工验收清单，要求对 5 类维护需求给出入口文件和验证方式

**检查点**：US3 可独立验收；文档能作为后续维护导航使用。

---

## Phase 6: 收尾与横切关注点

**目的**：统一格式、执行验证、同步 Spec Kit 状态。

- [X] T030 [P] 运行占位扫描命令并根据结果修复 `specs/001-windows-onboarding-docs/` 下的未解决模板残留
- [X] T031 [P] 运行占位扫描命令并根据结果修复 `docs/Windows迁移与维护接手手册.md` 下的未解决模板残留
- [X] T032 对照 `.specify/memory/constitution.md` 检查 `docs/Windows迁移与维护接手手册.md`、`specs/001-windows-onboarding-docs/research.md`、`specs/001-windows-onboarding-docs/data-model.md`、`specs/001-windows-onboarding-docs/contracts/documentation-contract.md`、`specs/001-windows-onboarding-docs/quickstart.md` 是否满足中文文档优先
- [X] T033 对照 `specs/001-windows-onboarding-docs/contracts/documentation-contract.md` 完成 `docs/Windows迁移与维护接手手册.md` 覆盖率复核并记录缺口
- [X] T034 更新 `specs/001-windows-onboarding-docs/checklists/requirements.md`，记录 tasks 阶段后的文档契约验证结果

---

## 依赖与执行顺序

### Phase 依赖

- **Phase 1 准备**：无依赖，可立即开始。
- **Phase 2 基础任务**：依赖 Phase 1，阻塞所有用户故事。
- **Phase 3 US1**：依赖 Phase 2，是 MVP 范围。
- **Phase 4 US2**：依赖 Phase 2，可在 US1 完成后推进，也可与 US1 并行但需避免同时改同一章节。
- **Phase 5 US3**：依赖 Phase 2，可在 US1/US2 后推进，也可与 US2 并行但需避免同一文件冲突。
- **Phase 6 收尾**：依赖计划内用户故事完成。

### 用户故事依赖

- **US1 快速理解项目全貌**：可独立完成，是 MVP。
- **US2 识别 Windows 迁移工作面**：可独立完成，但引用 US1 的架构基础会更清晰。
- **US3 支撑后续迭代维护**：可独立完成，但应复用 US1/US2 已建立的模块和风险命名。

### MVP 范围

MVP 建议只包含 Phase 1、Phase 2、Phase 3、Phase 6 中与 US1 相关的收尾验证。完成后读者应能理解项目全貌、调用链和底座边界。

---

## 并行机会

- T003 与 T004 可并行读取不同输入。
- T005、T006、T007、T008 可并行中文化不同 Spec Kit 文档；T009 依赖 T008。
- T010 与 T011 可并行验证不同来源。
- T017 与 T018 可并行验证 contract 与代码依赖。
- T024 与 T025 可并行验证 fork 文档和 process 文档。
- T030 与 T031 可并行扫描 specs 与 docs。

## 并行执行示例

### US1

```text
Task: T010 对照 documentation-contract.md 检查主文档覆盖范围
Task: T011 对照 AGENTS.md 检查底座边界与扩展点
```

### US2

```text
Task: T017 检查 Windows 风险表数量与字段
Task: T018 检查 file_ingest.rs 外部工具风险覆盖
```

### US3

```text
Task: T024 检查 fork 同步与 fork guard 覆盖
Task: T025 检查已下线方案状态覆盖
```

## 实施策略

1. 先完成 Phase 1 和 Phase 2，统一文档语言和验收标准。
2. 以 US1 作为 MVP，先保证新工程师能理解项目全貌和调用链。
3. 继续补 US2 的 Windows 迁移风险表，让后续实现排期有抓手。
4. 最后补 US3 的维护导航、fork 同步和历史方案状态。
5. 每个用户故事完成后立即按独立测试标准验收，不把所有验证推到最后。
