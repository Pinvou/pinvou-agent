# app-automations 内置工具（会话创建定时任务）设计与验收

> 状态：**已实施**（设计定稿 2026-09-29，随本分支落地；§9 为 CRUD 阶段增补，同样已实施）。配套契约：`docs/builtin-toolset-contract.md`（§9 注册表行已由 "not initiated" 更新为已注册）。
> 基线：`luzeyang-dev`（0a9fb107e）——本设计依赖 builtin 工具集契约、`session-reader` 包与 `features/messaging`（PR 尚未合入 main），实施分支须链接在该依赖之后。
> 实现模式：完全复用 `send_message_to_session`（26b575a17）的 "MCP 工具 + spool + 应用侧 watcher" 骨架，**新开能力族** `app-automations`，零 CodeWhale 变更。
> 用法：实施前逐节评审；实施后按 §5 验收矩阵逐项验收，P0 全过才算完成；矩阵编号被 §6 的测试映射反向引用。

---

## 1. 背景与目标

会话模型当前没有可用的定时任务创建通道：

- 基座 `automation` 工具（`CodeWhale/crates/tui/src/tools/automation.rs`）在应用会话中已注册但 manager 未挂载，调用必然失败（"AutomationManager is not attached"）；
- 现有"对话创建"流程靠提示词（`SCHEDULED_TASK_CHAT_PROMPT`，`features/scheduled/tasks.rs:235`）约束模型只输出 `scheduled-task-draft` 草稿块、由前端调 `create_scheduled_task` 代创建，仅在定时任务面板的专属引导会话中可用。

目标：在**任意会话**里，模型可通过内置工具直接创建定时任务，落库路径与面板完全一致（同一 `ScheduledTaskState::create_task` 领域函数、同一数据目录 `~/.pinvou3/automations`），并满足契约 §5 的 L1 安全要求。

## 2. 设计决策记录

| 决策点 | 结论 | 理由 |
|---|---|---|
| 归属 | **新开能力族**，新 MCP server `app-automations`（契约 §2/§8.1） | 调度是独立能力族，未来 run-now、记忆整理等自动化工具可归入；避免 session-reader 族膨胀 |
| 交互确认 | **沿用消息发送先例**：typed Ask 规则 + 审计日志 + 时间线可见性，不加阻塞式确认卡 | 与已上线 L1 工具一致；残余风险见 §9 |
| 返回语义 | **短等待同步**：server.py 落盘后轮询结果标记 ≤5s，命中返回任务 ID，超时回 pending | 模型能直接告知用户"已创建 XX"，前端可联动；纯异步会让模型无法确认结果 |
| rrule 范围 | 工具层只接受产品子集（HOURLY/WEEKLY/ONCE），**拒绝 CRON 与一切分钟级频率** | 与 `SCHEDULED_TASK_CHAT_PROMPT` 的产品约束一致；比领域层（`parse_rrule` 另支持 CRON）更严格是有意为之 |
| 基座死工具 | 顺带把 `automation`、`send_later` 加入应用 disallow 列表 | 应用内未挂 manager、永远失败，留在目录里只会诱导模型试错；`tasks`/`github` 不动（超范围） |

## 3. 方案总览

### 3.1 工具面

| 工具全名 | 等级 | 参数 |
|---|---|---|
| `mcp_app-automations_create_scheduled_task` | L1 | `name`(必填≤200)、`prompt`(必填≤32768)、`rrule`(必填)、`model_id`(可选)、`paused`(可选默认 false)、`idempotency_key`(可选≤128) |
| `mcp_app-automations_list_scheduled_tasks` | L0 | `limit`(1–100 默认 20)；返回 id/name/rrule/status/nextRunAt/model，**不含 prompt** |

### 3.2 数据流

```
模型调用工具 → server.py 校验（rrule 子集/caps/字符集）
  → 原子写 spool：~/.pinvou3/task-requests/spool/<sha256(from_session|kind|task_id|idempotency_key)|uuid>.json（§9.2 的 CRUD 命名；本节的历史形式已被其取代）
  → 轮询 spool/.done/<spool_id>.json ≤5s（0.2s 间隔）
      命中       → 返回 {ok:true, taskId, taskName, duplicate?}
      超时       → 返回 {ok:true, taskId:null, delivery:"pending"}

应用侧 watcher（features/scheduled 新模块，1s 轮询）
  → 重校验（spool 目录用户可写，不信任 server 侧校验）
  → .done 已存在 → 跳过（幂等）
  → ScheduledTaskState::create_task（复用既有管线：强制 yolo、工作间创建、模型绑定 sidecar、失败回滚）
  → 成功：写 .done {ok:true,task_id,task_name}、删 spool、audit::append(from_session 执行根)、
          emit "scheduled_task:run_updated"（双端前端已有监听，面板实时刷新）
  → 失败：≤3 次尝试（至多 2 次重试） → 隔离 spool/failed/ + 写 .done {ok:false,error}（等待中的调用拿到失败）
```

### 3.3 组件与职责

| 组件 | 位置 | 职责 |
|---|---|---|
| MCP server | `pinvou3-app/resources/mcp-servers/app-automations/{server.py,manifest.json}`（新增） | 模型工具面：校验、spool、结果等待、feature_disabled 回退 |
| 包注册 | `src-tauri/src/features/marketplace/mcp_catalog.rs`（改） | `MCP_PACKAGES` 加一项；默认安装列表加 id |
| 创建 watcher | `src-tauri/src/features/scheduled/creation_requests.rs`（新增） | spool 排水、重校验、调 `create_task`、结果标记、审计、事件 |
| 状态克隆化 | `src-tauri/src/features/scheduled/tasks.rs`（改） | `ScheduledTaskState` 派生 Clone（JoinHandle 包 `Arc<Mutex<Option<…>>>`，EnginePool.idle_reaper 同款） |
| 审批规则 | `src-tauri/src/features/assistant/platform/bridge.rs`（改） | `ToolAskRule::new(create 工具全名)`；为审批模式分化预留（当前产品全模式全自动，规则不弹窗；无人值守递归的确定性防线是引擎侧 `unattended_disallowed_tools`——三个写工具与 create_goal/update_goal 同列；watcher 的 sched-/eval_/aux- 发起方拒绝只是 claimed-string 过滤器，from_session 可省略且未认证） |
| 死工具清理 | `src-tauri/src/lib.rs`（改） | disallow 基座 `automation`/`send_later` |
| 前端 | `src/features/tools/tool-renderers.jsx`、`src/shared/i18n/{en,ja,zh}.js`（改） | 渲染卡（执行可见性）、三语文案 |
| 契约文档 | `docs/builtin-toolset-contract.md`（改） | §2 注册新族、§9 改 landed（round-8/9 更正：早前版本声称的"§6 回写短等待同步新模式"修订并未落地，短等待语义记录于本设计文档 §3.2，契约未单列） |

## 4. 实施步骤

**步骤 0 — 同步**：按 CONTRIBUTING 同步 origin/main 并链接依赖（builtin 工具集 / messaging 合入后 rebase），对齐子模块 gitlink。

**步骤 1 — MCP 包**（详见 §3.1/§3.2）：
- `server.py`：纯 stdlib NDJSON JSON-RPC over stdio，逐段镜像 session-reader 骨架（PROTOCOL_VERSION、错误脱敏不漏宿主路径、catch-all、`feature_disabled` 回退读 manifest `tool_features` + `builtin_features.json`）。
- rrule 校验：仅 `FREQ=HOURLY`(INTERVAL≥1 小时 + BYHOUR/BYMINUTE)、`FREQ=WEEKLY`(BYDAY 必填 + BYHOUR 0–23 + BYMINUTE 0–59)、`FREQ=ONCE`(AT 本地 `YYYY-MM-DDTHH:MM`、拒绝时区后缀、必须未来)；拒绝 CRON、分钟级、未知键、越界值；trim + 大写。
- spool 记录：`{schema_version:1, id, kind, task_id?, name?, prompt?, rrule?, model_id?, paused?, from_session?, created_at, idempotency_key?}`（round-8 更正：早期草案列出的 `from_title?` 两侧实现均未写入），mkstemp + os.replace 原子写。

**步骤 2 — Rust 注册**：`MCP_PACKAGES` 加项（include_str! 两文件）、`len()==12→13`；`DEFAULT_INSTALLED_MCP_TOOLS`（`marketplace/mod.rs:613`）加 id；`builtin.rs`/`types.rs` 镜像 session-reader 的清单与特性注册表测试。

**步骤 3 — 创建 watcher**：`ScheduledTaskState` 克隆化（步骤见 §3.3）；`ScheduledTaskState::start_creation_watcher（creation_requests.rs）` 用 `tauri::async_runtime::spawn`（勿用 tokio::spawn，a6d135840 教训），select! cancel/sleep(1s)，`boot_runtime` 末尾启动、句柄挂 state、Drop 取消；单文件处理逻辑见 §3.2。

**步骤 4 — Ask 规则**：`bridge.rs:215` 旁加 `pub const SCHEDULED_TASK_CREATE_TOOL`；`scope_deny_ruleset_with`(~:2358) push `ToolAskRule::new(...)`；list 工具不加（L0）。注：当前产品所有会话固定全自动审批，规则暂不弹窗（为审批分化 S-1 预留）；无人值守的递归自改由引擎侧 `unattended_disallowed_tools` 确定性拒绝（watcher 的 `check_sender_session_id` 隔离前缀拒绝只是 claimed-string 过滤器——from_session 可省略且未认证）。更新 bridge.rs 规则集测试（4209–4470 一带）。

**步骤 5 — 死工具清理**：lib.rs `tool_policy` 闭包追加 disallow `"automation"`、`"send_later"`。

**步骤 6 — 前端与 i18n**：三语 `uiBuiltinPlugins.dataAccess` 新 scope + `uiToolDetails.tools["app-automations"]` 副标题；`tool-renderers.jsx` 加渲染卡（name/rrule/prompt 摘要）。无新 Tauri 命令 → 无 access-policy/协议测试变更。

**步骤 7 — 契约文档**：§2 注册新族（附新开族论证）、§9 两行改 landed（round-9 更正：原计划的 §6 回写未执行，短等待语义以本设计文档 §3.2 为准）。

**步骤 8 — 测试**：按 §6 清单新增。

**步骤 9 — 检查与提交**：按 §7 门禁全绿；提交遵循 `docs/commit-message-convention.md` + DCO。

## 5. 验收矩阵

优先级：P0 = 硬 gate，全过才合并；P1 = 随功能验收必验；P2 = 尽力而为/后续加固。
"验证方式"列中测试名标注：◇ = 拟新增，○ = 既有。

### A. 核心创建链路

| # | 场景 | 前置/输入 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|---|
| A1 | 对话式创建端到端 | 普通会话说"每天 8:30 建个早报定时任务" | 模型调用 create 工具；≤5s 返回 taskId/taskName；面板出现新任务 | 手工 QA（真模型） | P0 |
| A2 | 短等待同步返回 | watcher 建任务后写 `.done` | server.py 命中标记，返回 `{ok,taskId,taskName}` | ◇python `test_short_wait_returns_task_ids` | P0 |
| A3 | 等待超时回 pending | `.done` 5s 内未出现（如 watcher 暂停） | 返回 `{ok:true, taskId:null, delivery:"pending"}`，非错误 | ◇python `test_wait_timeout_returns_pending_not_error` | P0 |
| A4 | 调度生效 | 创建成功 | 记录带 `next_run_at`；既有 15s 调度循环拾取；ONCE 到点运行一次后自动暂停，HOURLY/WEEKLY 按点触发 | 手工（短间隔任务）+ ○领域调度既有测试 | P0 |
| A5 | 面板实时刷新 | 面板已打开时 watcher 建任务 | `scheduled_task:run_updated` 事件驱动列表刷新，Tauri 与 web 双端一致 | 手工双端验证 | P1 |
| A6 | 模型绑定 | 传/不传 `model_id` | 传：`model-bindings.json` 落盘，运行会话用该模型；不传：回落应用 fallback 模型 | ◇Rust watcher 测试 + 手工 | P1 |
| A7 | paused 语义 | `paused:true` 创建 | 任务落库为 paused，不调度；面板可手动恢复 | ◇Rust 测试 + 手工 | P1 |

### B. 参数与校验（server.py 与 watcher 双层）

| # | 场景 | 输入 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|---|
| B1 | rrule 合法形态 | 文档三种样例（HOURLY INTERVAL；WEEKLY BYDAY；ONCE AT） | 通过并落库 | ◇python 参数化用例 | P0 |
| B2 | 分钟级拒绝 | "每 5 分钟"类表达（MINUTELY/CRON */5 等） | 拒绝，错误指引用户改每 N 小时/每天/每周 | ◇python | P0 |
| B3 | CRON 拒绝 | `FREQ=CRON;...` | 拒绝（工具层比领域层严格，产品约束） | ◇python | P0 |
| B4 | 过去/带时区 ONCE | `AT` 早于当前时刻；带 `Z`/`+08:00` 后缀 | 均拒绝，错误说明需未来本地时刻 | ◇python | P0 |
| B5 | rrule 越界/未知键 | `BYHOUR=24`、缺 `BYDAY`、未知键 | 拒绝 | ◇python | P1 |
| B6 | 长度上限 | name>200 / prompt>32768 / key>128；恰等于上限 | 超限拒绝；边界值通过 | ◇python 边界用例 | P1 |
| B7 | 必填缺失 | name/prompt/rrule 缺失或空 | `invalid` 错误，模型可据此修正 | ◇python | P1 |
| B8 | 规范化 | 小写/带空白 rrule | trim+大写后落库（与领域一致） | ◇python | P1 |
| B9 | watcher 重校验 | 手工篡改 spool（越界 prompt/非法 rrule） | 拒绝进 `failed/`，不创建任务（spool 目录用户可写，不信任 server 侧） | ◇Rust 测试 | P1 |

### C. 幂等与可靠性

| # | 场景 | 输入 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|---|
| C1 | 同幂等键重试 | 同 key 连续两次调用 | 同一 spool 文件、同一任务；第二次返回同 taskId 且 `duplicate:true`，不新建 | ◇smoke journey + ◇python | P0 |
| C2 | 无幂等键 | 不传 key | uuid 文件名，各自独立创建 | ◇python | P1 |
| C3 | watcher 重启恢复 | spool 有未处理文件时重启应用 | 重启后继续处理；`.done` 成功标记存在的不重复创建（ok:false 的失败标记允许重试重做）。投递语义为 at-least-once：应用与写标记之间的崩溃窗口内重试可重复 create（与 features/messaging 同窗口，已记录） | ◇Rust 测试 + 手工 | P0 |
| C4 | 失败重试与隔离 | 创建持续失败（如磁盘只读） | ≤3 次尝试（至多 2 次重试） → 移入 `spool/failed/` + `.done {ok:false,error}`；等待中的调用收到失败而非悬挂 | ◇Rust 测试 | P1 |
| C5 | 并发写容忍 | list 工具读取时恰逢 watcher 落盘 | 跳过坏行不崩溃 | ◇python | P1 |
| C6 | 目录惰性创建 | spool 目录不存在 | 首次调用自动创建 | ◇python | P2 |

### D. 权限与安全

| # | 场景 | 输入 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|---|
| D1 | Ask 规则注册 | 任意会话规则集 | 含 create 工具的 `ToolAskRule`(Ask，无 command 约束) | ◇Rust bridge 规则集断言（镜像 ○`scope_deny_ruleset_asks_for_session_messaging_send`） | P0 |
| D2 | 无人值守拒绝 | 定时任务运行会话内模型调用 create | `unattended_disallowed_tools` 引擎侧拒绝（本轮评审修订：force-prompt 链在当前全自动审批下不会触发）（递归自建被挡） | ○既有语义测试 + ◇新规则集断言 | P0 |
| D3 | 审计日志 | 成功创建 | from_session 执行根 append `scheduled_task_create` 记录（tool/task_id/outcome）；另有 R4-M3 的看门狗侧影子审计（<home>/automations/audit/，以 spool 文件名为键、不依赖 from_session，请求者无法通过省略字段跳过） | ◇Rust 测试（含 shadow_audit_covers_requests_without_from_session） | P1 |
| D4 | 错误脱敏 | 构造各类失败 | 错误文本不含宿主绝对路径 | ◇python（镜像 ○`test_error_messages_do_not_leak_absolute_paths`） | P0 |
| D5 | 不越界写 | 全流程 | 不写 `~/.pinvou3/sessions/*.json`、不触碰 schtasks/cron/systemd（工具与 watcher 均只落 `~/.pinvou3/task-requests` 与领域目录） | 代码评审 + grep 检查项 | P1 |
| D6 | 协议健壮性 | 非法 stdin/未知工具/深嵌套 JSON | 服务不死、`-32601/-32602`、catch-all 不漏异常文本 | ◇python（镜像 ○既有协议用例） | P1 |

### E. 可见性与 UI

| # | 场景 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|
| E1 | 执行可见性 | 工具调用出现在时间线；渲染卡展示 name/rrule/prompt 摘要 | 手工 + ◇node renderer 测试 | P1 |
| E2 | 内置插件卡片 | 插件中心内置区显示 app-automations（L1、数据范围、版本、不可卸载、无插件级开关） | 手工 + ○builtin 列表既有测试 | P1 |
| E3 | i18n 三语 | 新 scope 与副标题 en/ja/zh 齐全 | ○`ui_language_coverage.test.mjs` + 手工切换语言 | P1 |
| E4 | 配置可见性 | 工具不出现在会话配置的工具开关列表（builtin 惯例），仅时间线可见 | 手工核对 | P2 |

### F. 特性开关与生命周期

| # | 场景 | 操作 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|---|
| F1 | 关闭→目录移除 | `set_builtin_feature_enabled("scheduled-task-automation", false)` | 新会话 disallowed 含本族五工具全名（小写），目录不再出现 | ◇Rust（镜像 ○`feature_removal_flows_into_unavailable_tool_names`） | P0 |
| F2 | 在途调用降级 | 关闭后旧上下文仍调用 | 结构化 `feature_disabled` + 替代动作提示，非通用 not_found | ◇python（镜像 ○feature-gate 系列） | P1 |
| F3 | 联合语义 | 仅关本族特性 | 只移除本族五工具，session-reader 工具不受影响 | ◇Rust 注册表测试 | P1 |
| F4 | 状态持久一致 | 开关后重启 | prefs 与 `builtin_features.json` 镜像一致（○既有 replay 机制回归） | ○既有测试 | P1 |

### G. 平台与兼容

| # | 场景 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|
| G1 | 默认安装与升级 | 新装与既有 `~/.pinvou3` 启动后 app-automations 已安装、`bundle/mcp.json` 有 server 条目 | ◇扩展 ○`ensure_default_installed_mcp_tools_seeds_only_missing_records` + 手工 | P0 |
| G2 | 服务可启动 | `python3 server.py` stdio initialize/tools/list 往返正常，工具集与 manifest 一致 | ◇smoke 矩阵（13 包） | P0 |
| G3 | 双端一致 | web 端无新命令（无 access-policy 变更）；面板刷新事件双端已监听（○既有） | 代码核对 | P1 |
| G4 | 与对话创建共存 | 面板"对话创建"（scheduled-task-draft → 前端 create_scheduled_task）回归不受影响 | 手工 QA | P1 |
| G5 | 多智能体会话 | 工具面与普通会话一致，不额外注入 | 手工抽查 | P2 |

### H. 回归与门禁（P0 硬 gate）

| # | 检查 | 通过标准 |
|---|---|---|
| H1 | `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib --locked -- --test-threads=1` | 0 failed |
| H2 | `cargo clippy --manifest-path pinvou3-app/src-tauri/Cargo.toml -- -D warnings` + `cargo fmt --check` | 0 告警 |
| H3 | `npm --prefix pinvou3-app run test:node` | 0 failed |
| H4 | `python3 -m unittest discover -s scripts/tests` | 0 failed（含 session-reader 既有用例不回归） |
| H5 | `python3 scripts/mcp-server-contract-smoke.py` | 全绿（矩阵扩为 13 包） |
| H6 | `python3 scripts/architecture-guard.py` | 通过 |
| H7 | `./scripts/fork-guard.sh --fast` | 通过（零 fork 变更） |
| H8 | catalog/默认安装测试 | `len()==13`；默认安装种子含 app-automations |

## 6. 测试文件与矩阵映射

| 测试文件（拟新增◇/既有○） | 覆盖矩阵 |
|---|---|
| ◇`scripts/tests/test_app_automations_server.py` | A2 A3 B1–B8 C1 C2 C5 C6 D4 D6 F2 |
| ◇`scripts/mcp-server-contract-smoke.py`（journey） | A2 A3 C1 G2 H5 |
| ◇`features/scheduled/creation_requests.rs` 内联 Rust 测试 | A6 A7 B9 C3 C4 D3 |
| ◇`features/assistant/platform/bridge.rs` 规则集测试 | D1 D2 |
| ◇`features/marketplace/{mcp_catalog,builtin,types,mod}.rs` 测试 | E2 F1 F3 F4 G1 H8 |
| ◇`pinvou3-app/tests/`（renderer/coverage，如适用） | E1 E3 |
| 手工 QA | A1 A4 A5 E1 E2 E4 G4 G5 |

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 提示注入静默创建任务（交互会话自动批准） | 审计日志 + 时间线结果卡展示草稿摘要（超长截断） + 特性开关可全局关闭 + 无人值守运行由引擎侧 `unattended_disallowed_tools` 一律拒绝；后续可复用本 watcher 骨架与 forwarder 挂点加阻塞确认卡（已在决策中明确暂不做） |
| 任务爆炸（模型频繁创建） | list 工具让模型先查重；name 非唯一由用户在面板管理；`spool/failed` 隔离防队列阻塞 |
| 与基座死工具混淆 | `automation`/`send_later` 加入 disallow（步骤 5），模型目录里只剩一条可用路径 |
| watcher 与面板/领域并发写 | 创建唯一入口仍是 `ScheduledTaskState::create_task`（operation_locks 既有串行化）；watcher 不直接写 AutomationManager |
| 依赖未合入 main | builtin 工具集/messaging 目前仅在 `luzeyang-dev`；本分支链接其后，依赖 PR 合入 main 后统一 rebase（步骤 0） |

## 8. 边界与已披露事项

- 交互会话无阻塞确认（决策记录 §2）；`tasks`/`github` 基座工具可见性问题本次不处理。
- **无人值守递归防护的真实边界（round-8 M1 修订）**：确定性防线是引擎侧 `unattended_disallowed_tools`（deny 名单）+ round-8 起对 sched- 隐藏会话**关闭 subagents**（`agent(inherit_disallowed_tools=false)` 无法再剥离 deny 名单——子运行时本会保留 MCP+自动批准，而 posture 名单不含本族三写工具）。剩余通道：无人值守运行携带 shell 时可直接向 `~/.pinvou3/task-requests/` 写入合法 spool 记录（claimed-string 过滤器的既有让步）；上游把宿主 deny 视为不可剥离上限是 CodeWhale 侧的正确修法（已列入家族跟进）。
- 现有面板"对话创建"流程保留不动，两条通道汇合到同一个 `create_task` 领域函数。
- 零 CodeWhale / fork 变更，无需更新 `docs/fork-modifications.md`；本计划文档本身随本分支入库，作为后续实施的评审基线。
- 领域层 `parse_rrule` 实际支持 CRON，工具层收紧为三种子集是有意的产品约束；若未来要放开，只改 server.py + watcher 的子集校验，领域无需动。


---

## 9. 实施扩展记录：CRUD 全覆盖（2026-09-29，同分支追加定稿）

首版设计（§1–§8）只覆盖创建 + 列表。按评审意见扩展为完整增删改查，本节为其评审基线；§1–§8 中与本文冲突之处以本节为准。

### 9.1 工具面（替换 §3.1）

| 工具全名 | 等级 | 参数 |
|---|---|---|
| `mcp_app-automations_create_scheduled_task` | L1 | 同 §3.1 |
| `mcp_app-automations_read_scheduled_task` | L0 | `task_id`(必填)；返回单任务全量字段，**含 prompt**（更新前检视用） |
| `mcp_app-automations_list_scheduled_tasks` | L0 | 同 §3.1（不含 prompt） |
| `mcp_app-automations_update_scheduled_task` | L1 | `task_id`(必填) + `name`/`prompt`/`rrule`/`model_id`/`paused`（均可选，至少其一）；rrule 同产品子集 |
| `mcp_app-automations_delete_scheduled_task` | L1 | `task_id`(必填)；归档后删除，破坏性 |

### 9.2 设计决策补充

| 决策点 | 结论 | 理由 |
|---|---|---|
| 粒度 | 五个 `verb_noun` 工具，不合入 mode 枚举 | 契约 §4.2 偏好 one tool + mode，但读写审批语义不同：合入会让 L0 读也吃 L1 Ask 门（骚扰）或写操作失去门控；session-reader 三工具先例同理 |
| 删除授权 | delete 为 L1；typed Ask 规则已注册，但当前产品两模式均为全自动审批（session_policy `approval_params` = (true, Auto)），运行时不会弹逐次确认 | 诚实披露：写操作（含删除）立即生效，实际审查入口是审计日志、时间线结果卡与定时任务面板；Ask 规则为审批模式分化（S-1）落地后的强制点预留。删除走面板同一条 archive-then-delete 管线（历史可追溯） |
| spool 扩展 | 记录加 `kind`(`create`/`update`/`delete`) + `task_id`，其余字段全部可选 | 契约 §4.4 只允许增字段：旧 create 记录经 watcher 默认 `kind=create` 继续有效 |
| 幂等命名 | `sha256("<from_session>\|<kind>\|<task_id>\|<key>")`；key 必须随 from_session | 命名空间按发送方收敛，不随 from_session 缺省退化为全局；同键不同操作不互踩（创建与更新可共用一个键而不互相覆盖） |
| 目标预检 | update/delete 在 server 侧先探测目标存在 | 快速失败省一轮 spool；watcher 与领域层仍重检（不信任 server 侧） |
| Ask 规则 | update/delete 同 create 一样注册 typed Ask 规则；read/list 不加 | 读操作不加门（不打扰）；规则在当前全自动审批下不弹窗，为审批分化预留；无人值守的递归自改由引擎侧 `unattended_disallowed_tools` 确定性拒绝（watcher 的 sched-/eval_/aux- 发起方拒绝只是 claimed-string 过滤器） |

### 9.3 验收矩阵追加（接续 §5 编号）

| # | 场景 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|
| I1 | 更新端到端 | update 改名/rrule/paused 落库并实时刷新 | ◇Rust `update_request_changes_the_existing_task` + 手工 | P0 |
| I2 | 删除端到端 | delete 走 archive-then-delete，任务从面板消失、历史可追溯 | ◇Rust `delete_request_archives_and_removes_the_task` + 手工 | P0 |
| I3 | 读详情 | read 返回含 prompt 的全量字段；未知 id 显式 not found | ◇python + 手工 | P0 |
| I4 | update 校验 | 缺 task_id / 零字段 / CRON rrule 均拒绝 | ◇python + ◇Rust `update_request_validation_requires_target_and_a_field` | P0 |
| I5 | delete 校验 | 缺 task_id、带多余字段均拒绝 | ◇python + ◇Rust `delete_request_validation_rejects_extra_fields` | P1 |
| I6 | Ask 规则扩展 | 规则集含 update/delete 两条 Ask；read/list 不含 | ◇Rust `scope_deny_ruleset_asks_for_scheduled_task_create` 扩展 | P0 |

§6 测试映射相应扩展：python 套件覆盖 I3/I4/I5，smoke 旅程覆盖 read/update/delete 校验路径，渲染卡测试覆盖三操作结果解析。


---

## 10. Implementation addendum: scheduled messages (the `session_message` kind, 2026-09-29; re-cut and translated 2026-10-06)

On top of §9's CRUD, a third task kind: `session_message` (scheduled messages) — at fire time no new conversation is created; instead the task's prompt is delivered into the designated session (steer when busy, a new turn when idle, reusing features/messaging's delivery semantics).

### 10.1 Design decisions

| Decision | Outcome | Rationale |
|---|---|---|
| Shape | A task kind, not a new tool family | Scheduling/CRUD/panel/pause-resume/run history are all reused; a new scheduling loop would be duplication |
| Kind derivation | On create, the presence of the `target_session` field means session_message; on update, `target_session` is only a retarget of an existing session_message task — an ordinary task cannot be converted | The tool surface and the panel express it the same way, with no half-specified state; an explicit kind parameter is mutually exclusive with it |
| Target allowlist | Ordinary sessions only; sched-/aux-/eval_ rejected at every layer (server validation, watcher re-check, executor re-check before each delivery) | Waking an unattended session on a schedule is the recursion direction; the isolation-prefix semantics follow contract §5 |
| Target storage | Reuses the task-kinds sidecar (entries gain an additive target_session field) | No new store file; old records stay compatible via serde defaults |
| Message body | The task's prompt (≤32k, the same cap as the messaging channel) | The cap is validated at create and update, with a belt re-check before each executor delivery; the panel/details/run records display it naturally, with no new field |
| Self-addressing | Allowed (target = the creating session is the main use case) | The product currently runs full-auto approval; the practical review surface is the timeline result card and the audit log; the full message text is visible in the timeline |
| Run records | Thread-less (like memory_organize), with the result text naming the target session | The target session is the thing to open; known limitation: a run record is not yet click-to-jump to the target |
| Dead targets | A deleted target session → that delivery's run is marked failed (the delivery channel's own error) | No silent retry loop; the next fire fails the same way and leaves a trace |
| Steer-loss window | A steer accepted but then dropped by the foundation → the run records Completed while the message never landed | The same pre-existing window as features/messaging (documented in messaging/mod.rs); this kind inherits and discloses it honestly; it is a rare event |
| Delivery bound | Steer + dispatch share one 30s budget (the messaging channel bounds each stage at 30s) | A slow cold engine start can time one delivery out into failure; the next fire retries (as covered under dead targets above) |

### 10.2 Acceptance addendum

| # | Scenario | Verification | Priority |
|---|---|---|---|
| J1 | Scheduled message delivers at fire time (busy → steer / idle → new turn) | ● Rust executor tests + manual | P0 |
| J2 | Target allowlist enforced across the three layers | ● python + ● Rust (create rejection / watcher isolation+charset / executor gate test incl. the run-level pin) | P0 |
| J3 | Kind is one-time: an ordinary task cannot be converted via update | ● Rust | P0 |
| J4 | Retarget is limited to session_message tasks and the target must exist | ● Rust | P1 |
| J5 | The watcher loop lands the kind+target sidecar | ● Rust | P0 |
| J6 | Delivery run records are thread-less and the result names the target | ● Rust + manual | P1 |

● = landed test (the round-3 review verified these by name; the earlier ◇ "planned" markers misread as unimplemented and are retired).

### 10.3 Further disclosed boundaries (round 4)

- **Misfire semantics**: a missed recurring fire within the 60s grace catches up; beyond it the fire is silently skipped (the next occurrence runs). A missed ONCE task delivers arbitrarily late with no staleness bound — surprising for a *message*, accepted for v1.
- **No origin envelope**: unlike features/messaging's delivered block, a scheduled delivery injects the raw task prompt as the target's opening instruction, with no machine-readable origin block; the audit trail and the run record are the provenance. An origin block is future work.
- **ACP/code targets**: rejected at creation and per fire via the sidecar `session-agents.json` — NARROWER than messaging's twin gate (which consults the live AcpPool's metadata recovery; a double-faulted index plus failed boot recovery escapes this check) — deliver through the independent code page instead.
- **Panel asymmetry**: the panel can neither create a session-message task nor retarget one (retargeting an ordinary task is impossible in the domain); target-setting is tool-only.
- **Record-vs-reality windows (round 6)**: once `pool.steer` returns Ok the message cannot be recalled — a cancel landing between the enqueue and the next poll records **Canceled** while the message still lands; symmetrically the 30s timeout can record **Failed** while a just-submitted steer/dispatch lands. The run record is bookkeeping, not a delivery receipt.
- **Model binding is display-only for this kind (round 6 minor 3)**: creation accepts `target_session` together with `model`/`model_id` and the DTO displays the binding, but delivery never consults it — steer uses the target's live engine, the dispatch fallback resolves the model exactly as the target session's own next turn would (`prepare_runtime_model` → session override else app default). The binding is retained for the DTO/panel display and for a future per-message model override.
- **Poison taxonomy breadth (round 6 minor 8)**: `is_permanent_domain_error` sees only watcher-apply errors, so several of its markers ("Unsupported scheduled task kind", "pass target_session to create", "cannot be combined", "require memory to be enabled", "isolated and cannot", "cannot be blank") are a dead safety net from that call site — `build_create_input`/`build_update_input` cannot produce them (blanks die at watcher validate; kind is hardcoded). Meanwhile the live attempt-1-poison classes are the TARGET-PROBE and cap errors only: a missing target session ("target_session not found: …") and this PR's own cap messages ("… exceeds the … character limit"). An update/delete of an UNKNOWN TASK does NOT poison on attempt 1 — the parent CRUD wraps its miss as "Failed to update/delete scheduled task '<id>': …" which matches no permanent marker, so it burns the full 3-attempt budget before quarantine (round-7 M-A reword: the round-6 sentence claimed attempt-1 quarantine for this class, which the code contradicts; both converge to quarantine, the budget differs).
- **Delivery at-least-once / crash-replay window (round 7 minor 8)**: the delivery path has NO result marker — a crash between the engine accepting a turn and the run record persisting replays the message on the next process start (the foundation scheduler advances `next_run_at` only after the run persists). This is the delivery-path analogue of the CRUD spool's accepted marker window, but the window is foundation-owned.
- **The 32k prompt cap is tool-stricter than the panel (round 7 minor 9)**: the server caps EVERY kind's prompt at 32k code points; the domain/panel cap session_message prompts only — the same deliberate-stricter posture as the rrule subset (CRON/minute-granular rejected at the tool layer), recorded so it does not read as drift.
- **Feature switch scope (round 6 minor 4)**: disabling `scheduled-task-automation` removes the MCP tool surface but does NOT stop existing `session_message` tasks from firing — the scheduler/executor are app core and the panel remains the management surface (pause/delete there), same as every other task kind.
