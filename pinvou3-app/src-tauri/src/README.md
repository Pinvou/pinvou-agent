# Rust 目录边界

Rust 后端按“功能优先、平台适配次之”组织：

- `app/`：Tauri 命令入口、启动装配、事件桥和应用级基础设施；`commands.rs` 仅维护稳定门面，命令实现按业务域放在 `app/commands/`。
- `features/`：业务功能；功能专属的平台代码放在该功能的 `platform/` 子目录。
- `platform/`：跨功能复用的操作系统原语与兼容门面，例如进程、凭证、通知和路径。
- `core/`：预留给不依赖 Tauri、具体功能或操作系统的共享类型。
- `lib.rs`：唯一的 crate 组合根，只声明模块和装配应用。

迁移期通过 `lib.rs` 的 `#[path]` 保持原有 `crate::<module>` 路径，避免目录调整同时改变公共 API。`app/commands.rs` 使用 `include!` 将领域文件保持在同一个 Rust 模块中，从而维持 Tauri 命令名、参数协议和 `crate::commands::*` 路径。新增命令应放入 `app/commands/<domain>.rs`，不再向门面文件或 `platform/os/*_system.rs` 追加完整业务实现。

平台代码规则：

1. 业务流程留在 feature 根目录。
2. Windows、Linux、macOS 差异放入 `features/<name>/platform/`。
3. 只有被多个 feature 共同使用的低层能力才能进入 `platform/`。
4. 操作系统选择使用 `cfg(target_os)`；Cargo feature 不用于模拟操作系统。
5. 未支持能力必须显式返回 unsupported，不得静默执行其他平台实现。

上述依赖和平台边界由仓库根目录的 `scripts/architecture-guard.py` 检查。迁移期保留
`#[path]`、`include!` 可以用于兼容公共路径或生成代码，语法本身不作为架构违规；是否
合理取决于实际职责、依赖方向和生成代码边界。可稳定检测的结构性债务记录在
`scripts/architecture-baseline.json`，只能逐步减少，不能新增或扩大；规则、显式例外
和本地运行方式见 `docs/architecture-guard.md`。

当前 Rust 依赖方向为 `lib.rs/app → features → platform/core`。feature 不能引用
`app/commands`，feature 之间也不能形成依赖环。跨功能协作由组合根注入：Engine 工具和
工具门控通过 `EngineToolFactory` / `ToolPolicy` 注入，Tauri 事件通过 `AppEventBus`
转发，远控附件 staging 通过 `AttachmentStager` 注入。资源 bundle、连接器可见性协调和
路径安全策略属于跨功能基础设施，统一位于 `platform/`。

资源治理沿用同一组合根纪律。当前 `lib.rs/app` 已装配 schema v6
`HostWorkRegistry`、每个受信 Adapter 的独立异步 worker 与 Linux Supervisor client；
`assistant`、`scheduled`、`knowledge` 或 `connector` feature 仍不得为控制彼此而
新增依赖。跨边界只传 opaque `work_id + generation + directive_id` 和封闭动作
枚举，PID、systemd unit、cgroup 路径与命令只保留在受信 Adapter 或 Supervisor
内部。模型、Renderer、远程 Web / MCP 和普通 Tauri 命令面不得注册任意
HostWork 或传入 OS 标识；Supervisor 的固定 app launcher 是当前唯一随包的
`Launch` 用法，只能选固定 app descriptor，不能传入任意 target 或 command。它与
同 UID socket 也不构成对恶意同 UID shell 的强隔离。

组合根当前注册 6 个静态生产 Adapter：scheduled、knowledge、编译期固定
connector、仅作用于 root turn 已终态且 session 空闲的 detached sub-agent、经
Supervisor 停止的 ASR，以及 `essential + non-governable` 的 app cgroup status-only
Adapter。前五者都只声明 Stop；最后的 app cgroup 不声明任何控制动作。前台
turn、任意 managed child 与 app/WebKit 自停不在当前 Governor 动作面。旧 Mission
同步 callback 生产面已移除，Mission Adapter 为 0。详细动作矩阵、账本与未经
MegaBook 实机 E2E 的部署边界见
[`ADR-0009`](../../../docs/adr/0009-PinvouOS-资源治理与Host-Supervisor.md)。
