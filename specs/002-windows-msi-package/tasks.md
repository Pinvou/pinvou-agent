# 任务：Windows MSI 安装包构建

**输入**：`/specs/002-windows-msi-package/` 下的设计文档

**前置条件**：plan.md、spec.md、research.md、data-model.md、contracts/windows-msi-contract.md、quickstart.md

**测试**：本 feature 是桌面打包交付，验证以 Windows 构建前置检查、Tauri build、MSI 产物检查、安装 smoke、最小变更契约复核为主；不要求新增业务单元测试，除非实现阶段触碰运行时代码。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3。
- 描述中必须包含精确文件路径或可执行命令。
- 文档、任务描述和验收说明默认使用中文；英文仅保留必要命令、路径、API 字段或原文。

## Phase 1: 准备（共享基础）

**目的**：确认当前 MSI feature 上下文、构建边界和已有项目状态。

- [X] T001 阅读 `specs/002-windows-msi-package/spec.md`，提取 P1/P2/P3 用户故事、FR-001 至 FR-010 和 SC-001 至 SC-005
- [X] T002 阅读 `specs/002-windows-msi-package/plan.md`，确认宪章检查、技术上下文、范围排除项和目标平台
- [X] T003 [P] 阅读 `specs/002-windows-msi-package/contracts/windows-msi-contract.md`，整理构建前置、MSI 产物、安装验收和最小变更契约清单
- [X] T004 [P] 阅读 `specs/002-windows-msi-package/quickstart.md`，确认 Windows 构建命令、产物查找路径和 smoke 验收步骤
- [X] T005 检查当前工作树状态，运行 `git status --short --branch`，记录与本 feature 相关的未提交变更，避免覆盖用户修改

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立 Windows MSI 构建所需的前置判断、配置策略和验证记录骨架。

**CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T006 对照 `pinvou3-app/src-tauri/tauri.conf.json`、`pinvou3-app/package.json` 和 `pinvou3-app/src-tauri/Cargo.toml` 检查产品名、版本、identifier、icon 和当前 bundle target
- [X] T007 对照 `DeepSeek-TUI/` 和 `pinvou3-app/src-tauri/Cargo.toml` 检查 submodule/path dependency 是否可解析，并记录在 `specs/002-windows-msi-package/msi-build-report.md`
- [X] T008 [P] 在 `specs/002-windows-msi-package/msi-build-report.md` 创建 Windows 构建环境记录模板，包含 Rust、Cargo、Node、npm、Tauri CLI、WebView2、WiX/MSI、VBSCRIPT 和 submodule 状态
- [X] T009 [P] 在 `specs/002-windows-msi-package/minimal-change-record.md` 创建最小变更清单模板，字段覆盖 changed_file、change_type、reason、risk、verification、out_of_scope_note
- [X] T010 评估 `pinvou3-app/src-tauri/tauri.conf.json` 是否能通过命令行显式 `--bundles msi` 生成 MSI；若不能，确定是否需要最小配置改动或 Windows 专用配置覆盖文件

**检查点**：构建前置、记录文件和配置策略明确，可以开始按用户故事实施。

---

## Phase 3: 用户故事 1 - 生成可安装的 Windows MSI 包 (Priority: P1) MVP

**目标**：在 Windows 构建环境中生成 `.msi` 安装包，并能完成基础安装。

**独立测试**：执行 Windows 打包命令后，在 `pinvou3-app/src-tauri/target/release/bundle/msi/` 或记录的输出目录找到非空 `.msi` 文件，并能在 Windows 机器上完成安装。

### 测试 / 验证

- [X] T011 [P] [US1] 在 Windows 构建机运行 `git submodule update --init --recursive` 并记录结果到 `specs/002-windows-msi-package/msi-build-report.md`
- [X] T012 [P] [US1] 在 Windows 构建机运行 `rustc --version`、`cargo --version`、`node --version`、`npm --version` 并记录结果到 `specs/002-windows-msi-package/msi-build-report.md`
- [X] T013 [P] [US1] 在 `pinvou3-app/` 运行 `npm install` 并记录是否生成可用 `node_modules` 和 Tauri CLI 到 `specs/002-windows-msi-package/msi-build-report.md`
- [X] T014 [US1] 在 `pinvou3-app/` 运行 `npm run tauri build -- --bundles msi` 或实现阶段确认的等效命令，并记录完整命令、成功/失败摘要到 `specs/002-windows-msi-package/msi-build-report.md`

### 实现

- [X] T015 [US1] 若 T014 因 bundle target 限制失败，在 `pinvou3-app/src-tauri/tauri.conf.json` 或 Windows 专用 Tauri 配置覆盖文件中做最小 MSI target 调整，并在 `specs/002-windows-msi-package/minimal-change-record.md` 记录原因
- [X] T016 [US1] 若 T014 因 Windows MSI/WiX/VBSCRIPT 前置缺失失败，在 `specs/002-windows-msi-package/msi-build-report.md` 记录缺失项、失败日志摘要和补齐路径
- [X] T017 [US1] 查找 `pinvou3-app/src-tauri/target/release/bundle/msi/*.msi`，验证 MSI 文件存在且大小大于 0，并把产物路径写入 `specs/002-windows-msi-package/msi-build-report.md`
- [X] T018 [US1] 在 Windows 机器上安装 T017 生成的 MSI，记录安装结果、开始菜单或安装目录入口到 `specs/002-windows-msi-package/msi-build-report.md`

**检查点**：US1 可独立演示：存在可追踪的 MSI 产物，或存在明确阻塞原因和补齐路径。

---

## Phase 4: 用户故事 2 - 保持现有代码行为最小变更 (Priority: P2)

**目标**：确保 MSI 打包没有重写聊天、agent、session、工具执行、MCP、workflow 或 DeepSeek-TUI 底座行为。

**独立测试**：审查 `git diff --stat`、`git diff --name-only` 和最小变更清单，确认变更集中在打包配置、构建说明、验证记录或必要启动兼容范围。

### 测试 / 验证

- [X] T019 [P] [US2] 运行 `git diff --stat` 和 `git diff --name-only`，把变更文件列表整理到 `specs/002-windows-msi-package/minimal-change-record.md`
- [X] T020 [P] [US2] 对照 `AGENTS.md` 和 `.specify/memory/constitution.md` 检查本 feature 是否触碰 DeepSeek-TUI 底座禁区，并把结论写入 `specs/002-windows-msi-package/minimal-change-record.md`
- [X] T021 [P] [US2] 对照 `docs/Windows迁移与维护接手手册.md` 检查 `.deb`、`apt`、`pkexec`、updater、附件外部工具等 Windows 暂不纳入项是否未被伪装为已完成能力

### 实现

- [X] T022 [US2] 在 `specs/002-windows-msi-package/minimal-change-record.md` 为每个实际修改文件填写 change_type、reason、risk 和 verification
- [X] T023 [US2] 如实现阶段修改了 `pinvou3-app/src-tauri/src/lib.rs`、`pinvou3-app/src-tauri/src/updater.rs`、`pinvou3-app/src-tauri/src/file_ingest.rs` 或 `DeepSeek-TUI/` 下文件，必须在 `specs/002-windows-msi-package/minimal-change-record.md` 逐项说明必要性；否则记录“未触碰运行时代码/底座”
- [X] T024 [US2] 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`，若未运行或失败则在 `specs/002-windows-msi-package/msi-build-report.md` 记录原因和补验路径

**检查点**：US2 可独立验收：变更范围清晰，底座和聊天主链路未被重写，任何必要代码改动都有理由和验证。

---

## Phase 5: 用户故事 3 - 形成可重复的 Windows 打包交接路径 (Priority: P3)

**目标**：让后续维护工程师无需历史对话即可复现 Windows MSI 构建、定位产物、执行安装 smoke 并理解已知限制。

**独立测试**：另一名工程师按 `specs/002-windows-msi-package/quickstart.md` 和 `msi-build-report.md` 复现流程，能得到 MSI 或明确阻塞点。

### 测试 / 验证

- [X] T025 [P] [US3] 对照 `specs/002-windows-msi-package/contracts/windows-msi-contract.md` 检查 `specs/002-windows-msi-package/msi-build-report.md` 是否覆盖构建命令、产物路径、失败摘要和补齐路径
- [X] T026 [P] [US3] 对照 `specs/002-windows-msi-package/contracts/windows-msi-contract.md` 检查 `specs/002-windows-msi-package/msi-build-report.md` 是否覆盖安装、启动、配置入口、卸载和用户数据保留 5 类 smoke

### 实现

- [X] T027 [US3] 更新 `specs/002-windows-msi-package/quickstart.md`，把实现阶段确认的实际 MSI 构建命令、输出目录和前置依赖补齐
- [X] T028 [US3] 在 `specs/002-windows-msi-package/msi-build-report.md` 增加“已知限制”章节，明确代码签名、Windows 原生 updater、附件外部工具全量适配、企业分发策略是否不在本 feature 范围
- [X] T029 [US3] 在 `specs/002-windows-msi-package/msi-build-report.md` 增加“复现步骤”章节，列出从克隆/submodule 到 npm install、构建 MSI、安装 smoke 的完整顺序

**检查点**：US3 可独立验收：构建和安装交接材料完整，后续维护者能复现或定位阻塞。

---

## Phase 6: 收尾与横切关注点

**目的**：统一格式、执行最终验证、同步 Spec Kit 状态。

- [X] T030 [P] 运行占位符扫描 `$patterns=@('NEEDS'+' CLARIFICATION','['+'FEATURE NAME'+']','$'+'ARGUMENTS','ACTION'+' REQUIRED','REMOVE'+' IF UNUSED','T'+'XXX'); rg -n (($patterns | ForEach-Object {[regex]::Escape($_)}) -join '|') specs\\002-windows-msi-package AGENTS.md` 并修复未解决模板残留
- [X] T031 [P] 对照 `.specify/memory/constitution.md` 检查 `specs/002-windows-msi-package/tasks.md`、`quickstart.md`、`msi-build-report.md`、`minimal-change-record.md` 是否满足中文文档优先
- [X] T032 对照 `specs/002-windows-msi-package/contracts/windows-msi-contract.md` 复核 `specs/002-windows-msi-package/msi-build-report.md` 和 `minimal-change-record.md` 覆盖率，记录缺口或确认无缺口
- [X] T033 运行 `rg -n "DeepSeek-TUI|Engine|ToolRegistry|SSE|Session|SkillRegistry|Commands|MCP|Hooks|Cycle|Compaction" specs\\002-windows-msi-package\\minimal-change-record.md`，确认未出现底座重写说明；如出现则逐项解释
- [X] T034 更新 `specs/002-windows-msi-package/checklists/requirements.md`，记录 tasks 阶段后构建契约、最小变更和 Windows smoke 验证结果

---

## 依赖与执行顺序

### Phase 依赖

- **Phase 1 准备**：无依赖。
- **Phase 2 基础任务**：依赖 Phase 1，阻塞所有用户故事。
- **Phase 3 US1**：依赖 Phase 2，是 MVP 范围。
- **Phase 4 US2**：依赖 Phase 2，可在 US1 构建尝试后推进，最终需读取 US1 产生的变更和报告。
- **Phase 5 US3**：依赖 Phase 3 和 Phase 4 的实际结果。
- **Phase 6 收尾**：依赖所有计划内用户故事完成。

### 用户故事依赖

- **US1 生成 MSI**：可独立交付核心产物或阻塞原因，是 MVP。
- **US2 最小变更**：可独立审查变更范围，但需要读取 US1 实际构建改动。
- **US3 可重复交接**：依赖 US1/US2 的真实命令、报告和变更结论。

### MVP 范围

MVP 建议包含 Phase 1、Phase 2、Phase 3 和 Phase 6 中与 US1 直接相关的收尾验证。完成后至少应有 `.msi` 产物，或明确记录当前环境无法生成 MSI 的具体阻塞原因。

## 并行机会

- T003 与 T004 可并行读取不同设计产物。
- T008 与 T009 可并行创建不同报告骨架。
- T011、T012、T013 可并行收集 Windows 构建环境信息。
- T019、T020、T021 可并行进行不同维度的变更范围审查。
- T025、T026 可并行检查构建契约和安装 smoke 覆盖。
- T030、T031 可并行做占位符扫描和中文优先检查。

## 并行执行示例

### US1

```text
Task: T011 在 Windows 构建机初始化 submodule 并记录结果
Task: T012 采集 Rust/Cargo/Node/npm 版本并记录结果
Task: T013 在 pinvou3-app/ 安装 npm 依赖并记录结果
```

### US2

```text
Task: T019 整理 git diff 变更范围
Task: T020 检查 DeepSeek-TUI 底座边界
Task: T021 检查 Linux-only 能力未被伪装为 Windows 已完成
```

### US3

```text
Task: T025 检查构建命令与产物路径契约
Task: T026 检查安装 smoke 五类结果契约
```

## 实施策略

1. 先完成 Phase 1 和 Phase 2，明确当前环境是否具备 Windows MSI 构建条件。
2. 以 US1 作为 MVP，优先尝试生成 MSI；若环境阻塞，先把阻塞原因记录清楚。
3. US2 紧随 US1，确保任何配置或代码改动都有最小变更说明。
4. US3 最后补齐可重复交接路径，让下一位维护者能按文档复现。
5. 每完成一个用户故事立即按独立测试标准验收，不把构建、安装、变更审查全部堆到最后。
