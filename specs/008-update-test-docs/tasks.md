# 任务：Windows 全功能测试参考清单

**输入**：`specs/008-update-test-docs/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/windows-app-test-contract.md`、`quickstart.md`

**测试**：本 feature 是 QA 文档交付，不修改运行时代码。任务中的“验证”指文档覆盖度、可执行性和与当前 Windows UI 行为的一致性检查。

**组织方式**：任务按用户故事分组，保证每个测试域可以独立补充、审阅和交付。

## Phase 1: 准备（共享基础）

**目的**：确认当前 feature 目录、设计文档和源码参考边界。

- [X] T001 阅读并确认 Windows 全功能测试参考的范围在 `specs/008-update-test-docs/plan.md`
- [X] T002 阅读并确认 9 个用户故事、功能需求和成功标准在 `specs/008-update-test-docs/spec.md`
- [X] T003 [P] 阅读 UI 合约并记录需要映射到任务的页面入口在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T004 [P] 阅读 quickstart 并记录冒烟流程与专项流程在 `specs/008-update-test-docs/quickstart.md`

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立文档交付的一致结构、源码事实来源和最终验收口径。

**关键要求**：本阶段完成前，不应开始拆分单个用户故事的文档任务。

- [X] T005 对照当前 Tauri 命令注册清单校验功能域边界在 `pinvou3-app/src-tauri/src/lib.rs`
- [X] T006 对照当前前端页面结构校验主要 UI 入口在 `pinvou3-app/src/index.html`
- [X] T007 对照当前前端状态与命令调用校验跨页面交互来源在 `pinvou3-app/src/tauri-bridge.js`
- [X] T008 [P] 建立实体到用户故事的映射说明在 `specs/008-update-test-docs/data-model.md`
- [X] T009 定义最终文档质量检查项和覆盖标准在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：源码参考、实体映射、UI 合约和质量清单已统一，后续故事可并行补充。

---

## Phase 3: 用户故事 1 - 启动应用并完成基础导航 (Priority: P1) MVP

**目标**：测试人员能独立验证 Windows 安装后启动、主窗口渲染和主要页面导航。

**独立测试**：安装并启动 Windows 应用，逐一进入聊天、监控、设置、工作流、专家卡、产物和工具市场页面，确认无白屏、崩溃或阻塞弹窗。

### 验证 / 文档

- [X] T010 [P] [US1] 补充启动入口、主窗口和页面导航验收项在 `specs/008-update-test-docs/spec.md`
- [X] T011 [P] [US1] 补充启动与导航 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T012 [US1] 补充 Windows 启动冒烟步骤和路径风险检查在 `specs/008-update-test-docs/quickstart.md`
- [X] T013 [US1] 校验 US1 覆盖 FR-001、FR-002、SC-001 和 Windows 路径边界在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US1 可单独交付给测试人员执行启动和导航冒烟。

---

## Phase 4: 用户故事 2 - 管理聊天会话并完成对话 (Priority: P1)

**目标**：测试人员能验证会话生命周期、消息发送、流式回复、取消生成和多会话状态隔离。

**独立测试**：准备可用模型服务，创建多个会话并执行发送、取消、切换、重命名、删除和重启恢复。

### 验证 / 文档

- [X] T014 [P] [US2] 补充会话管理和聊天生成验收项在 `specs/008-update-test-docs/spec.md`
- [X] T015 [P] [US2] 补充聊天与会话 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T016 [US2] 补充会话、消息和聊天项实体说明在 `specs/008-update-test-docs/data-model.md`
- [X] T017 [US2] 补充聊天冒烟和多会话切换步骤在 `specs/008-update-test-docs/quickstart.md`
- [X] T018 [US2] 校验 US2 覆盖 FR-003、FR-004、FR-005 和 SC-002 在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US2 可单独验证核心聊天和会话体验。

---

## Phase 5: 用户故事 3 - 配置模型、语言、主题与系统能力 (Priority: P1)

**目标**：测试人员能验证设置页配置保存、后端状态、本地服务探测、语言主题、权限、依赖和版本状态。

**独立测试**：分别使用默认配置、错误配置和有效配置保存设置，并重启应用确认生效。

### 验证 / 文档

- [X] T019 [P] [US3] 补充设置页和模型状态验收项在 `specs/008-update-test-docs/spec.md`
- [X] T020 [P] [US3] 补充设置与模型状态 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T021 [US3] 补充设置和依赖状态实体说明在 `specs/008-update-test-docs/data-model.md`
- [X] T022 [US3] 补充设置保存、语言主题和后端状态冒烟步骤在 `specs/008-update-test-docs/quickstart.md`
- [X] T023 [US3] 校验 US3 覆盖 FR-006、FR-007、FR-025、FR-026 和 SC-003 在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US3 可单独验证设置页和 Windows 系统能力入口。

---

## Phase 6: 用户故事 4 - 添加附件并查看产物 (Priority: P1)

**目标**：测试人员能验证附件添加、粘贴图片、依赖缺失反馈、产物预览和 Windows 系统打开。

**独立测试**：使用普通文档、图片、压缩包和中文路径文件执行附件上传和产物查看流程。

### 验证 / 文档

- [X] T024 [P] [US4] 补充附件和产物验收项在 `specs/008-update-test-docs/spec.md`
- [X] T025 [P] [US4] 补充附件与产物 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T026 [US4] 补充附件和产物实体说明在 `specs/008-update-test-docs/data-model.md`
- [X] T027 [US4] 补充文件选择、拖拽、粘贴图片和产物打开步骤在 `specs/008-update-test-docs/quickstart.md`
- [X] T028 [US4] 校验 US4 覆盖 FR-008、FR-009、FR-010、FR-020、SC-004 和 SC-005 在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US4 可单独验证文件输入、产物输出和 Windows 文件集成。

---

## Phase 7: 用户故事 5 - 使用 Plan 模式、工具调用和用户输入卡片 (Priority: P2)

**目标**：测试人员能验证 Plan 模式、工具卡片、工具阻塞、用户输入请求和上下文压缩提示。

**独立测试**：构造需要计划、工具调用、用户确认和上下文压缩的对话，逐项验证卡片状态与按钮行为。

### 验证 / 文档

- [X] T029 [P] [US5] 补充 Plan、工具和用户输入卡片验收项在 `specs/008-update-test-docs/spec.md`
- [X] T030 [P] [US5] 补充 Plan 与工具卡片 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T031 [US5] 补充消息和聊天项中的卡片类型说明在 `specs/008-update-test-docs/data-model.md`
- [X] T032 [US5] 校验 US5 覆盖 FR-011、FR-012 和 SC-006 在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US5 可单独验证复杂对话控制卡片。

---

## Phase 8: 用户故事 6 - 管理专家卡、技能和会话绑定 (Priority: P2)

**目标**：测试人员能验证专家卡池、用户自定义卡片、技能详情和会话绑定持久化。

**独立测试**：使用内置卡片和自定义卡片分别执行浏览、绑定、卸下和重启恢复操作。

### 验证 / 文档

- [X] T033 [P] [US6] 补充专家卡、技能和会话绑定验收项在 `specs/008-update-test-docs/spec.md`
- [X] T034 [P] [US6] 补充专家卡与技能 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T035 [US6] 补充专家卡和技能绑定实体说明在 `specs/008-update-test-docs/data-model.md`
- [X] T036 [US6] 补充专家卡筛选、装备、卸下和自定义卡片步骤在 `specs/008-update-test-docs/quickstart.md`
- [X] T037 [US6] 校验 US6 覆盖 FR-013、FR-014、FR-015 和 SC-006 在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US6 可单独验证角色能力和技能绑定体验。

---

## Phase 9: 用户故事 7 - 执行工作流任务 (Priority: P2)

**目标**：测试人员能验证工作流模板、任务创建、材料追加、角色状态、门禁审批、重试和取消。

**独立测试**：选择一个可执行工作流模板，创建任务并覆盖成功、失败、门禁审批和取消路径。

### 验证 / 文档

- [X] T038 [P] [US7] 补充工作流任务验收项在 `specs/008-update-test-docs/spec.md`
- [X] T039 [P] [US7] 补充工作流 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T040 [US7] 补充工作流运行实体说明在 `specs/008-update-test-docs/data-model.md`
- [X] T041 [US7] 补充工作流模板、启动、门禁和角色详情步骤在 `specs/008-update-test-docs/quickstart.md`
- [X] T042 [US7] 校验 US7 覆盖 FR-016、FR-017 和 SC-006 在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US7 可单独验证多角色工作流和长任务状态。

---

## Phase 10: 用户故事 8 - 查看监控、工具市场和系统集成状态 (Priority: P2)

**目标**：测试人员能验证监控降级、工具市场安装卸载和 Windows 系统打开行为。

**独立测试**：在有无模型服务、有无 GPU、有无网络的环境下查看状态，并对市场工具执行安装和卸载。

### 验证 / 文档

- [X] T043 [P] [US8] 补充监控、工具市场和系统集成验收项在 `specs/008-update-test-docs/spec.md`
- [X] T044 [P] [US8] 补充监控与工具市场 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T045 [US8] 补充监控快照和工具市场项实体说明在 `specs/008-update-test-docs/data-model.md`
- [X] T046 [US8] 校验 US8 覆盖 FR-018、FR-019、FR-020、SC-007 和 Windows 系统打开边界在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US8 可单独验证诊断、扩展和系统集成能力。

---

## Phase 11: 用户故事 9 - 完成 Windows 更新、安装包和依赖验证 (Priority: P1)

**目标**：测试人员能验证检查更新、下载、取消、MSI 提权安装、安装后启动、升级反馈和依赖检查。

**独立测试**：使用 0.4.4 到 0.4.5 的升级包或等价测试包，在真实 Windows 环境执行完整升级和失败场景。

### 验证 / 文档

- [X] T047 [P] [US9] 补充 Windows 更新和依赖验证验收项在 `specs/008-update-test-docs/spec.md`
- [X] T048 [P] [US9] 补充 Windows 更新与依赖 UI 合约在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T049 [US9] 补充更新任务和依赖状态实体说明在 `specs/008-update-test-docs/data-model.md`
- [X] T050 [US9] 补充 Windows 更新专项步骤和 UAC 失败路径在 `specs/008-update-test-docs/quickstart.md`
- [X] T051 [US9] 对照 Windows 更新实现校验 MSI、提权和反馈行为来源在 `pinvou3-app/src-tauri/src/os/windows/windows_update.rs`
- [X] T052 [US9] 校验 US9 覆盖 FR-021、FR-022、FR-023、FR-024、FR-025、SC-007 和 SC-008 在 `specs/008-update-test-docs/checklists/requirements.md`

**检查点**：US9 可单独验证 Windows 更新和依赖检查交付质量。

---

## Phase 12: 收尾与横切关注点

- [X] T053 [P] 统一中文术语、优先级和功能域命名在 `specs/008-update-test-docs/spec.md`
- [X] T054 [P] 统一 UI 合约中的入口、操作、可见状态和失败反馈格式在 `specs/008-update-test-docs/contracts/windows-app-test-contract.md`
- [X] T055 [P] 统一 quickstart 的冒烟流程、Windows 专项检查和输出建议在 `specs/008-update-test-docs/quickstart.md`
- [X] T056 运行任务格式检查并修复不符合 `- [X] T### [P?] [US?] ...路径` 的条目在 `specs/008-update-test-docs/tasks.md`
- [X] T057 复核所有成功标准均能由至少一个用户故事或 quickstart 步骤验证在 `specs/008-update-test-docs/spec.md`
- [X] T058 记录未覆盖或需要真实 Windows 环境验证的剩余风险在 `specs/008-update-test-docs/checklists/requirements.md`

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 依赖 Phase 1，且阻塞所有用户故事。
- P1 用户故事建议顺序：US1 -> US2 -> US3 -> US4 -> US9。
- P2 用户故事可在 Phase 2 后并行：US5、US6、US7、US8。
- Phase 12 依赖所有计划内用户故事完成。

## 用户故事依赖图

```text
Phase 1 准备
  -> Phase 2 基础任务
      -> US1 启动与导航 (MVP)
      -> US2 聊天与会话
      -> US3 设置与模型状态
      -> US4 附件与产物
      -> US9 Windows 更新与依赖
      -> US5 Plan/工具/用户输入
      -> US6 专家卡/技能
      -> US7 工作流
      -> US8 监控/市场/系统集成
          -> Phase 12 收尾
```

## 并行机会

- US1：T010 与 T011 可并行，T012 在二者之后整合。
- US2：T014 与 T015 可并行，T016 可由数据模型维护者并行完成。
- US3：T019 与 T020 可并行，T021 可并行补充实体说明。
- US4：T024 与 T025 可并行，T026 可并行补充实体说明。
- US5：T029 与 T030 可并行，T031 可并行补充卡片类型。
- US6：T033 与 T034 可并行，T035 可并行补充绑定实体。
- US7：T038 与 T039 可并行，T040 可并行补充工作流实体。
- US8：T043 与 T044 可并行，T045 可并行补充监控和市场实体。
- US9：T047 与 T048 可并行，T051 可由熟悉 Windows 更新实现的人并行校验。
- 收尾：T053、T054、T055 可并行，T056-T058 需在其后执行。

## 实施策略

1. 先完成 Phase 1、Phase 2 和 US1，形成最小可交付的 Windows 启动与导航测试参考。
2. 继续完成 P1 故事 US2、US3、US4、US9，覆盖核心使用和交付质量。
3. 并行补充 P2 故事 US5、US6、US7、US8，覆盖复杂交互和扩展能力。
4. 最后执行 Phase 12，统一术语、格式、覆盖矩阵和剩余风险。

## MVP 范围

MVP 为 Phase 1、Phase 2 和 US1。若要形成可交给测试的首版 Windows 回归清单，建议至少包含所有 P1 故事：US1、US2、US3、US4、US9。
