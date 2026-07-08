# 研究：Windows 系统监控改为 CPU 卡片

## 决策 1：CPU 快照作为 `MonitorSnapshot` 的可选字段

**Decision**：在现有监控快照中增加可选 CPU 快照字段，Windows 返回真实数据，非 Windows 返回空值并继续使用原 GPU 快照。

**Rationale**：现有系统监控页已经通过 `get_monitor_snapshot` 做按需轮询，增加快照字段可以复用刷新、错误降级和前端格式化链路，不需要新增命令或独立轮询。

**Alternatives considered**：

- 新增单独 `get_cpu_snapshot` 命令：会增加前端轮询和生命周期管理复杂度。
- 直接复用 `gpu` 字段承载 CPU 数据：字段语义混乱，后续维护成本高。

## 决策 2：Windows CPU 采样放入 OS 层

**Decision**：新增或扩展 OS 层接口，由 `crate::os::cpu_snapshot()` 提供平台相关 CPU 快照；Windows 实现真实采样，Linux/unsupported 返回 `None`。

**Rationale**：项目已将路径、内存、系统工具等平台差异放在 `os` 层；继续沿用该边界可以避免在 `monitor.rs` 或前端中散落 `cfg(windows)` 判断。

**Alternatives considered**：

- 在 `monitor.rs` 中直接写 Windows 条件编译：短期更快，但会让通用监控逻辑承担平台细节。
- 在前端使用浏览器能力采样 CPU：桌面 WebView 不提供可靠 CPU 监控能力，无法满足需求。

## 决策 3：监控项限定为稳定可获取指标

**Decision**：首版只展示 CPU 名称、总体 CPU 使用率、应用进程 CPU 使用率、逻辑处理器数量和采样时间。

**Rationale**：这些指标能覆盖用户排查卡顿的核心诉求，且比 CPU 温度、功耗、频率等指标更稳定、更普遍可获取。

**Alternatives considered**：

- 展示 CPU 温度/功耗：Windows 设备差异大，可靠来源不统一，容易出现空值或误导。
- 展示每核心使用率：信息密度高但当前卡片空间有限，且需求只要求替换 GPU 一栏。

## 决策 4：前端只在 Windows 下替换 GPU 卡片

**Decision**：前端保留现有 GPU 卡片组件路径，但在 Windows 快照包含 CPU 数据时显示 CPU 卡片；非 Windows 继续显示 GPU 卡片。

**Rationale**：规格要求 Windows 替换，非 Windows 不变；用快照数据作为展示依据可以降低平台判断散落范围，并便于 smoke 验证。

**Alternatives considered**：

- 所有平台统一改为 CPU：违反非 Windows 保持 GPU 行为的要求。
- Windows 同时显示 CPU 和 GPU：超出本 feature 范围，也会改变页面布局。

## 决策 5：不新增持久化配置

**Decision**：CPU 监控不新增 settings、用户偏好或本地状态。

**Rationale**：这是系统监控页的固定 Windows 展示调整，不需要用户配置；新增配置会增加迁移和测试成本。

**Alternatives considered**：

- 提供 GPU/CPU 切换开关：需求没有要求，且会扩大 UI 和设置面。
