# 任务：Windows 系统监控改为 CPU 卡片

**输入**：`specs/023-windows-cpu-monitor/` 下的设计文档

**前置条件**：`plan.md`、`spec.md`、`research.md`、`data-model.md`、`contracts/monitor-snapshot.md`、`quickstart.md`

**测试**：本 feature 涉及 Windows 系统指标采样和系统监控 UI，任务包含 Rust 单元测试、前端 smoke 检查和非 Windows 回归检查。

**组织方式**：任务按用户故事分组，确保 US1 可先作为 MVP 独立实现和验证。

## Phase 1: 准备（共享基础）

**目的**：确认上下文、当前 worktree 和验证路径。

- [X] T001 阅读 `specs/023-windows-cpu-monitor/plan.md`、`specs/023-windows-cpu-monitor/spec.md` 和 `specs/023-windows-cpu-monitor/contracts/monitor-snapshot.md`，确认 CPU 快照契约和 Windows-only 范围
- [X] T002 检查 `pinvou3-app/src-tauri/src/monitor.rs`、`pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src/index.html`、`pinvou3-app/src-tauri/src/os/` 的现有改动，避免覆盖用户 worktree 修改
- [X] T003 [P] 对照 `specs/023-windows-cpu-monitor/quickstart.md` 准备验证命令和 Windows 手动验收清单

---

## Phase 2: 基础任务（阻塞后续故事）

**目的**：建立所有用户故事共享的 CPU 快照数据通道。

**CRITICAL**：本阶段完成前，不应开始任何用户故事实现。

- [X] T004 在 `pinvou3-app/src-tauri/src/monitor.rs` 中新增 `CpuSnapshot` 序列化结构，并在 `MonitorSnapshot` 中增加可选 `cpu` 字段
- [X] T005 在 `pinvou3-app/src-tauri/src/os/interface/mod.rs` 和 `pinvou3-app/src-tauri/src/os/interface/` 中增加 `cpu_snapshot()` OS 层接口导出
- [X] T006 在 `pinvou3-app/src-tauri/src/os/windows/mod.rs`、`pinvou3-app/src-tauri/src/os/linux/mod.rs` 和 `pinvou3-app/src-tauri/src/os/unsupported.rs` 中接入 `cpu_snapshot()` 平台导出或空实现
- [X] T007 在 `pinvou3-app/src-tauri/src/monitor.rs` 的 `sample_all()` 中接入 `crate::os::cpu_snapshot()`，确保 CPU 采样失败不影响 `ram`、`vllm`、`self_perf`、`app`

**检查点**：后端快照契约具备 `cpu` 可选字段，平台层具备统一入口。

---

## Phase 3: 用户故事 1 - 查看 CPU 负载状态 (Priority: P1) MVP

**目标**：Windows 系统监控页原 GPU 卡片位置显示 CPU 卡片，并展示 CPU 名称、总体 CPU 使用率、应用进程 CPU 使用率和逻辑处理器数。

**独立测试**：在 Windows 上打开系统监控页，确认不再显示 GPU 卡片，CPU 卡片在 2 秒内显示核心监控项。

### 测试 / 验证

- [X] T008 [P] [US1] 在 `pinvou3-app/src-tauri/src/os/windows/` 的 CPU 采样实现文件中添加百分比范围、逻辑处理器数和进程 CPU 计算的单元测试
- [X] T009 [P] [US1] 在 `pinvou3-app/src/tauri-bridge.js` 中为 CPU 快照格式化逻辑准备可手动构造快照验证的最小检查点

### 实现

- [X] T010 [US1] 在 `pinvou3-app/src-tauri/Cargo.toml` 中为 Windows target 补充 CPU 采样所需的 `windows-sys` feature
- [X] T011 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/windows_cpu.rs` 中实现 Windows CPU 名称、总体 CPU 使用率、应用进程 CPU 使用率和逻辑处理器数采样
- [X] T012 [US1] 在 `pinvou3-app/src-tauri/src/os/windows/mod.rs` 中导出 `windows_cpu::cpu_snapshot`
- [X] T013 [US1] 在 `pinvou3-app/src/tauri-bridge.js` 中格式化 `snap.cpu` 为 `cpuName`、`cpuTotal`、`cpuTotalPct`、`cpuProcess`、`cpuProcessPct`、`cpuLogicalProcessors`、`cpuAvailable`
- [X] T014 [US1] 在 `pinvou3-app/src/index.html` 中新增中英日 CPU 卡片文案，替换 Windows CPU 卡片需要的标题、不可用提示、总体使用率、应用占用和逻辑处理器标签
- [X] T015 [US1] 在 `pinvou3-app/src/index.html` 的 `MonitorView` 中将 Windows 有 CPU 快照时的资源卡片渲染为 CPU 卡片，并移除该路径下 GPU 显存、nvidia-smi、温度和功耗展示
- [X] T016 [US1] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib cpu` 或对应 Windows CPU 单测命令，记录结果到本轮交付说明

**检查点**：US1 可独立演示；Windows 上系统监控页显示 CPU 卡片和核心数据项。

---

## Phase 4: 用户故事 2 - 识别 CPU 数据不可用状态 (Priority: P2)

**目标**：CPU 采样失败或部分字段缺失时，页面保持 CPU 语义并显示占位值，其它监控卡片继续工作。

**独立测试**：模拟 CPU 快照为空或字段缺失，系统监控页仍显示 CPU 不可用状态，RAM、模型服务和应用指标继续刷新。

### 测试 / 验证

- [X] T017 [P] [US2] 在 `pinvou3-app/src/tauri-bridge.js` 中用手动构造的 `cpu: null`、缺失 `name`、缺失百分比字段快照验证格式化占位值

### 实现

- [X] T018 [US2] 在 `pinvou3-app/src-tauri/src/os/windows/windows_cpu.rs` 中确保 CPU 名称、总体使用率、进程使用率任一采样失败时返回部分可用快照或 `None`，不得 panic
- [X] T019 [US2] 在 `pinvou3-app/src/tauri-bridge.js` 中完善 CPU 字段缺失时的 `—`、0% 进度和不可用文案格式化
- [X] T020 [US2] 在 `pinvou3-app/src/index.html` 中确保 CPU 不可用路径仍渲染 CPU 卡片，不回退显示 GPU 或 `nvidia-smi` 文案
- [X] T021 [US2] 在 `pinvou3-app/src-tauri/src/monitor.rs` 中补充 CPU 快照为 `None` 时 `sample_all()` 仍返回其它监控字段的回归测试

**检查点**：US1 和 US2 均可独立验证；CPU 失败不影响其它监控区域。

---

## Phase 5: 用户故事 3 - 保持非 Windows 平台行为不变 (Priority: P3)

**目标**：Linux 等非 Windows 平台继续展示原有 GPU 卡片和 GPU 降级逻辑。

**独立测试**：在非 Windows 平台或通过空 CPU 快照路径验证系统监控页仍使用 GPU 展示。

### 测试 / 验证

- [X] T022 [P] [US3] 在 `pinvou3-app/src/tauri-bridge.js` 中用 `cpu` 缺失且 `gpu` 存在的快照检查非 Windows/GPU 展示格式仍可用

### 实现

- [X] T023 [US3] 在 `pinvou3-app/src-tauri/src/os/linux/mod.rs` 和 `pinvou3-app/src-tauri/src/os/unsupported.rs` 中确认 `cpu_snapshot()` 返回 `None` 且不影响现有 `nvidia_smi_candidates()` GPU 路径
- [X] T024 [US3] 在 `pinvou3-app/src/index.html` 中确保非 Windows 或无 CPU 快照时继续走现有 GPU 卡片渲染路径
- [X] T025 [US3] 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib monitor`，并按 `specs/023-windows-cpu-monitor/quickstart.md` 记录非 Windows GPU 回归验证结果或未执行原因

**检查点**：所有计划内用户故事均可独立验证。

---

## Phase N: 收尾与横切关注点

- [X] T026 [P] 检查 `specs/023-windows-cpu-monitor/tasks.md`、`specs/023-windows-cpu-monitor/quickstart.md` 和 `specs/023-windows-cpu-monitor/contracts/monitor-snapshot.md` 是否需要随实现细节同步修正
- [X] T027 运行 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib monitor` 和 Windows CPU 相关测试，并在最终交付说明中列出结果
- [ ] T028 在 Windows 上启动 `pinvou3-app` 并按 `specs/023-windows-cpu-monitor/quickstart.md` 完成系统监控页 smoke 验证
- [X] T029 检查 `pinvou3-app/src-tauri/src/monitor.rs`、`pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src/index.html` 中是否仍有 Windows CPU 卡片路径下的 GPU、显存、nvidia-smi 误导文案
- [X] T030 检查 `git diff --stat` 和 `git status --short`，确认没有修改 DeepSeek-TUI 底座或无关文件

---

## 依赖与执行顺序

- Phase 1 无依赖。
- Phase 2 阻塞所有用户故事。
- US1 是 MVP，完成后即可在 Windows 上演示 CPU 卡片。
- US2 依赖 US1 的 CPU 数据和前端 CPU 卡片路径，但可通过构造缺失字段快照独立验证。
- US3 依赖基础平台接口，主要保护非 Windows/GPU 路径不回归。
- 收尾任务依赖所有用户故事完成。

## 并行机会

- T003 可与 T001/T002 并行。
- T008 和 T009 可并行准备测试与前端检查点。
- T013 和 T014 可在 T011/T012 后并行推进，但 T015 依赖二者完成。
- T017 可与 T018 并行，分别验证前端占位和后端失败降级。
- T022 可与 T023 并行，分别验证前端 GPU 回退和平台 stub。
- T026 可与 T027/T028 的验证准备并行。

## 并行执行示例

### US1

```text
T008: 添加 Windows CPU 采样单元测试
T009: 准备前端 CPU 快照格式化检查点
```

### US2

```text
T017: 构造 CPU 缺失字段快照验证前端占位
T018: 完善 Windows CPU 采样失败降级
```

### US3

```text
T022: 构造 GPU 快照验证前端非 Windows 回退
T023: 确认 Linux/unsupported CPU stub 不影响 GPU 路径
```

## 实施策略

1. 先完成 Phase 1 和 Phase 2，建立统一 CPU 快照字段和 OS 层入口。
2. 优先完成 US1 作为 MVP：Windows 上能看到 CPU 卡片和核心指标。
3. 再完成 US2，确保 CPU 数据不可用时不会误导用户或影响其它卡片。
4. 最后完成 US3，确认非 Windows 平台仍使用原 GPU 监控体验。
5. 收尾阶段统一运行 quickstart 验证，并记录无法执行的平台验证原因。
