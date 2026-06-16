# 研究：Windows 系统内存监控

## 决策 1：内存采样进入 OS 抽象层

**Decision**：新增统一的 `ram_snapshot` 平台能力，由 `os/interface` 暴露，`os/linux` 和 `os/windows` 分别实现；`monitor.rs` 只负责组装完整监控快照。

**Rationale**：当前 `monitor.rs` 直接读取 `/proc/meminfo`，这是 Linux-only 机制。项目已经建立 `path`、`permission`、`dependency`、`system`、`update` 等 OS 抽象分层，内存采样应沿用同一边界，避免在业务层继续增加平台判断。

**Alternatives considered**：
- 在 `monitor.rs` 中加 `cfg(target_os = "windows")`：拒绝。会把平台差异重新散落在业务模块中，违背用户要求和既有 OS 抽象方向。
- 前端检测 Windows 后隐藏内存项：拒绝。不能解决用户需要看到 Windows 内存状态的核心问题。

## 决策 2：Linux 保留现有 `/proc/meminfo` 行为

**Decision**：Linux 侧将现有解析逻辑搬迁到 `os/linux/linux_memory.rs`，字段含义保持不变：`total_kib`、`used_kib`、`swap_total_kib`、`swap_used_kib`。

**Rationale**：现有 Linux 行为已经满足监控页需要，迁移目标是隔离平台实现，而不是重写 Linux 数据源。保持字段含义可以降低回归风险。

**Alternatives considered**：
- Linux 也切换到跨平台第三方库：暂不采用。会扩大变更面，并引入不必要的行为差异。

## 决策 3：Windows 使用系统级内存状态能力

**Decision**：Windows 侧使用系统级内存状态能力读取物理内存总量和可用量，并换算为 KiB；交换分区/页面文件信息若不可得则允许返回 0。

**Rationale**：监控页当前最关键的是物理内存已用量、总量和使用率；Windows 没有 `/proc/meminfo`。系统级能力可直接提供物理内存总量和可用量，满足 P1 验收。

**Alternatives considered**：
- 调用 PowerShell/WMI 命令：拒绝。外部命令开销更高、输出格式不稳定，也更容易受权限和本地化影响。
- 只返回空值并提示不支持：拒绝。与本 feature 目标冲突。

## 决策 4：采样失败保持降级为空

**Decision**：统一接口返回 `Option<RamSnapshot>`；采样失败时返回 `None`，完整 `MonitorSnapshot` 继续返回。

**Rationale**：`MonitorSnapshot` 当前所有监控项已经按 `Option` 设计，GPU/vLLM 也遵循 graceful degrade。保持该行为能避免单项失败导致页面崩溃或轮询中断。

**Alternatives considered**：
- 采样失败时返回错误给 Tauri command：拒绝。会让整个监控快照失败，前端只能看到整体不可用。

## 决策 5：不修改前端页面

**Decision**：保持 `tauri-bridge.js` 与 `index.html` 的展示结构不变，只保证后端 `ram` 字段在 Windows 下有值。

**Rationale**：前端已经能展示 `ram.used_kib`、`ram.total_kib` 和交换空间字段；问题根因在后端 Windows 采样为空。后端补齐数据即可满足用户价值，且变更最小。

**Alternatives considered**：
- 新增 Windows 专用 UI 文案或布局：暂不采用。当前需求不要求页面变化，且会扩大验证范围。
