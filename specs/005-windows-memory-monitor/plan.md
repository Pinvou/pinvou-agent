# 实施计划：Windows 系统内存监控

**分支**：`006-windows-memory-monitor` | **日期**：2026-06-16 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/005-windows-memory-monitor/spec.md` 的功能规格

**说明**：本计划由 `/speckit-plan` 填充。计划必须遵守 `.specify/memory/constitution.md` 中的项目宪章。

## 概要

当前“系统监控”页的系统内存采样在 Rust 后端中直接读取 Linux 专有的 `/proc/meminfo`，导致 Windows 下 `ram` 快照始终为空。该 feature 将内存采样迁入既有 `os` 抽象层：监控业务只调用统一的 `crate::os::ram_snapshot()`，Linux 实现保留现有 `/proc/meminfo` 行为，Windows 实现补齐系统内存采样能力。前端页面结构、GPU 采样、vLLM 探测和 DeepSeek-TUI 底座均不改动。

## 技术上下文

**语言/版本**：Rust 2021（`pinvou3-app/src-tauri/Cargo.toml` 当前 `rust-version = "1.88"`）、JavaScript 前端桥接代码保持现状

**主要依赖**：Tauri 2、现有 `pinvou3-app/src-tauri/src/os` 抽象层、Windows 系统内存能力、Linux `/proc/meminfo`

**存储**：N/A；该功能只做按需采样，不新增持久化数据

**测试**：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml`、`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml`、Windows 手动打开“系统监控”页 smoke 验证

**目标平台**：Windows 桌面为主，Linux 桌面保持既有行为

**项目类型**：desktop-app

**性能目标**：进入监控页后 2 秒内展示 Windows 系统内存首个有效值；采样失败时不阻塞完整监控快照

**约束**：中文文档优先；不改 DeepSeek-TUI 底座；不重做前端页面；不顺带修 GPU/vLLM；新增平台差异必须进入 `os` 抽象层；保持小步变更

**规模/范围**：涉及 `pinvou3-app/src-tauri/src/monitor.rs` 与 `pinvou3-app/src-tauri/src/os/**`；如 Windows 实现需要系统 API crate，应作为 Tauri 后端的显式、最小依赖接入

## 宪章检查

*门禁：Phase 0 研究前必须通过；Phase 1 设计后必须复查。*

- **中文文档优先**：PASS。本计划、研究、数据模型、契约、quickstart 均使用中文；保留必要命令和 API 字段英文。
- **DeepSeek-TUI 底座优先**：PASS。本功能只触碰 Tauri app 监控采样，不改 Engine、ToolRegistry、Session、MCP、Hooks 等底座能力。
- **本地算力与数据边界**：PASS。系统内存采样为本机状态读取，不引入远程网络、外部模型或用户数据外发。
- **小步高质量变更**：PASS。范围限定为 RAM 采样从 `monitor.rs` 迁入 `os` 抽象层，前端展示和其他监控项不变。
- **可测试性与可验证交付**：PASS。计划包含 Rust 单测/检查和 Windows 手动 smoke 验证。
- **可维护性与长期演进**：PASS。通过 `os/interface` 增加明确平台能力边界，后续 macOS 或其他系统可在同一边界扩展。

**门禁结果**：PASS，无需复杂度豁免。

## 项目结构

### 文档（本 feature）

```text
specs/005-windows-memory-monitor/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── monitor-memory-contract.md
└── checklists/
    └── requirements.md
```

### 源码（仓库根目录）

```text
pinvou3-app/
├── src-tauri/
│   ├── Cargo.toml
│   └── src/
│       ├── monitor.rs
│       └── os/
│           ├── mod.rs
│           ├── unsupported.rs
│           ├── interface/
│           │   ├── mod.rs
│           │   └── memory.rs
│           ├── linux/
│           │   ├── mod.rs
│           │   └── linux_memory.rs
│           └── windows/
│               ├── mod.rs
│               └── windows_memory.rs
```

**结构决策**：延续当前 `os/interface/*` + `os/linux/linux_*` + `os/windows/windows_*` 的拆分方式。新增内存采样归类为 `memory` 业务域，避免把平台代码继续留在 `monitor.rs`，也避免在 app 层散落 `cfg`。

## 复杂度追踪

无需填写。当前方案符合既有 OS 抽象层，未引入宪章违反项。

## Phase 0：研究结论

见 [research.md](./research.md)。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- 监控契约：[contracts/monitor-memory-contract.md](./contracts/monitor-memory-contract.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 复查

- **中文文档优先**：PASS。新增产物均为中文。
- **DeepSeek-TUI 底座优先**：PASS。无底座改动。
- **本地算力与数据边界**：PASS。只读本机系统状态。
- **小步高质量变更**：PASS。新增文件按现有 OS 目录规则拆分。
- **可测试性与可验证交付**：PASS。契约和 quickstart 明确验证路径。
- **可维护性与长期演进**：PASS。新增平台能力边界可被后续系统复用。
