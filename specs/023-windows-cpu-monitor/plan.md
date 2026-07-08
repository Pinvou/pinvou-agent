# 实施计划：Windows 系统监控改为 CPU 卡片

**分支**：`023-windows-cpu-monitor` | **日期**：2026-07-08 | **规格**：[spec.md](./spec.md)

**输入**：来自 `specs/023-windows-cpu-monitor/spec.md` 的功能规格

## 概要

在 Windows 系统监控页中，将现有 GPU 资源卡片替换为 CPU 资源卡片，展示 CPU 名称、总体 CPU 使用率、pinvou3 应用进程 CPU 使用率、逻辑处理器数量和采样时间；非 Windows 平台保留现有 GPU 卡片和 `nvidia-smi` 采样路径。实现上沿用现有 `get_monitor_snapshot` 按需采样链路，新增 Windows OS 层 CPU 快照能力，并让前端基于快照内容展示 CPU 卡片。

## 技术上下文

**语言/版本**：Rust 1.88、JavaScript、Tauri 2

**主要依赖**：Tauri 命令、现有 `monitor.rs` 采样链路、Windows target 下的 `windows-sys`

**存储**：N/A；CPU 监控为实时快照，不落盘

**测试**：Rust 单元测试覆盖 CPU 快照计算；`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib monitor` 或相关 OS 层测试；Windows 手动 smoke 验证系统监控页展示

**目标平台**：Windows 桌面为本 feature 目标；Linux 等非 Windows 平台保持现有 GPU 监控行为

**项目类型**：desktop-app

**性能目标**：系统监控页打开后 2 秒内展示 CPU 可用数据；离开监控页后不新增后台轮询

**约束**：不改 DeepSeek-TUI 底座；不新增远程依赖；不展示不稳定的 CPU 温度/功耗；保持小步改动；中文文档；非 Windows 平台不回归

**规模/范围**：涉及 `pinvou3-app/src-tauri/src/monitor.rs`、`pinvou3-app/src-tauri/src/os/interface/`、`pinvou3-app/src-tauri/src/os/windows/`、`pinvou3-app/src-tauri/src/os/linux/` 或 unsupported stub、`pinvou3-app/src/index.html`、`pinvou3-app/src/tauri-bridge.js`、`pinvou3-app/src-tauri/Cargo.toml`

## 宪章检查

- **中文文档优先**：PASS；本计划、研究、数据模型、契约、quickstart 均使用中文，保留必要字段名和命令。
- **DeepSeek-TUI 底座优先**：PASS；本需求只涉及 Tauri UI/Rust wrapper 系统监控，不触碰 Engine、Session、MCP、Skill 等底座能力。
- **本地算力与数据边界**：PASS；CPU 监控只读取本机系统指标，不引入外部服务或网络数据。
- **小步高质量变更**：PASS；沿用现有 `get_monitor_snapshot` 和 OS 分层，新增 CPU 快照而不是重写监控页。
- **可测试性与可验证交付**：PASS；计划包含 CPU 计算单测、快照契约验证和 Windows 手动 smoke。
- **可维护性与长期演进**：PASS；新增 OS 层接口和契约文档，避免把 Windows 专有采样逻辑散落到前端或通用 monitor 逻辑。

**门禁结果**：PASS

## 项目结构

### 文档（本 feature）

```text
specs/023-windows-cpu-monitor/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── monitor-snapshot.md
└── checklists/
    └── requirements.md
```

### 源码（仓库根目录）

```text
pinvou3-app/src-tauri/src/
├── monitor.rs
└── os/
    ├── interface/
    │   ├── mod.rs
    │   └── system.rs 或新增 cpu.rs
    ├── windows/
    │   ├── mod.rs
    │   └── windows_system.rs 或新增 windows_cpu.rs
    ├── linux/
    │   └── mod.rs / linux stub
    └── unsupported.rs

pinvou3-app/src/
├── index.html
└── tauri-bridge.js

pinvou3-app/src-tauri/Cargo.toml
```

**结构决策**：沿用项目现有 OS 层模式，Windows 专有 CPU 采样放在 `os/windows`，通用 monitor 只消费 `crate::os::cpu_snapshot()`；前端继续通过 `get_monitor_snapshot` 接收完整快照，避免新增命令或重复轮询。

## 复杂度追踪

无宪章违背项。

## Phase 0：研究结论

见 [research.md](./research.md)。关键结论：CPU 快照作为 `MonitorSnapshot` 的可选字段加入；Windows 提供真实采样，非 Windows 返回 `None` 并继续使用 GPU 卡片；前端展示根据 CPU 快照和平台语义切换。

## Phase 1：设计产物

- 数据模型：[data-model.md](./data-model.md)
- 契约：[contracts/monitor-snapshot.md](./contracts/monitor-snapshot.md)
- 验证指南：[quickstart.md](./quickstart.md)

## Phase 1 后宪章复查

- **中文文档优先**：PASS
- **DeepSeek-TUI 底座优先**：PASS
- **本地算力与数据边界**：PASS
- **小步高质量变更**：PASS
- **可测试性与可验证交付**：PASS
- **可维护性与长期演进**：PASS

**复查结果**：PASS
