# Tasks: Windows 系统兼容性修复

**Input**: `specs/004-windows-compat-fixes/` 下的 `spec.md`、`plan.md`、`research.md`、`data-model.md`、`contracts/os-dispatch-contract.md`、`quickstart.md`
**Prerequisites**: 已存在首轮兼容性修复变更；本任务清单以“新增 OS 调度抽象层、最小迁移现有调用点”为目标

**Tests**: 本功能涉及跨平台运行行为，任务包含静态扫描、Rust 编译检查和平台降级验证；不要求新增完整自动化测试套件，但应优先补充低风险的单元/平台门控测试。

**Organization**: 任务按用户故事组织，优先交付 P1 的兼容性风险识别，再迁移高价值调用点，最后沉淀维护验证路径。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 可并行执行，且不修改同一文件
- **[Story]**: 用户故事标签，格式为 `[US1]`、`[US2]`、`[US3]`
- 每个任务都包含明确文件路径，便于直接定位修改范围

## Phase 1: Setup

**Purpose**: 确认当前工作区状态、现有首轮修复范围，以及 OS 调度抽象层的实施边界。

- [X] T001 阅读并确认 `specs/004-windows-compat-fixes/plan.md` 中的 OS 调度抽象方案，确保后续任务不扩大业务代码改动范围
- [X] T002 检查当前工作区变更并记录已存在的首轮修复文件清单到 `specs/004-windows-compat-fixes/compatibility-scan.md`
- [X] T003 [P] 复核 `pinvou3-app/src-tauri/src/commands.rs` 中当前系统打开逻辑，标记待迁移到 OS 调度层的代码段
- [X] T004 [P] 复核 `pinvou3-app/src-tauri/src/monitor.rs` 中当前 GPU 命令路径逻辑，标记待迁移到 OS 调度层的代码段
- [X] T005 [P] 复核 `pinvou3-app/src-tauri/src/file_ingest.rs` 中当前命令探测和依赖安装逻辑，标记待迁移到 OS 调度层的代码段
- [X] T006 [P] 复核 `pinvou3-app/src-tauri/src/bridge/paths.rs`、`pinvou3-app/src-tauri/src/super_permission.rs` 与 `pinvou3-app/src-tauri/src/updater.rs` 中当前路径兼容和 Linux 专属逻辑，标记待迁移到 OS 调度层的代码段

---

## Phase 2: Foundational

**Purpose**: 先建立平台调度边界，后续所有用户故事都应基于该边界迁移，避免继续在业务调用点分散增加 `cfg`。

- [X] T007 新增 `pinvou3-app/src-tauri/src/os/mod.rs`，定义并导出 `open_target`、`command_exists`、`nvidia_smi_candidates`、`user_home_dir`、`platform_compat_path`、`super_permission_*`、`install_dependencies`、`check_update_platform_support`、`install_update_package` 等统一 API
- [X] T008 [P] 新增 `pinvou3-app/src-tauri/src/os/linux/`，按业务域迁移或包装现有 Linux 行为，包括 `xdg-open`、`which`、`HOME` 家目录、`/usr/bin/nvidia-smi`、`sudoers`、`pkexec`、`.deb` 安装
- [X] T009 [P] 新增 `pinvou3-app/src-tauri/src/os/windows/`，按业务域实现 Windows 行为，包括 `cmd /C start`、`where`、`USERPROFILE`/`HOMEDRIVE`/`HOMEPATH` 家目录、`/tmp` 兼容映射、Windows 常见 `nvidia-smi.exe` 路径，以及 Linux 专属能力的清晰降级
- [X] T010 [P] 确认 pinvou3-app 暂不新增 `pinvou3-app/src-tauri/src/os/macos.rs`；非 Linux/Windows 平台统一走 `pinvou3-app/src-tauri/src/os/unsupported.rs` 的 fallback
- [X] T011 在 `pinvou3-app/src-tauri/src/lib.rs` 中注册 `os` 模块，使 Tauri Rust 代码可通过 `crate::os::*` 调用平台能力
- [X] T012 [P] 在 `pinvou3-app/src-tauri/src/os/mod.rs` 中补充平台无关的轻量测试或编译期断言，覆盖 API 返回类型、路径解析接口和基础降级文案
- [X] T013 更新 `specs/004-windows-compat-fixes/compatibility-scan.md`，说明首轮分散修复将迁移到 `pinvou3-app/src-tauri/src/os/` 抽象层

**Checkpoint**: OS 调度层骨架存在，业务调用点尚未迁移也应能保持可编译或具备明确的下一步迁移任务。

---

## Phase 3: User Story 1 - 识别 Windows 兼容性风险 (Priority: P1)

**Goal**: 从源码层面识别 Linux 硬编码、Unix 命令、路径假设和平台专属能力，并形成可维护的问题清单。

**Independent Test**: 运行静态扫描后，`compatibility-scan.md` 能按风险类别、影响文件、目标处理方式列出问题，且区分运行时代码、测试代码、文档示例和上游底座代码。

### Validation for User Story 1

- [X] T014 [P] [US1] 使用 `rg` 扫描 `pinvou3-app/src-tauri/src/` 中的 Linux 命令和路径特征，并将结果补充到 `specs/004-windows-compat-fixes/compatibility-scan.md`
- [X] T015 [P] [US1] 使用 `rg` 扫描 `pinvou3-app/` 中的 `/tmp`、`/usr`、`USERPROFILE`、`HOMEDRIVE`、`HOMEPATH`、`xdg-open`、`pkexec`、`apt`、`sudo`、`which` 等关键词，并将误报分类写入 `specs/004-windows-compat-fixes/compatibility-scan.md`
- [X] T016 [P] [US1] 使用 `rg` 扫描 `DeepSeek-TUI/` 中的平台专属代码，仅记录与 pinvou3 Windows 打包运行直接相关的问题到 `specs/004-windows-compat-fixes/compatibility-scan.md`

### Implementation for User Story 1

- [X] T017 [US1] 更新 `specs/004-windows-compat-fixes/compatibility-scan.md`，为每个确认问题补充风险等级、所属平台、处理策略和对应 OS 调度接口
- [X] T018 [US1] 更新 `specs/004-windows-compat-fixes/compatibility-scan.md`，明确哪些问题本轮修复、哪些仅记录为后续项，避免扩大当前改动范围

**Checkpoint**: Windows 兼容性风险清单完整，可单独作为接手和维护依据。

---

## Phase 4: User Story 2 - 修复高价值低风险兼容性问题 (Priority: P2)

**Goal**: 在尽量不改变现有业务逻辑的前提下，把已识别的高价值、低风险平台差异迁移到 OS 调度层，并保持 Linux 行为兼容。

**Independent Test**: `cargo check` 通过；业务文件不再直接维护可迁移的平台分支；Windows 上 Linux 专属能力返回清晰错误而非崩溃或误执行；路径兼容逻辑集中在 OS 调度层。

### Validation for User Story 2

- [X] T019 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/mod.rs` 或相邻测试模块中补充 `open_target`、`command_exists`、`nvidia_smi_candidates`、`user_home_dir`、`platform_compat_path` 的低风险测试或平台门控断言
- [X] T020 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/mod.rs` 或相邻测试模块中补充 Linux 专属能力在非 Linux 平台的降级文案断言，并覆盖 Windows `/tmp` 兼容映射的门控断言

### Implementation for User Story 2

- [X] T021 [US2] 重构 `pinvou3-app/src-tauri/src/commands.rs`，移除本文件内平台打开命令实现，统一调用 `crate::os::open_target`
- [X] T022 [US2] 重构 `pinvou3-app/src-tauri/src/monitor.rs`，移除本文件内 `nvidia-smi` 平台候选路径实现，统一调用 `crate::os::nvidia_smi_candidates`
- [X] T023 [US2] 重构 `pinvou3-app/src-tauri/src/file_ingest.rs`，将命令探测和依赖安装平台分支迁移到 `crate::os::command_exists` 与 `crate::os::install_dependencies`
- [X] T024 [US2] 重构 `pinvou3-app/src-tauri/src/super_permission.rs`，保留现有 Tauri 命令签名并委托 `crate::os::super_permission_*` 实现平台行为
- [X] T025 [US2] 重构 `pinvou3-app/src-tauri/src/updater.rs`，将平台支持检查和安装命令委托 `crate::os::check_update_platform_support` 与 `crate::os::install_update_package`
- [X] T026 [US2] 重构 `pinvou3-app/src-tauri/src/bridge/paths.rs`，移除本文件内平台路径 `cfg` 和 Windows `/tmp` 兼容转换实现，统一调用 `crate::os::user_home_dir` 与 `crate::os::platform_compat_path`
- [X] T027 [US2] 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`，将结果记录到 `specs/004-windows-compat-fixes/compatibility-scan.md`

**Checkpoint**: 运行时代码的主要平台差异集中在 `pinvou3-app/src-tauri/src/os/`，Linux 既有行为保持可追溯，Windows 降级路径明确，`bridge/paths.rs` 只保留业务目录布局组合逻辑。

---

## Phase 5: User Story 3 - 建立后续维护验证路径 (Priority: P3)

**Goal**: 为后续 Windows 适配提供稳定验证步骤，让维护者能快速判断新增代码是否再次引入 Linux 硬编码。

**Independent Test**: 维护者仅依赖 `quickstart.md` 和 `compatibility-scan.md`，即可复现静态扫描、编译检查和关键平台降级验证。

### Validation for User Story 3

- [X] T028 [P] [US3] 按 `specs/004-windows-compat-fixes/quickstart.md` 执行静态扫描命令，并将偏差更新到 `specs/004-windows-compat-fixes/quickstart.md`
- [X] T029 [P] [US3] 按 `specs/004-windows-compat-fixes/quickstart.md` 执行 Rust 编译检查命令，并将结果更新到 `specs/004-windows-compat-fixes/compatibility-scan.md`

### Implementation for User Story 3

- [X] T030 [US3] 更新 `specs/004-windows-compat-fixes/quickstart.md`，补充 OS 调度层新增文件、路径调度边界、调用边界和维护时必须执行的扫描命令
- [X] T031 [US3] 更新 `specs/004-windows-compat-fixes/compatibility-scan.md`，记录迁移完成后的最终状态、未修复项和后续建议
- [X] T032 [US3] 检查 `pinvou3-app/src-tauri/src/commands.rs`、`pinvou3-app/src-tauri/src/monitor.rs`、`pinvou3-app/src-tauri/src/file_ingest.rs`、`pinvou3-app/src-tauri/src/bridge/paths.rs`、`pinvou3-app/src-tauri/src/super_permission.rs`、`pinvou3-app/src-tauri/src/updater.rs` 的变更范围，确认仅保留委托调用、业务目录布局组合和必要错误处理

**Checkpoint**: 后续维护者具备清晰的扫描、编译、人工验证路径，新增平台逻辑应优先进入 `pinvou3-app/src-tauri/src/os/`。

---

## Phase 6: Polish & Cross-Cutting

**Purpose**: 做最终一致性检查，确保任务结果满足最小化修改、中文文档和可维护性要求。

- [X] T033 运行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`，确认 `pinvou3-app/src-tauri/` 编译通过
- [X] T034 使用 `rg` 复查 `pinvou3-app/src-tauri/src/` 中 Linux 专属命令，确认新增或保留项集中在 `pinvou3-app/src-tauri/src/os/linux/` 或有明确记录
- [X] T035 使用 `rg -n "cfg\\((windows|not\\(windows\\)|target_os)" pinvou3-app/src-tauri/src/bridge/paths.rs` 复查 `pinvou3-app/src-tauri/src/bridge/paths.rs`，确认路径平台分支已迁移到 OS 调度层
- [X] T036 使用 `rg` 复查 `specs/004-windows-compat-fixes/` 中是否存在未替换的占位符或英文模板残留，并修正文档
- [X] T037 检查 `git diff --stat`，确认改动集中在 `pinvou3-app/src-tauri/src/os/`、少量委托调用文件和 `specs/004-windows-compat-fixes/` 文档
- [X] T038 运行 `git status --short`，在 `specs/004-windows-compat-fixes/compatibility-scan.md` 中记录最终未提交文件范围

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: 无依赖
- **Foundational (Phase 2)**: 依赖 Setup；所有用户故事依赖 OS 调度层骨架
- **US1 (Phase 3)**: 依赖 Setup；可在 Foundational 后完善抽象映射
- **US2 (Phase 4)**: 依赖 Foundational 和 US1 的问题确认
- **US3 (Phase 5)**: 依赖 US1 和 US2 的最终状态
- **Polish (Phase 6)**: 依赖所有目标故事完成

### Story Dependencies

- **US1 (P1)**: 可独立交付，产出兼容性风险清单
- **US2 (P2)**: 依赖 OS 调度层骨架和 US1 的问题清单
- **US3 (P3)**: 依赖 US1/US2 结果，用于沉淀维护验证路径

### Parallel Opportunities

- T003、T004、T005、T006 可并行复核不同文件
- T008、T009、T010 可并行实现不同平台模块
- T014、T015、T016 可并行执行不同范围的静态扫描
- T019、T020 可并行补充不同 API 的测试或断言
- T028、T029 可并行验证文档中的扫描和编译步骤

---

## Parallel Example: User Story 2

```text
Task: "T019 [P] [US2] 在 pinvou3-app/src-tauri/src/os/mod.rs 或相邻测试模块中补充 open_target、command_exists、nvidia_smi_candidates、user_home_dir、platform_compat_path 的低风险测试或平台门控断言"
Task: "T020 [P] [US2] 在 pinvou3-app/src-tauri/src/os/mod.rs 或相邻测试模块中补充 Linux 专属能力在非 Linux 平台的降级文案断言，并覆盖 Windows /tmp 兼容映射的门控断言"
```

---

## Implementation Strategy

### MVP First (US1 Only)

1. 完成 Phase 1 和 Phase 2 的最小骨架确认
2. 完成 Phase 3，形成完整兼容性风险清单
3. 暂不迁移业务调用点，也能为 Windows 适配提供可靠决策依据

### Incremental Delivery

1. 完成 US1：问题识别和风险分类
2. 完成 US2：迁移高价值低风险调用点到 OS 调度层
3. 完成 US3：固化后续维护验证路径

### Minimal-Change Discipline

1. Linux 现有逻辑优先移动或包装，不重写业务语义
2. Windows 新实现集中在 `pinvou3-app/src-tauri/src/os/windows/`
3. 业务调用点只保留委托调用和必要错误处理
4. 未能低风险修复的问题记录到 `compatibility-scan.md`，不在本轮扩大范围
