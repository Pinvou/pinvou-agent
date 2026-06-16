# 任务：Windows 系统内存监控

**输入**：`specs/005-windows-memory-monitor/` 下的设计文档

**前置条件**：plan.md、spec.md、research.md、data-model.md、contracts/monitor-memory-contract.md、quickstart.md

**测试**：本功能涉及 Windows 迁移和 OS 抽象层，任务包含 Rust 单测、编译检查和 Windows 手动 smoke 验证。

**组织方式**：任务按用户故事分组，保证每个故事可以独立实现和验证。

## 格式：`[ID] [P?] [Story] 描述`

- **[P]**：可并行执行，前提是修改不同文件且没有依赖关系。
- **[Story]**：任务对应的用户故事，例如 US1、US2、US3。
- 描述中必须包含精确文件路径。
- 文档、任务描述和验收说明默认使用中文；英文仅保留必要命令、路径、API 字段或原文。

## 路径约定

- **Tauri 桌面应用**：`pinvou3-app/src/`、`pinvou3-app/src-tauri/`
- **OS 抽象层**：`pinvou3-app/src-tauri/src/os/`
- **监控后端**：`pinvou3-app/src-tauri/src/monitor.rs`
- **文档**：`specs/005-windows-memory-monitor/`

## Phase 1: 准备（共享基础）

**目的**：确认上下文、目录、依赖和验证方式。

- [X] T001 阅读 `specs/005-windows-memory-monitor/plan.md` 并确认本 feature 只改系统内存监控，不修 GPU、vLLM 或前端页面。
- [X] T002 检查 `git status --short --branch` 输出并确认 `pinvou3-app/src-tauri/src/monitor.rs` 与 `pinvou3-app/src-tauri/src/os/` 下是否存在用户未提交改动。
- [X] T003 [P] 阅读 `specs/005-windows-memory-monitor/contracts/monitor-memory-contract.md` 并记录 `ram` 字段必须满足的契约。
- [X] T004 [P] 阅读 `pinvou3-app/src-tauri/src/os/mod.rs`、`pinvou3-app/src-tauri/src/os/interface/mod.rs`、`pinvou3-app/src-tauri/src/os/linux/mod.rs`、`pinvou3-app/src-tauri/src/os/windows/mod.rs`，确认现有 OS 抽象导出模式。

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立所有用户故事共同依赖的内存采样抽象。

**⚠️ CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T005 在 `pinvou3-app/src-tauri/src/os/interface/memory.rs` 中新增统一的 `ram_snapshot()` 接口，并让它委托到 `super::super::platform::ram_snapshot()`。
- [X] T006 在 `pinvou3-app/src-tauri/src/os/interface/mod.rs` 中声明 `memory` 模块并导出 `ram_snapshot`。
- [X] T007 在 `pinvou3-app/src-tauri/src/os/mod.rs` 中导出 `ram_snapshot`，供 `crate::os::ram_snapshot()` 调用。
- [X] T008 在 `pinvou3-app/src-tauri/src/os/unsupported.rs` 中新增不支持平台的 `ram_snapshot()`，返回 `None`。

**检查点**：`crate::os::ram_snapshot()` 的抽象入口存在，后续平台实现可以接入。

---

## Phase 3: 用户故事 1 - Windows 系统内存显示可用 (Priority: P1) 🎯 MVP

**目标**：Windows 下打开“系统监控”页时，系统内存区域显示有效已用内存、总内存和使用率。

**独立测试**：在 Windows 环境运行应用并打开“系统监控”页，确认系统内存不是 `—`，且物理内存百分比合理。

### 测试 / 验证

- [X] T009 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_memory.rs` 中添加 Windows 内存字节到 KiB 的换算和边界单测。
- [X] T010 [US1] 执行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml`，确认新增 Windows 内存辅助逻辑测试通过或记录当前失败点。

### 实现

- [X] T011 [US1] 在 `pinvou3-app/src-tauri/Cargo.toml` 中按需新增最小 Windows 系统 API 直接依赖，避免通过外部 PowerShell/WMI 命令采样。
- [X] T012 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_memory.rs` 中实现 Windows `ram_snapshot()`，返回 `total_kib`、`used_kib`、`swap_total_kib`、`swap_used_kib`。
- [X] T013 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/mod.rs` 中声明并导出 `windows_memory::ram_snapshot`。
- [X] T014 [US1] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中将 `sample_all` 的 `ram` 字段改为调用 `crate::os::ram_snapshot()`。
- [X] T015 [US1] 执行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`，确认 Windows 编译通过。
- [ ] T016 [US1] 按 `specs/005-windows-memory-monitor/quickstart.md` 启动应用并在“系统监控”页验证 Windows 系统内存显示有效值。

**检查点**：US1 可独立演示和验证，Windows 系统内存显示可用。

---

## Phase 4: 用户故事 2 - 保持 Linux 内存监控行为不变 (Priority: P2)

**目标**：Windows 适配后，Linux 下既有 `/proc/meminfo` 字段含义和展示行为不变。

**独立测试**：Linux 内存解析测试继续通过；如具备 Linux 环境，监控页内存展示仍可用。

### 测试 / 验证

- [X] T017 [P] [US2] 在 `pinvou3-app/src-tauri/src/os/linux/linux_memory.rs` 中为 `/proc/meminfo` 解析逻辑补充样例文本单测，覆盖 `MemTotal`、`MemAvailable`、`SwapTotal`、`SwapFree`。

### 实现

- [X] T018 [US2] 将 `pinvou3-app/src-tauri/src/monitor.rs` 中现有 Linux `/proc/meminfo` 解析逻辑迁移到 `pinvou3-app/src-tauri/src/os/linux/linux_memory.rs`，保持字段含义不变。
- [X] T019 [US2] 在 `pinvou3-app/src-tauri/src/os/linux/mod.rs` 中声明并导出 `linux_memory::ram_snapshot`。
- [X] T020 [US2] 删除或调整 `pinvou3-app/src-tauri/src/monitor.rs` 中依赖旧 `ram_snapshot()` 私有函数的测试，确保测试目标转移到 OS Linux 内存模块。
- [ ] T021 [US2] 执行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml`，确认 Linux 解析回归测试和现有测试通过。

**检查点**：US1 和 US2 均可独立验证，Windows 可用且 Linux 行为不回归。

---

## Phase 5: 用户故事 3 - 监控采样失败时可诊断 (Priority: P3)

**目标**：内存采样失败时只让 `ram` 降级为空，不影响完整监控快照返回。

**独立测试**：不支持平台或模拟失败路径返回 `None`，`monitor::sample_all` 仍返回包含 `app`、`gpu`、`vllm` 字段语义的完整快照。

### 测试 / 验证

- [X] T022 [P] [US3] 在 `pinvou3-app/src-tauri/src/os/unsupported.rs` 或相关平台模块中确认 `ram_snapshot()` 失败路径返回 `None` 的单测或编译期保护。

### 实现

- [X] T023 [US3] 检查 `pinvou3-app/src-tauri/src/monitor.rs` 中 `MonitorSnapshot` 聚合逻辑，确保 `ram: crate::os::ram_snapshot()` 为 `None` 时不影响 `gpu`、`vllm` 和 `app` 字段返回。
- [X] T024 [US3] 执行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml`，确认失败降级相关测试通过。

**检查点**：所有计划内用户故事均可独立验证。

---

## Phase N: 收尾与横切关注点

- [X] T025 [P] 更新 `specs/005-windows-memory-monitor/quickstart.md`，补充实际执行过的 Windows smoke 验证结果或未执行原因。
- [X] T026 执行 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 并记录结果。
- [X] T027 执行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml` 并记录结果。
- [X] T028 检查 `pinvou3-app/src/tauri-bridge.js` 与 `pinvou3-app/src/index.html` 未发生非必要修改，确保本 feature 保持前端页面不变。
- [X] T029 检查 `DeepSeek-TUI/` 未发生改动，确认没有触碰底座边界。
- [X] T030 检查 `specs/005-windows-memory-monitor/tasks.md` 中所有任务完成状态，并在最终说明中汇总 Windows 内存监控验证结论。

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事，因为 US1、US2、US3 都依赖统一的 `crate::os::ram_snapshot()` 抽象入口。
- US1 是 MVP，建议优先完成。
- US2 可在 Phase 2 后与 US1 并行迁移 Linux 实现，但需要避免同时编辑 `pinvou3-app/src-tauri/src/monitor.rs`。
- US3 依赖 Phase 2，并建议在 US1、US2 的实现边界稳定后完成。
- Phase N 在所有用户故事完成后执行。

## 并行机会

- T003 与 T004 可并行阅读不同上下文文件。
- T009 可与 T017 并行编写不同平台模块的单测。
- T011、T012、T013 串行完成 Windows 实现；T017、T018、T019 可由另一人处理 Linux 迁移，但需要协调 T014/T020 对 `monitor.rs` 的修改。
- T025 可与 T026、T027 并行准备文档结果，但必须等待实际验证命令完成后填写最终结果。

## 实施策略

1. 先完成 Phase 1 和 Phase 2，建立统一 OS 内存采样入口。
2. 优先完成 US1，让 Windows 系统内存先显示可用，作为 MVP。
3. 接着完成 US2，把旧 Linux 解析逻辑迁到 OS 抽象层并用单测保护。
4. 最后完成 US3，确认失败路径不会导致监控快照整体失败。
5. 收尾阶段运行 quickstart 中的验证命令，确认未改前端页面和 DeepSeek-TUI 底座。
