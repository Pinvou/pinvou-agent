# DeepSeek-TUI 源码职责分析

## 阅读导向

本文回答三个问题：

1. DeepSeek-TUI 在 pinvou3 中承担哪些底座职责。
2. pinvou3-app 如何从 Tauri/Rust 适配层接入 DeepSeek-TUI 的 Engine、Session、Tool、Skill、MCP、Hooks、Compaction 和 SubAgent 能力。
3. Windows 维护、合并、构建、打包和排查时，应优先看哪些源码区域和检查项。

本文不做这些事：

- 不替代 DeepSeek-TUI 官方架构文档，也不逐行翻译上游源码。
- 不修改 `DeepSeek-TUI/` 或 `pinvou3-app/` 业务代码。
- 不重新设计 agent runtime、工具注册、会话存储、工作流调度或打包方案。
- 不覆盖所有历史方案；分析以当前仓库检出的 `DeepSeek-TUI` 子模块和 `pinvou3-app/src-tauri/src/` 接入方式为准。

## 一句话定位

DeepSeek-TUI 是 pinvou3 的 agent 底座，负责模型调用、Engine 循环、工具注册与执行、流式事件、Session、Skill、Commands、MCP、Hooks、SubAgent、Compaction 等运行时能力。

pinvou3-app 是 Tauri UI、Rust wrapper、配置翻译和状态适配层：它把 `~/.pinvou3` 下的用户设置、会话目录、工作流目录、bundle prompt、搜索配置和 Windows 桌面事件翻译为 DeepSeek-TUI 可消费的 `EngineConfig`、`Op` 和事件回传。

## 源码全景

### DeepSeek-TUI 工作区

当前 `DeepSeek-TUI/Cargo.toml` 是 Rust workspace，版本为 `0.8.60`，edition 为 `2024`，`rust-version` 要求 `1.88`。workspace 成员如下：

| crate/目录 | 主要职责 | pinvou3 接入状态 |
|---|---|---|
| `crates/tui` | 主 runtime，包含 `EngineConfig`、`EngineHandle`、`spawn_engine`、`Op`、`Event`、tool registry、skill registry、session manager、compaction、TUI app、runtime API | 当前直接接入，是 pinvou3-app 最核心依赖 |
| `crates/config` | API provider、模型、搜索、配置类型 | 当前直接接入，`bridge/mod.rs` 构造 `DtConfig` 和搜索配置 |
| `crates/hooks` | Hook 事件、sink、dispatcher | 当前直接接入配置，运行期由底座调度 |
| `crates/mcp` | MCP server/client 管理、工具/资源描述、启动状态 | 间接依赖，通过底座工具体系和 MCP 配置接入 |
| `crates/tools` | 工作区中独立的工具能力 crate | 间接依赖，主入口仍经 `crates/tui/src/tools` 和 registry |
| `crates/core` | 共享核心类型或底层能力 | 间接依赖，pinvou3 主要通过 `deepseek_tui::core::*` re-export 使用 |
| `crates/protocol` | 协议结构 | 间接依赖，由 runtime、MCP、API 层消费 |
| `crates/state` | 状态相关能力 | 间接依赖 |
| `crates/secrets` | 密钥/敏感配置处理 | 间接依赖 |
| `crates/execpolicy` | 执行策略 | 间接依赖 |
| `crates/agent` | agent 相关独立能力 | 当前未直接接入，属于底座背景能力 |
| `crates/cli` | DeepSeek-TUI CLI 入口 | 当前未直接接入；pinvou3 走 Tauri app |
| `crates/app-server` | app server/runtime API 形态 | 当前未直接接入主线 |
| `crates/release` | 发布辅助 | 当前未直接接入主线 |
| `crates/tui-core` | TUI 共享 UI 核心 | 当前未直接接入主线 |
| `crates/whaleflow` | workflow 相关底座能力 | 当前未直接接入主线；pinvou3 有自己的 `harness`/workflow 编排 |

### 当前直接接入的 DeepSeek-TUI 能力

- `DeepSeek-TUI/crates/tui/src/core/engine.rs`：定义 `EngineConfig`、`EngineHandle`、`spawn_engine`，pinvou3-app 通过它创建每个 session 独立 Engine。
- `DeepSeek-TUI/crates/tui/src/core/ops.rs`：定义 `Op::SendMessage`、`Op::SyncSession`、`Op::SpawnSubAgent`、`Op::CompactContext`、`Op::Shutdown` 等操作。
- `DeepSeek-TUI/crates/tui/src/core/events.rs`：定义 `Event::MessageDelta`、`ToolCallStarted`、`ToolCallComplete`、`TurnComplete`、`AgentComplete`、`CompactionStarted` 等事件。
- `DeepSeek-TUI/crates/tui/src/session_manager.rs`：定义 `SessionManager`、`SavedSession`、`SessionMetadata`，pinvou3 包装后定向到 `~/.pinvou3/sessions/`。
- `DeepSeek-TUI/crates/tui/src/skills/mod.rs`：定义 `SkillRegistry`，pinvou3 用它发现 bundle skills。
- `DeepSeek-TUI/crates/tui/src/tools/registry.rs`：定义 `ToolRegistry` 和 `ToolRegistryBuilder`，底座负责统一注册、过滤、执行工具。
- `DeepSeek-TUI/crates/tui/src/compaction.rs`：定义 `CompactionConfig` 和压缩逻辑，pinvou3 只调整本地模型和阈值。
- `DeepSeek-TUI/crates/hooks/src/lib.rs`：定义 `HookEvent`、`HookDispatcher` 等 hook 机制，pinvou3 在 bridge 中生成 hook 配置。
- `DeepSeek-TUI/crates/mcp/src/lib.rs`：定义 `McpManager` 与 MCP 工具/资源结构，底座经工具体系统一接入。
- `DeepSeek-TUI/crates/tui/src/tools/pinvou3_blocklist.rs`：fork 中的 pinvou3 工具隐藏清单，`lib.rs` 有 guard 测试防止合并时误删。

## pinvou3 接入边界

### bridge：唯一配置翻译层

`pinvou3-app/src-tauri/src/bridge/mod.rs` 是 pinvou3-app 和 DeepSeek-TUI 的配置边界。它负责：

- 读取 pinvou3 设置、bundle、路径、搜索 provider、本地 vLLM/OpenAI-compatible 配置。
- 构造 `deepseek_tui::core::engine::EngineConfig`。
- 构造 DeepSeek-TUI config。
- 把 GUI mode、plan phase、persona reminder 翻译为 `Op::SendMessage`。
- 构造 `HooksConfig`。
- 为普通 session、workflow run 分别生成不同 `workspace`、`instructions`、`tool_whitelist`。

一个重要维护点是 `build_engine_config()` 显式 destructure `EngineConfig::default()`：上游新增字段时这里会编译失败，迫使维护者判断该字段在 pinvou3 中应覆盖还是透传。这是防止 fork drift 的有意设计。

### engine：EngineHandle wrapper 和事件转译层

`pinvou3-app/src-tauri/src/engine.rs` 不重新实现 Engine。它只做三件事：

- 调 `spawn_engine(engine_config, &dt_config)` 创建 DeepSeek-TUI Engine。
- 通过 `EngineHandle::send(...)` 注入 `Op::SendMessage`、`Op::SyncSession`、`Op::SpawnSubAgent`、`Op::CompactContext` 等操作。
- 后台读取 `EngineHandle::rx_event`，把 DeepSeek-TUI `Event` 转为 Tauri 前端事件，例如 `chat:delta`、`chat:tool_start`、`chat:tool_end`、`chat:done`、`workflow:agent_progress`、`chat:compaction`。

### engine_pool：多 session 生命周期管理

`pinvou3-app/src-tauri/src/engine_pool.rs` 维护 `session_id -> AppEngine`。当前模型是每个 session 一个独立 Engine：

- 首次给某个 session 发消息时 lazy spawn。
- session 有磁盘历史时，spawn 后用 `Op::SyncSession` 注入 messages。
- 删除 session 时发送 `Op::Shutdown` 并 abort 对应 event forwarder。
- cancel、compact、submit_user_input 都按 session 路由。

### commands：Tauri 命令入口

`pinvou3-app/src-tauri/src/commands.rs` 是前端调用入口。它不直接实现模型循环，而是：

- chat/send 相关命令路由到 `EnginePool::send_user_message`。
- skill 列表和启用逻辑复用 `deepseek_tui::skills::SkillRegistry`。
- workflow 启动、kick、retry 通过 pinvou3 harness 生成任务，再向底座发送 `Op::SpawnSubAgent`。
- session、persona、workflow、user input 等状态通过 `SessionStore`、`EnginePool` 和 Tauri event 连接前后端。

### sessions：上游 SessionManager 的 pinvou3 包装

`pinvou3-app/src-tauri/src/bridge/sessions.rs` 包装 `deepseek_tui::session_manager::SessionManager`：

- session 数据目录定向到 `~/.pinvou3/sessions/`，隔离 `~/.deepseek/`。
- `save_session` 的 atomic write 由上游处理。
- pinvou3 额外维护 active session、mode state、active skill binding、artifact 列表和 auto-continue 计数。
- 切换或冷启动旧会话时，由 `EnginePool` 把磁盘 messages 重新注入到该 session 专属 Engine。

## 关键调用链

| 场景 | 入口 | pinvou3 适配层 | DeepSeek-TUI 能力 | 输出或副作用 |
|---|---|---|---|---|
| 应用启动 | `pinvou3-app/src-tauri/src/lib.rs` 初始化 Tauri state 和 commands | `SessionStore::boot()`、`EnginePool::new()`、`Pinvou3Bridge::boot()` | `SessionManager::new(...)`、`EngineConfig` 构造前置 | 建立 session 存储、全局 bridge、空 Engine 池 |
| 首次发送消息 | 前端 invoke chat command | `commands.rs` 取 session/mode，`EnginePool::send_user_message()` lazy spawn | `spawn_engine(...)`、`Op::SendMessage`、`EngineHandle::send` | 模型 turn 开始，流式事件进入 `rx_event` |
| 流式渲染首页/对话内容 | `EngineHandle::rx_event` | `engine.rs::spawn_event_forwarder` | `Event::MessageDelta`、`Event::ToolCallStarted`、`Event::ToolCallComplete`、`Event::TurnComplete` | Tauri emit `chat:delta`、`chat:tool_start`、`chat:tool_end`、`chat:done` |
| 切换或恢复 session | 前端加载历史 session | `SessionStore::load()`、`EnginePool::get_or_spawn()`、`AppEngine::sync_session()` | `SessionManager::load_session`、`Op::SyncSession` | 将磁盘 messages 和 workspace 注入该 session Engine |
| 手动上下文压缩 | 前端 token/压缩入口 | `EnginePool::compact_now()`、`AppEngine::compact_now()` | `Op::CompactContext`、`CompactionConfig`、`Event::Compaction*` | 底座执行压缩，前端收到 `chat:compaction` |
| skill 列表和启用 | 工作流/技能视图 | `commands.rs::list_skills_v2()`、`start_skill_session()` | `SkillRegistry::discover(...)` | 前端拿到 bundle skill 摘要；session 绑定 active skill |
| workflow 派发子任务 | `kick_workflow` / `retry_workflow_role` | pinvou3 `harness` 生成 `HarnessAction::SpawnAgent`，commands 发送 op | `Op::SpawnSubAgent`、SubAgent runtime、`Event::AgentComplete` | 子 agent 执行，完成后 `engine.rs` 推进 harness 和前端状态 |
| request_user_input | 工具触发用户输入 | `engine.rs` emit `chat:user_input_required`，commands 提交选择 | `Event::UserInputRequired`、`EngineHandle::submit_user_input` | 前端选择回灌到底座工具执行 |
| 工具审批 | 底座发审批事件 | `engine.rs` 对 pinvou3 yolo 助手自动 approve | `Event::ApprovalRequired`、`EngineHandle::approve_tool_call` | 工具调用继续执行 |
| 工具隐藏清单防回归 | Rust 测试 | `pinvou3-app/src-tauri/src/lib.rs` 的 `blocklist_contract` | `deepseek_tui::tools::pinvou3_blocklist` | 防止 fork 合并后 LLM tool schema 膨胀 |

## 不要重复造轮子的能力

| 底座能力 | DeepSeek-TUI 已做的事 | pinvou3 推荐扩展方式 |
|---|---|---|
| Engine | `EngineConfig`、`EngineHandle`、`spawn_engine`、turn loop、取消、用户输入、事件通道 | 只在 `bridge` 构造配置，只在 `engine`/`engine_pool` 包装生命周期 |
| ToolRegistry | 统一注册 native tools、MCP tools、agent tools，并负责执行和 API schema 输出 | 新工具优先走底座 registry 或 MCP server；不要在 Tauri 层自造 LLM 工具路由 |
| 流式事件 | `Event` 枚举承载 delta、tool、approval、user input、turn complete、compaction、subagent 事件 | 只在 `engine.rs` 转译为前端事件；不要另开一套 SSE/stream parser |
| Session | `SessionManager` 负责 session 文件、metadata、atomic write、历史加载 | pinvou3 只改目录和附加 UI 状态；不要重写 session 落盘格式 |
| SkillRegistry | 扫描 `SKILL.md`，解析 skill 元数据，提供发现和列表 | 领域能力写 `SKILL.md` 或 bundle skill；不要把领域 agent 写死进 Rust runtime |
| Commands | DeepSeek-TUI 有 slash command 和 TUI command 体系 | pinvou3 的 Tauri command 只做桌面 UI 入口；底座命令能力不要复制一份 |
| MCP | MCP manager 负责外部 server、工具、资源、启动状态和调用 | 外部 API 接独立 MCP server；pinvou3 只提供配置入口或用户可见状态 |
| Hooks | `HookEvent`、`HooksConfig`、`HookDispatcher` 提供统一事件 hook | pinvou3 在 bridge 配置 hook；不要在业务层散落独立 hook 协议 |
| Cycle | 当前 0.8.60 中 `EngineConfig.cycle` 已移除；历史上属于底座行为循环/容量控制边界 | 不在 pinvou3-app 重新补回 cycle；如上游重新引入，仍应在 bridge 显式评估字段 |
| Compaction | `CompactionConfig`、自动/手动压缩、压缩事件、context overflow 兜底 | pinvou3 只设置本地模型和阈值；不要自行摘要历史或改写底座消息链 |
| SubAgent | `Op::SpawnSubAgent`、agent progress、mailbox、completion 事件 | pinvou3 harness 只决定何时派发和如何推进 workflow；SubAgent 执行交给底座 |
| 模型/API 配置 | provider、base_url、model、search provider、vision config 等结构 | bridge 做配置翻译；不要在前端散落 provider 兼容逻辑 |

## 源码证据索引

| 证据点 | 路径或类型 | 说明 |
|---|---|---|
| 1 | `DeepSeek-TUI/Cargo.toml` | workspace、版本 `0.8.60`、Rust `1.88`、crate 成员 |
| 2 | `DeepSeek-TUI/crates/tui/src/core/engine.rs::EngineConfig` | Engine 配置主结构 |
| 3 | `DeepSeek-TUI/crates/tui/src/core/engine.rs::EngineHandle` | pinvou3 持有并发送 `Op` 的 runtime handle |
| 4 | `DeepSeek-TUI/crates/tui/src/core/engine.rs::spawn_engine` | pinvou3 创建 Engine 的底座工厂 |
| 5 | `DeepSeek-TUI/crates/tui/src/core/ops.rs::Op` | SendMessage、SyncSession、SpawnSubAgent、CompactContext、Shutdown |
| 6 | `DeepSeek-TUI/crates/tui/src/core/events.rs::Event` | delta、tool、turn、compaction、agent 事件源 |
| 7 | `DeepSeek-TUI/crates/tui/src/tools/registry.rs::ToolRegistry` | 工具注册和执行中心 |
| 8 | `DeepSeek-TUI/crates/tui/src/tools/registry.rs::ToolRegistryBuilder` | native/MCP/agent 工具组装入口 |
| 9 | `DeepSeek-TUI/crates/tui/src/skills/mod.rs::SkillRegistry` | `SKILL.md` 发现与解析 |
| 10 | `DeepSeek-TUI/crates/tui/src/session_manager.rs::SessionManager` | session 持久化和加载 |
| 11 | `DeepSeek-TUI/crates/tui/src/compaction.rs::CompactionConfig` | 上下文压缩配置 |
| 12 | `DeepSeek-TUI/crates/mcp/src/lib.rs::McpManager` | MCP server/tool/resource 管理 |
| 13 | `DeepSeek-TUI/crates/hooks/src/lib.rs::HookDispatcher` | hook 事件广播 |
| 14 | `pinvou3-app/src-tauri/src/bridge/mod.rs::build_engine_config` | pinvou3 偏好到 `EngineConfig` 的翻译 |
| 15 | `pinvou3-app/src-tauri/src/engine.rs::AppEngine::spawn_for_session` | 每 session 创建独立 Engine |
| 16 | `pinvou3-app/src-tauri/src/engine.rs::spawn_event_forwarder` | DeepSeek-TUI 事件到 Tauri 事件的转译 |
| 17 | `pinvou3-app/src-tauri/src/engine_pool.rs::EnginePool` | session 到 Engine 的生命周期管理 |
| 18 | `pinvou3-app/src-tauri/src/bridge/sessions.rs::SessionStore` | 上游 `SessionManager` 的 pinvou3 包装 |
| 19 | `pinvou3-app/src-tauri/src/commands.rs::list_skills_v2` | 复用 `SkillRegistry` 展示 bundle skills |
| 20 | `pinvou3-app/src-tauri/src/commands.rs::kick_workflow` | harness action 转为 `Op::SpawnSubAgent` |

## Windows 与维护注意事项

| 检查项 | 识别方式 | 建议处理 |
|---|---|---|
| 子模块版本不匹配 | `git submodule status --recursive`；前缀 `-` 表示未初始化，`+` 表示检出提交与父仓记录不一致 | 先执行 `git submodule update --init --recursive DeepSeek-TUI`，再构建 |
| DeepSeek-TUI API 字段变化 | `bridge/mod.rs::build_engine_config` 编译失败，提示 `EngineConfig` 缺字段或多字段 | 不要用 `..Default::default()` 绕过；逐字段判断覆盖或透传 |
| `Cargo.lock` drift | 合并后 `pinvou3-app/src-tauri/Cargo.lock` 或 DeepSeek-TUI 依赖变化 | 用同一 Rust/Cargo 环境重新构建，确认 lock 变化是预期 |
| Rust 工具链 | `DeepSeek-TUI/Cargo.toml` 要求 `rust-version = "1.88"`；旧 rustc 会报版本或特性错误 | Windows 上可用最新 stable，但必须满足 `1.88+` |
| release exe 进程占用 | `cargo build --release` 报无法删除 `pinvou3-tauri.exe` 或 “拒绝访问” | 先关闭运行中的 release exe，必要时用任务管理器结束进程 |
| 用户目录路径 | `bridge/paths.rs` 和 `bridge/sessions.rs` 指向 `~/.pinvou3/...` | Windows 路径修复要注意盘符、UNC、中文用户名、rooted path |
| session/artifact/settings 边界 | session 在 `~/.pinvou3/sessions/`，artifact 随 session 元数据，settings 在 pinvou3 用户目录 | 不要把 `~/.deepseek` 和 `~/.pinvou3` 混用 |
| 打包产物路径 | Tauri release 产物在 `pinvou3-app/src-tauri/target/release/`，bundle/msi 在 `pinvou3-app/src-tauri/target/release/bundle/` | 问 exe 位置和 msi 位置时要区分 |
| 本地 vLLM 模型配置 | `bridge/mod.rs` 构造 provider、base_url、model、vision config、reasoning_effort | 白屏/无响应不要先猜 UI，先看配置、日志和底座事件是否返回 |
| 搜索 provider/API key | `bridge/mod.rs` 将 pinvou3 prefs 翻译为 DeepSeek-TUI `SearchProvider` 和 `search_api_key` | 搜索不可用时先核对 provider 与 key 是否符合底座要求 |
| 工具隐藏清单 | `lib.rs` 中 `blocklist_contract` 测试，`DeepSeek-TUI/crates/tui/src/tools/pinvou3_blocklist.rs` | 合并 fork 后若 tool schema 膨胀，先看 blocklist 是否丢失 |
| 合并后冒烟 | `cargo build --release --manifest-path pinvou3-app/src-tauri/Cargo.toml`，再启动 release exe 观察 20 秒 | 确认进程未立即退出、窗口响应、日志没有 fatal |

### 合并后最小冒烟步骤

```powershell
git submodule status --recursive
cargo build --release --manifest-path pinvou3-app/src-tauri/Cargo.toml
.\pinvou3-app\src-tauri\target\release\pinvou3-tauri.exe
```

观察点：

- release exe 启动后不立即退出。
- 窗口有响应，不持续白屏。
- 后端日志能看到 bridge/engine 启动、`spawn_engine`、事件 forwarder 等关键阶段。
- 发送一条简单消息后能看到 `chat:delta` 和 `chat:done`。

## 按问题类型排查

| 问题类型 | 优先查看 | 判断思路 |
|---|---|---|
| 白屏/闪退 | `pinvou3-app/src/index.html`、Tauri 启动日志、`pinvou3-app/src-tauri/src/lib.rs`、`engine.rs` 启动日志 | 区分 webview 未加载、Rust 初始化失败、Engine spawn 失败、前端等待事件 |
| 首屏长时间白屏 | `lib.rs` 初始化、`Pinvou3Bridge::boot()`、`EnginePool::new()`、前端首屏日志 | 用时间戳日志判断卡在前端资源、bridge boot、路径访问还是 engine lazy spawn |
| 编译失败 | `pinvou3-app/src-tauri/Cargo.toml`、`Cargo.lock`、`DeepSeek-TUI/Cargo.toml`、`bridge/mod.rs::build_engine_config` | 先看子模块提交和 Rust 版本，再看上游字段变更 |
| 会话异常 | `bridge/sessions.rs`、`engine_pool.rs::get_or_spawn`、`engine.rs::sync_session`、`SessionManager` | 检查 session 目录、messages 是否加载、是否发了 `Op::SyncSession` |
| 工具不可用 | `DeepSeek-TUI/crates/tui/src/tools/registry.rs`、`tool_setup.rs`、`pinvou3_blocklist.rs`、`bridge/mod.rs` features | 判断是工具未注册、被隐藏、feature 未开、还是 MCP/server 配置问题 |
| 技能不可用 | `commands.rs::list_skills_v2`、`SkillRegistry::discover`、`bridge/paths.rs::bundle_workflow_dir` | 检查 bundle skill 目录、`SKILL.md` 是否存在、是否被 skiplist 隐藏 |
| workflow 异常 | `commands.rs::start_workflow`、`kick_workflow`、`engine.rs::AgentComplete` 分支、`harness.rs` | 判断卡在项目初始化、SubAgent 派发、AgentComplete 回传、gate 推进还是前端状态 |
| 模型配置异常 | `bridge/mod.rs::build_dt_config`、`build_engine_config`、用户 settings、vLLM endpoint | 检查 provider、base_url、model、api_key、reasoning_effort、vision config |
| 压缩异常 | `bridge/mod.rs` 中 `CompactionConfig`、`engine.rs` 的 `Event::Compaction*` 分支、`compaction.rs` | 判断是阈值过高、模型名不匹配、压缩事件未转译还是 context overflow |
| MCP 不可用 | `mcp_config_path`、`DeepSeek-TUI/crates/mcp/src/lib.rs`、`ToolRegistryBuilder` MCP adapter | 先看 MCP 配置路径，再看 server 启动状态和工具是否进入 registry |
| Hook 行为异常 | `bridge/mod.rs::build_hooks_config`、`DeepSeek-TUI/crates/hooks/src/lib.rs` | 检查 hook event 类型、sink、是否由 bridge 正确配置 |

## 验收清单

- [x] 中文主体：本文除技术名词、路径、类型和命令外均使用中文说明。
- [x] 定位清楚：已说明 DeepSeek-TUI 是 agent 底座，pinvou3-app 是 Tauri UI、Rust wrapper、配置/状态适配层。
- [x] 源码模块：已覆盖 `DeepSeek-TUI/Cargo.toml` workspace 和 16 个 crate。
- [x] 底座能力：已覆盖 Engine、ToolRegistry、流式事件、Session、SkillRegistry、Commands、MCP、Hooks、Cycle、Compaction、SubAgent、模型/API 配置。
- [x] 接入边界：已覆盖 `bridge/mod.rs`、`engine.rs`、`engine_pool.rs`、`commands.rs`、`bridge/sessions.rs`。
- [x] 调用链：已列出 10 条关键调用链，超过 6 条要求。
- [x] 源码证据：已列出 20 个源码证据点，超过 12 个要求。
- [x] Windows 维护风险：已列出 12 条检查项，超过 8 条要求。
- [x] 问题排查：已覆盖白屏/闪退、编译失败、会话异常、工具不可用、技能不可用、工作流异常、模型配置异常。
- [x] 交付边界：本文只新增文档，不要求修改业务代码。
