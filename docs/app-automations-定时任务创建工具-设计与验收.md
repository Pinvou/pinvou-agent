# app-automations 内置工具（会话创建定时任务）设计与验收

> 英文镜像：[`app-automations-定时任务创建工具-设计与验收.en.md`](./app-automations-定时任务创建工具-设计与验收.en.md)（按 CONTRIBUTING 对新开发者文档的英文要求补充；两文内容一一对应，分歧以中文原文为准）。
> 状态：**已实施**（设计定稿 2026-09-29，随本分支落地；§9 为 CRUD 阶段增补，同样已实施）。配套契约：`docs/builtin-toolset-contract.md`（§9 注册表行已由 "not initiated" 更新为已注册）。
> 基线：`main`——本设计依赖 builtin 工具集契约、`session-reader` 包与 `features/messaging`（撰写时三者经 luzeyang-dev 线评审、尚未合入 main；现已全部落地：messaging 经 #627，session-reader 内建经 #585/#586）。
> 实现模式：完全复用 `send_message_to_session`（`features/messaging`，经 #627 落地）的 "MCP 工具 + spool + 应用侧 watcher" 骨架，**新开能力族** `app-automations`，零 CodeWhale 内容变更（gitlink 随基座 main 的 #721 对齐）。
> 用法：实施前逐节评审；实施后按 §5 验收矩阵逐项验收，P0 全过才算完成；矩阵编号被 §6 的测试映射反向引用。

---

## 1. 背景与目标

会话模型当前没有可用的定时任务创建通道：

- 基座 `automation` 工具（`CodeWhale/crates/tui/src/tools/automation.rs`）在应用会话中已注册但 manager 未挂载，调用必然失败（"AutomationManager is not attached"）；
- 现有"对话创建"流程靠提示词（`SCHEDULED_TASK_CHAT_PROMPT`，`features/scheduled/tasks.rs::SCHEDULED_TASK_CHAT_PROMPT`）约束模型只输出 `scheduled-task-draft` 草稿块、由前端调 `create_scheduled_task` 代创建，仅在定时任务面板的专属引导会话中可用。

目标：在**桌面 Operate/Yolo 会话**里，模型可通过内置工具直接创建定时任务，落库路径与面板完全一致（同一 `ScheduledTaskState::create_task` 领域函数、同一数据目录 `~/.pinvou3/automations`），并满足契约 §5 的 L1 安全要求。

## 2. 设计决策记录

| 决策点 | 结论 | 理由 |
|---|---|---|
| 归属 | **新开能力族**，新 MCP server `app-automations`（契约 §2/§8） | 调度是独立能力族，未来 run-now、记忆整理等自动化工具可归入；避免 session-reader 族膨胀 |
| 交互确认 | 创建及名称/模型/暂停状态更新立即生效；prompt/rrule 覆盖返回 `confirmation_required`，由用户在现有面板检查并提交。~~删除同样返回 `confirmation_required`~~（round-36 修订：该半句是已被取代的 round-32 提案——无歧义目标的删除按 §5 例外直接执行，不返回 `confirmation_required`；见契约 §5/§9 与工具描述） | prompt/rrule 的面板评审由用户于 2026-10-21 明确选择；全自动模式下 typed Ask 规则不是授权机制 |
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
  → 成功：写 .done {ok:true,task_id,task_name,request_digest}、删 spool、audit::append(绑定会话 → 私有台账根，未绑定 → 执行根；round-24 M4 后同 messaging r8-M1)、
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
| 审批规则 | `src-tauri/src/features/assistant/platform/bridge.rs`（改） | `ToolAskRule::new(create 工具全名)`；为审批模式分化预留（当前产品全模式全自动，规则不弹窗；无人值守递归的确定性防线是引擎侧 `unattended_disallowed_tools`——三个写工具与 create_goal/update_goal 同列；watcher 的 eval_/aux-/agentic_ 发起方拒绝只是 claimed-string 过滤器；人工续聊的 sched- 来源可使用正常幂等请求，from_session 可省略且未认证） |
| 死工具清理 | `src-tauri/src/lib.rs`（改） | disallow 基座 `automation`/`send_later` |
| 前端 | `src/features/tools/tool-renderers.jsx`、`src/shared/i18n/{en,ja,zh}.js`（改） | 渲染卡（执行可见性）、三语文案 |
| 契约文档 | `docs/builtin-toolset-contract.md`（改） | §2 注册新族、§9 改 landed（round-8/9 更正：早前版本声称的"§6 回写短等待同步新模式"修订并未落地，短等待语义记录于本设计文档 §3.2，契约未单列） |

## 4. 实施步骤

**步骤 0 — 同步**：按 CONTRIBUTING 同步 origin/main 并链接依赖（builtin 工具集 / messaging 合入后 rebase），对齐子模块 gitlink。

**步骤 1 — MCP 包**（详见 §3.1/§3.2）：
- `server.py`：纯 stdlib NDJSON JSON-RPC over stdio，逐段镜像 session-reader 骨架（PROTOCOL_VERSION、错误脱敏不漏宿主路径、catch-all、`feature_disabled` 回退读 manifest `tool_features` + `builtin_features.json`）。
- rrule 校验：仅 `FREQ=HOURLY`(INTERVAL≥1 小时 + BYHOUR/BYMINUTE)、`FREQ=WEEKLY`(BYDAY 必填 + BYHOUR 0–23 + BYMINUTE 0–59)、`FREQ=ONCE`(AT 本地 `YYYY-MM-DDTHH:MM`、拒绝时区后缀、必须未来)；拒绝 CRON、分钟级、未知键、越界值；trim + 大写。
- spool 记录：`{schema_version:1, id, kind, task_id?, name?, prompt?, rrule?, model_id?, paused?, from_session?, created_at, idempotency_key?}`（round-8 更正：早期草案列出的 `from_title?` 两侧实现均未写入），mkstemp + os.replace 原子写。

**步骤 2 — Rust 注册**：`MCP_PACKAGES` 加项（include_str! 两文件）、`len()==12→13`；`DEFAULT_INSTALLED_MCP_TOOLS`（`marketplace/mod.rs::DEFAULT_INSTALLED_MCP_TOOLS`）加 id；`builtin.rs`/`types.rs` 镜像 session-reader 的清单与特性注册表测试。

**步骤 3 — 创建 watcher**：`ScheduledTaskState` 克隆化（步骤见 §3.3）；`ScheduledTaskState::start_creation_watcher（creation_requests.rs）` 用 `tauri::async_runtime::spawn`（勿用 tokio::spawn——原因与教训写在 `creation_requests.rs` 的 `start_creation_watcher` 文档与模块头），select! cancel/sleep(1s)，`boot_runtime` 末尾启动、句柄挂 state、Drop 取消；单文件处理逻辑见 §3.2。

**步骤 4 — Ask 规则**：`bridge.rs::SCHEDULED_TASK_CREATE_TOOL` 旁加 `pub const SCHEDULED_TASK_CREATE_TOOL`；`scope_deny_ruleset_with` push `ToolAskRule::new(...)`；list 工具不加（L0）。注：当前产品所有会话固定全自动审批，规则暂不弹窗（为审批分化 S-1 预留）；无人值守的递归自改由引擎侧 `unattended_disallowed_tools` 确定性拒绝（watcher 的 `check_sender_session_id` 隔离前缀拒绝只是 claimed-string 过滤器——from_session 可省略且未认证）。更新 bridge.rs 规则集测试（scope_deny_ruleset_with 一带）。

**步骤 5 — 死工具清理**：lib.rs `tool_policy` 闭包追加 disallow `"automation"`、`"send_later"`。

**步骤 6 — 前端与 i18n**：三语 `uiBuiltinPlugins.dataAccess` 新 scope + `uiToolDetails.tools["app-automations"]` 副标题；`tool-renderers.jsx` 加渲染卡（name/rrule/prompt 摘要）。无新 Tauri 命令 → 无 access-policy/协议测试变更。

**步骤 7 — 契约文档**：§2 注册新族（附新开族论证）、§9 原有单行替换为六条已注册记录（round-9 更正：原计划的 §6 回写未执行，短等待语义以本设计文档 §3.2 为准）。

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
| D3 | 审计日志 | 成功创建 | from_session **绑定会话 → 私有台账根**（`SessionRoots.ledger`，messaging r8-M1 同规则；round-24 M4 修订：原执行根落点把 provenance 行写进被审计模型自己的工作树，模型可用普通文件工具读取改写）append `scheduled_task_create` 记录（tool/task_id/outcome），未绑定会话保持执行根（两根同址）；另有 R4-M3 的看门狗侧影子审计（<home>/automations/audit/，以 spool 文件名为键、不依赖 from_session，省略字段不抑制本监听器的尽力记录，直接写任务存储则绕过） | ◇Rust 测试（含 shadow_audit_covers_requests_without_from_session + 绑定台账落点钉子） | P1 |
| D4 | 错误脱敏 | 构造各类失败 | 错误文本不含宿主绝对路径 | ◇python（镜像 ○`test_error_messages_do_not_leak_absolute_paths`） | P0 |
| D5 | 写入范围 | 请求、回执、隔离与自动化领域目录，另写入会话根及影子审计 JSONL；不修改会话正文，不安装 OS 调度器 | 代码审查 + 测试 | P1 |
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
| H7 | `./scripts/fork-guard.sh --fast` | 通过（零 fork 内容变更；分支不含任何 fork 登记册/gitlink 提交——gitlink 即基座 main 的 `083cebfd9`，round-27 M2 改真） |
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
| 依赖未合入 main（round-30 S4 改真） | ~~builtin 工具集/messaging 目前仅在 `luzeyang-dev`~~ **已失效**：messaging 经 #627、session-reader 经 #585/#586、本分支的基座即 main——该风险已消除 |

## 8. 边界与已披露事项

- 交互会话无阻塞确认（决策记录 §2）；`tasks`/`github` 基座工具可见性问题本次不处理。
- **无人值守递归防护的真实边界（round-8 M1 修订）**：确定性防线是引擎侧 `unattended_disallowed_tools`（deny 名单）+ round-8 起仅对**无人值守运行引擎**关闭 subagents、交互续接以正常委派重建（round-33 M8 改真；`agent(inherit_disallowed_tools=false)` 无法再剥离 deny 名单——子运行时本会保留 MCP+自动批准，而 posture 名单不含本族三写工具）。剩余通道有三（round-27 M1 改真；round-30 REQUIRED-2 增补第三并封堵）。其一，**无需 shell**：定时任务钉死 Yolo（`tasks.rs`）→ trust mode 下 `write_file` 接受绝对路径，`~/.pinvou3` 不在默认拒绝子树内，而无人值守 deny 名单只含具名工具——故仅凭文件工具即可向 `~/.pinvou3/task-requests/` 写入合法 spool 记录（claimed-string 过滤器的既有让步）。其二，**messaging spool 是同类的第二通道，且可跨会话洗白**：`features/messaging/mod.rs` 自述该目录为"未经认证的本地侧门"（无 digest 绑定、`from_session` 可选）——无人值守运行的文件工具写入后，投递会把普通目录回合注入任意普通会话，而该会话目录含本族三写工具（即 round-9 M2 在工具层修掉的间接性的文件写孪生）。其三，**registry 启动器曾可扩充同回合工具面**：`registry_sync`/`start_registry_mcp_server` 随应用白名单对 sched- 引擎可见，自动批准放行，而宿主全访问 hold 名单只含 `start_mcp_server`——启动用户缓存的 registry 候选会让其 `mcp_<name>_*` 工具以本盾看不见的新身份加入同一回合。**本波已把两个启动器加入 `unattended_disallowed_tools`**（引擎侧具名 deny 名单，round-30 REQUIRED-2），该逃逸通道就此关闭；把启动器加入宿主 hold 名单是 CodeWhale 侧的彻底修法（按 fork 边界另行上游跟进）。
- 现有面板"对话创建"流程保留不动，两条通道汇合到同一个 `create_task` 领域函数。
- 零 CodeWhale / fork 内容变更；本分支**不含** fork 登记册刷新与 gitlink 追平提交——gitlink 随基座 main 对齐（round-27 M2 改真），`docs/fork-modifications.md` 由各落地 PR 各自维护；本计划文档本身随本分支入库，作为后续实施的评审基线。
- 领域层 `parse_rrule` 实际支持 CRON，工具层收紧为三种子集是有意的产品约束；若未来要放开，只改 server.py + watcher 的子集校验，领域无需动。
- **存储读上限（round-31 M3 回填 EN 修订）**：单条读在被尺寸/类型策略拒绝时报告"不可读记录"，`list` 跳过策略拒绝/损坏的记录，权限/IO 失败按错误返回而不是谎称存储为空；超过 256 KiB 上限的任务在列表中仍不完整——模型的建前去重会因此重复创建（既知让步）。
- **运行结果不可观测（round-27 minor 12）**：`list`/`read` 只暴露 status/nextRunAt/lastRunAt——模型无法读取运行输出（session-reader 硬拒 `sched-` 目标，无旁路）；被替换的基础 `automation` 工具在此处更丰富（read 含运行记录）。单一用户语义下面子可接受，特此披露。
- **特性开关文案先于设置页（round-27 minor 13）**：`scheduled-task-automation` 开关的三语文案已入库，但本 PR 未随附自动化设置页——开关目前经命令/配置可达（契约 §3.1 的"随特性落地接入各自设置页"以跟进 PR 履行）。
- **IO 范围**：定时请求审计使用既有目录句柄原语与新增的句柄相对追加操作。本 PR 已移除先前对全局 audit.rs/open_private_append_file 的行为改动；消息、computer-use、诊断保持原行为。回执采用独占唯一临时文件，旧的截断临时文件辅助函数已移除。


---

## 9. 实施扩展记录：CRUD 全覆盖（2026-09-29，同分支追加定稿）

首版设计（§1–§8）只覆盖创建 + 列表。按评审意见扩展为完整增删改查，本节为其评审基线；§1–§8 中与本文冲突之处以本节为准。

### 9.1 工具面（替换 §3.1）

| 工具全名 | 等级 | 参数 |
|---|---|---|
| `mcp_app-automations_create_scheduled_task` | L1 | 同 §3.1 |
| `mcp_app-automations_read_scheduled_task` | L0 | `task_id`(必填)；返回单任务全量字段，**含 prompt**（更新前检视用） |
| `mcp_app-automations_list_scheduled_tasks` | L0 | 同 §3.1（不含 prompt） |
| `mcp_app-automations_update_scheduled_task` | L1 元数据 / L2 覆盖 | `task_id` + name/model_id/paused 可立即更新；prompt/rrule 覆盖需到面板检查提交，工具不执行 |
| `mcp_app-automations_delete_scheduled_task` | L1 有限例外 | `task_id`；用户明确要求且目标唯一时按现有权限执行。目标有歧义时先向用户确认。复用队列回执，不提供完整定义备份/撤销。 |

### 9.2 设计决策补充

| 决策点 | 结论 | 理由 |
|---|---|---|
| 粒度 | 五个 `verb_noun` 工具，不合入 mode 枚举 | 契约 §4.2 偏好 one tool + mode，但读写审批语义不同：合入会让 L0 读也吃 L1 Ask 门（骚扰）或写操作失去门控；session-reader 三工具先例同理 |
| 删除授权 | 用户于 2026-10-21 明确选择 L1 有限配置删除例外，不强制二次确认 | 使用列表/详情核对目标；重名、指代不清或信息冲突时必须先向用户确认，不能猜测。误删配置可能需手工重建，最小历史归档不是完整定义备份或撤销。 |
| spool 扩展 | 记录加 `kind`(`create`/`update`/`delete`) + `task_id`，其余字段全部可选 | 契约 §4.4 只允许增字段：旧 create 记录经 watcher 默认 `kind=create` 继续有效 |
| 幂等命名 | `sha256("<from_session>\|<kind>\|<task_id>\|<key>")`；key 必须随 from_session | 命名空间按发送方收敛，不随 from_session 缺省退化为全局；同键不同操作不互踩（创建与更新可共用一个键而不互相覆盖） |
| 目标预检 | 更新在 server 侧检查存在性；删除即使对象已不存在仍到达回执/领域层 | 保证删除重试不会在读取既有成功回执前失败；领域层区分明确 NotFound 与 IO/解析错误。 |
| Ask 规则 | update/delete 同 create 一样注册 typed Ask 规则；read/list 不加 | 读操作无应用侧确认规则，但仍受 feature/scope 与底座 Plan-mode 门约束（round-31 M3 回填 EN 修订）；规则在当前全自动审批下不弹窗，为审批分化预留；无人值守的递归自改由引擎侧 `unattended_disallowed_tools` 确定性拒绝（watcher 的 eval_/aux-/agentic_ 发起方拒绝只是 claimed-string 过滤器；人工续聊的 sched- 来源可使用正常幂等请求） |

### 9.3 验收矩阵追加（接续 §5 编号）

| # | 场景 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|
| I1 | 安全更新端到端 | name/model/paused 更新落盘并刷新；prompt/rrule 覆盖返回 confirmation_required 且不变更 | Rust + Python；实际 Linux 面板提交见下文 | P0 |
| I2 | 准确目标删除与重放 | 明确且无歧义的请求按 ID 经监听器/领域删除；同键崩溃重放及不同键重复删除均成功。目标不明确时模型先向用户确认 | Rust + Python；真实模型歧义/删除验收见下文 | P0 |
| I3 | 读详情 | read 返回含 prompt 的全量字段；未知 id 显式 not found | ◇python + 手工 | P0 |
| I4 | update 校验 | 缺 task_id / 零字段 / CRON rrule 均拒绝 | ◇python + ◇Rust `update_request_validation_requires_target_and_a_field` | P0 |
| I5 | delete 校验 | 缺 task_id、带多余字段均拒绝 | ◇python + ◇Rust `delete_request_validation_rejects_extra_fields` | P1 |
| I6 | Ask 规则扩展 | 规则集含 update/delete 两条 Ask；read/list 不含 | ◇Rust `scope_deny_ruleset_asks_for_scheduled_task_create` 扩展 | P0 |
| I7 | 排水加固（#680 波） | pending 超过 256 时排序尾部被隔离且不落库：尾部记录在移动前探测真实原因（损坏为 unparseable、超大/不可读为 unreadable or oversize），仅真正超限的记录保留 ceiling 标签；移动前快照重验把探测与改名之间的替换体留给其自己的新 pass（still_matches→rename 的残留窗口已收窄并如实披露）；移动前快照重验（divergence guard）将探测与改名之间落下的替换体留给其自己的新 pass，不按旧字节的标记作答；digest 绑定的 ok 回执抑制失败标记（回执照常应答），可解析尾部的处置留审计行。卡住 episode（Poison/重试耗尽臂与 park 门控）的审计行与失败标记每次 episode 只写一次（进程生命周期内），身份为 payload digest（尾部臂的一次性是结构性的：其审计与标记仅在成功移动内触发）：同字节重发体保持去重，更正后的重发体（服务端会解除旧 ok:false 标记）按身份变化重开新 episode；被解除的标记在保持审计去重的同时补写（ensure-presence，含 park 门控路径的 parked 状态说明）。停放记录的 un-park 由 payload 变化决定而非时钟：同字节 mtime bump 保持 parked 且零次重应用，更正体重绑并应用；各终态臂与 maintenance 在记录移动时清理 episode 身份。非 UTF-8 文件名在计数前先行隔离，不占用批槽位（枚举窗口截断在隔离之前，其占用的窗口槽位随积压消退释放） | ◇Rust `poison_park_corrected_respool_opens_a_fresh_episode` / `poison_park_same_bytes_bump_stays_parked_and_marker_is_restored` / `retry_park_same_bytes_bump_never_reapplies_corrected_payload_rebinds` / `poison_park_episode_clears_when_the_quarantine_finally_succeeds` / `poison_arm_clears_the_episode_entry_when_its_quarantine_succeeds` / `retry_arm_clears_the_episode_entry_when_its_quarantine_succeeds` / `corrupt_ceiling_tail_names_the_real_cause` / `oversize_ceiling_tail_names_the_real_cause` / `ceiling_tail_with_landed_receipt_writes_no_failure_marker` / `non_utf8_squatters_do_not_starve_legitimate_records` / `tail_snapshot_divergence_leaves_the_replacement_for_the_next_pass` / `vanished_ghost_skipped_and_dangling_link_answered_terminally` / `deduped_episode_repairs_a_marker_the_diverged_pass_skipped` | P1 |

§6 测试映射相应扩展：python 套件覆盖 I3/I4/I5，smoke 旅程覆盖 read/update/delete 校验路径，渲染卡测试覆盖三操作结果解析；I7 的排水加固波由 Rust 套件覆盖。

## 评审补充：排水边界与证据保留

- 非空排水轮仅从权威 settings.json 读取 `disabled_builtin_features`，不加载全量偏好、不探测语言、不迁移凭证。配置损坏或不可读时暂停排水，同一失败持续期仅记录一次；修复后下一轮恢复。空队列无需读取配置。
- 已消失的请求名清理重试和停放状态；非 UTF-8 文件名按原名字节隔离，无法生成协议回执。一轮内成功应用的请求最多触发一次面板刷新。
- 隔离复用既有跨平台句柄相对禁止覆盖移动原语并重试冲突后缀，移除重复的定时任务专属 OS adapter。Windows 命名的共享文件系统测试提供原生执行覆盖。
- 14 天回执清理遇到同名 spool 请求时保留标记，防止已应用但无法删除的请求在回执过期后再次执行。请求名消失后恢复清理；隔离证据和服务端遗留临时文件仍遵循 14 天策略。
- 模型绑定更新失败依赖存储自身的快照恢复，移除多余的第二次回滚写入，避免覆盖并发 CLI 重新绑定；此前已持久化的任务字段保留，可整体重试。
- **IO 范围**：定时请求审计使用既有目录句柄原语与新增的句柄相对追加操作。本 PR 已移除先前对全局 audit.rs/open_private_append_file 的行为改动；消息、computer-use、诊断保持原行为。回执采用独占唯一临时文件，旧的截断临时文件辅助函数已移除。
- 插件卡片给出真实关闭路径：在 settings.json 的 `disabled_builtin_features` 中加入功能标识，不再暗示已有设置界面开关。重复请求文案兼容已处理回放与替换排队请求两种情况。首发包版本为 `1.0.0`。

| 验收场景 | 预期 | 验证 |
|---|---|---|
| 排队上限 | 每轮最多 256 条尝试应用；超额请求留存隔离证据与失败回执，不覆盖旧证据；洪泛警告持续期仅一次 | `pending_ceiling_quarantines_the_tail_with_failure_markers`、`quarantine_keeps_colliding_evidence` |
| 保留期与临时文件 | 超过 14 天的终态与孤立临时文件被清理；同名请求仍在队列时保留回执 | `prune_stale_state_sweeps_old_markers_and_stray_tmp`、`retention_preserves_receipts_for_stuck_spool_records` |
| 夏令时墙钟 | 双层拒绝 ONCE 时间落在缺口或重叠；Python 缺 tzdata 时 Rust 为最终权威 | 既有 Rust 与 Python ONCE/DST 用例 |
| 配置损坏 | 暂停本排水器；修复即恢复，不重复迁移凭证 | `a_drain_pass_coalesces_refresh_and_pauses_on_bad_settings` |

## Round-32 评审补充

- **共享功能开关**：工具同时属于 A、B 时，只要任一功能开启就仍可用，只有两者都关闭才禁用。双功能夹具覆盖全开、分别关闭其一、全部关闭；模型错误提示与此规则一致。
- **懒加载导出**：时间线必须加载 `ScheduledTaskToolResult`，由它解析 MCP 信封后提供展示卡片。直接加载 `ScheduledTaskToolCard` 会跳过适配器并退回原始输出，现已锁定完整导入/导出映射。
- **停放时间与预算**：仅 `parked_at < mtime < now` 的替换可解除停放，未来或备份恢复时间不会每轮换取一次应用机会。排队与停放请求的重试预算均按**文件名**累计：同键请求已失败两次后替换内容，仍只剩三次总预算中的一次；合法替换解除停放也不重置已花预算。名称消失或终态才清理计数，以限制同键重植。新幂等键应仅用于有意的不同操作，仍需考虑已披露的至少一次投递重复窗口。
- **影子审计轮转**：常驻的 `automations/audit/workflow_audit.jsonl` 超过 2 MiB 后在追加前轮转，只保留一个 `workflow_audit.jsonl.old` 段。正常增长为每段阈值加一条事件，重启后按磁盘状态继续判断。既有更大的历史段先保留一次，下一次轮转时替换。复用尽力而为的日志原语，不串行化多个应用进程；轮转失败时保留旧证据并向当前日志追加新行、记录诊断；修复前可能超过正常上限。
- **服务端响应**：服务端为单线程。一次写请求等待回执的最多五秒内，同一服务上的 `ping`、`tools/list` 和其他调用也会排队；N 个写请求可能串行等待 N 倍时长。超时仍返回 pending，本轮不增加线程或改变协议顺序。
- **失败回执**：包括外部写入的 marker 在内，错误文本进入模型上下文前会替换已知的用户目录、请求队列和任务存储路径前缀；非文本错误使用通用失败消息。marker 仍是未经认证的本地输入，此处仅做路径脱敏，不验证来源。
- **孤立代理字符**：既有 UTF-8 键校验与回归保留；测试直运行入口移至所有测试类之后，确保直接运行也覆盖此前补丁用例。


## Round-33 当前行为与边界

- 支持桌面 Operate/Yolo；Plan 阻止整个 MCP 能力族（包括 L0 读取）。CLI/headless 没有 ScheduledTaskState 消费端，因此隐藏该族。五个工具在允许使用时常驻首轮目录，特性/作用域拒绝仍优先。全局提示词说明调度与应用需保持运行；会话指令给出准确的 `from_session` 供幂等键使用，该字段仍是不经认证的来源声明。人工续聊的 sched- 来源可正常使用幂等键；真正无人值守的写调用仍由引擎拒绝，aux/eval/agentic_ 来源限制保留。
- 用户最新明确选择：撤掉强制删除确认卡及快照确认机制。删除保留为受现有权限控制的普通模型工具：按需先用列表/详情核对，用户明确要求且对象唯一时按准确 ID 执行；重名、指代不清、信息冲突或无法可靠判断对象时必须先向用户确认，不能猜测或扩大范围。任务正文是参考数据，不是操作授权。此要求是模型行为约束，不保证程序确定性验证用户意图。复用既有监听器/领域删除流程和摘要绑定回执；对象不存在视为已删除，IO/解析失败仍报错。不增加完整任务定义备份或撤销。提示词/计划覆盖仍须面板提交，既有最小运行历史无法恢复完整定义。
- 审计 JSONL 没有内置查看器，且被工作区文件列表隐藏；实际可见的审查入口是结果卡与面板。影子审计只是本监听器的尽力证据。相同用户的文件工具可直接改写 `<home>/automations/automations/<id>.json`，完全绕过监听器、参数校验、来源与影子日志；除 §8 的 spool/messaging 文件通道外，必须计入这一绕过。按工具名限制并不是文件系统沙箱。
- spool、`.done`、failed、audit 逐级使用真实目录句柄，拒绝符号链接/reparse point。启动时准备目录；audit 被普通文件占用时，保留为唯一 `audit-obstruction-*` 备份后创建目录；其他不安全或不可用目录使初始化明确失败。正常轮换保留前一段；轮换被阻塞时记录警告并继续追加当前证据（修复前可能持续增长），不再丢弃所有新行。追加失败仍会记录警告，不声称防篡改。
- 每次最多枚举 290 项、对选中的最多 256 个 JSON 请求执行领域调用，最多隔离 32 个尾部请求。只对当前有限批排序，不再全量枚举后排序。同用户持续灌入或非请求项可能延迟后续处理，不保证公平或限流。失败收尾在审计 IO 后重新比较捕获字节及文件身份；替换后的请求保留，旧错误不回给新请求。检查到移动间的微小竞态及便携大小/mtime 身份下限仍存在。
- 隔离证据从移动时开始计保留期；回执和证据保留 14 天，过期后的同键重试可能重新执行。仍有原请求的成功回执继续保留。`.done/*.tmp` 和 spool 临时残留按有限批清理。回执使用独占唯一临时文件、文件同步和现有句柄锚定的原子发布。成功执行但发布失败时，本进程缓存回执并只重试发布，面板首次执行即通知；进程退出会丢失缓存，执行到回执间崩溃仍可能重复创建。
- 手工编辑 settings.json 后须重启，使 MCP 镜像与排水门控一致；当前没有界面开关。manifest 与 MCP server 均为 1.0.0。
- 无人值守 shield 和禁止委派只作用于该次运行的引擎；刷新广播读取当前无人值守标志。交互/无人值守姿态不匹配时重建引擎。无人值守任务中依赖委派的脚本仍不支持；用户继续聊天时恢复正常委派能力。
- 集成测试 `real_stdio_server_drains_through_real_rust_domain` 启动真实 Python MCP 服务，通过 Rust 监听器及领域存储排水。它本身不是 A1 真模型测试；另行完成的真实桌面验收记录见下文。
- 进程内 ToolSpec 是可减少可写 spool 协议的架构备选；迁移既有内置包、待处理请求和幂等回执需要单独评审兼容方案，本轮不实施。当前修复复用领域 CRUD 和现有文件系统原语，不新增调度器，也不改 CodeWhale。

新增标识冲突时拒绝替换同名自定义 app-automations 包；须先保留并重命名自定义包。宿主启动参数显式传入实际请求/存储目录，子进程过滤 PINVOU3_HOME 不再使队列错位。

### 真实桌面验收，2026-10-21

在 Linux arm64 的实际 Tauri/Wry 开发窗口中，使用真实 Ornith-1.5-35B-A3B-NVFP4 模型验证候选。使用隔离的 `PINVOU3_HOME` 和独立应用标识，没有改动用户已有任务或会话。临时本地驱动在真实渲染器中派发 DOM 输入/点击事件，同时核对可见面板详情、实际 IPC DTO、落盘回执、运行记录和会话正文。驱动不作为产品入口发布。

- **A1 通过：**普通聊天创建每天 08:30、保持暂停的任务。模型直接调用常驻加载的 list/create 工具，没有调用 `tool_search`；create 在 604 ms 内返回确认的任务编号和名称（完整模型回合为 9.042 s）。实际面板展示了名称、完整正文、每日时间和暂停状态。
- **A4 已观察触发通过：**另一个普通聊天创建本地 17:34（UTC 09:34）的 ONCE、HOURLY、WEEKLY 任务。既有调度器于 09:34:11.919 将三者入队；真实模型回合均在 09:34:20.254 前完成，三个会话落盘的助手回复均为 `QA_R33_FIRED`。ONCE 自动暂停且没有下次执行；HOURLY 仍启用，下次为 UTC 10:34；WEEKLY 仍启用，下次为 10 月 28 日 UTC 09:34。再观察 32 秒后，三者仍各只有一条运行记录。这验证首次实际触发及下次执行计算，没有声称已等待下一小时或下一周再次触发。
- **面板提交通过：**模型尝试纠正 prompt 时收到 `confirmation_required`，存储正文保持原值；随后在既有面板编辑并保存，修正确实落盘。删除确认框中点取消保留暂停夹具，点确认才删除。打开 ONCE 的已完成运行行，实际展示了该运行的会话和回复。

本次验收范围为 Linux 开发桌面，未手工运行 Windows/macOS、打包发布版界面或 Web 客户端；既有确定性门禁与 CI 是独立证据。

补充失败边界：有限批优先处理已记录的重试/停放/待发布回执，目录顺序变化不会扩大这些表。创建先持久化暂停记录，工作间、模型与类型保存完成后才按请求启用；工作间失败回滚暂停记录。证据时间戳无法修改时保留为 .untimed-*，待手工清理，不按旧时间提前删除。

### 删除产品取舍与对照

用户明确接受：指令明确且目标唯一时直接删除；无法分辨是否为正确对象时，先在对话中向用户确认，不增加专门确认卡或完整定义备份。简单任务通常能较快重建，复杂正文也可能重建成本较高；风险可接受不等于零风险或自动恢复。删除复用既有取消运行/历史处理流程，不会撤销过去运行造成的影响。

[Claude Cowork](https://support.claude.com/en/articles/13854387-schedule-recurring-tasks-in-claude-cowork) 的文档包含对话设置时核对拟定参数，以及编辑、暂停和删除任务。[Cursor Bot routines](https://prod.cursor.com/help/grok-bot/routines) 明确说明删除需确认且不能撤销。这里只以用户交互模式为对照，不据此声称内部授权实现或未公开的备份策略相同。 这些文档不能证明必须新增强制删除卡片；本项目采用上文用户明确选择的有限例外规则。

### 目标歧义与直接删除：真实模型验收

在重编译后的 Linux 桌面中，准备两个同名但小时间隔不同的暂停夹具。普通对话要求删除该名称时，真实模型列出两者并询问用户所指对象，没有调用变更工具，两条记录均保留。用户随后指定“删除每两小时的那个，保留每小时的那个”后，模型只按正确 ID 调用了一次删除工具，收到应用成功回执后再报告完成。实际存储与 IPC 列表均表明仅指定目标被删除，每小时任务及其他干扰夹具保留，没有额外确认卡。剩余夹具均保持暂停。此结果证明本次观察到的模型行为，不保证所有模型永远正确理解意图。


### Round-36 队列恢复与同键替换

停放的终态记录和未发布回执不占应用或尾部隔离名额。每轮对未更换的停放记录仅重试带身份检查的终态移动，不再执行领域操作；合法的新替换仍保留按文件名累计的已用重试预算。内存证据达到有界容量时，新请求保持排队并按故障持续期告警，直到存储恢复，不再收到虚假的上限失败。隔离成功后自动释放容量，无需重启。目录枚举仍有界，本次不保证洪泛下的公平性。

同键替换若发生在 watcher 正在应用旧内容期间，两份内容都可能应用。这是不依赖进程崩溃的另一个至少一次窗口。重试回执仅描述本次应用，并明确提示旧请求也可能已应用；再次重试前应检查面板。不承诺恰好一次或取消已经开始的旧请求。

---

## 10. Scheduled messages (`session_message`, revised for R19)

A scheduled message reuses the existing automation scheduler and CRUD flow. At
execution it delivers the stored body into an existing ordinary session: steer
when a turn accepts it, otherwise the EnginePool messaging new-turn path. It
creates no separate scheduled-run conversation and no new scheduler or store.

### 10.1 Surfaces and product decisions

| Surface | Supported behavior |
|---|---|
| MCP create | Supplying `target_session` derives `session_message`; it is mutually exclusive with an explicit kind. Ordinary create remains unchanged. |
| MCP read/list | Both expose `kind`; read also exposes `targetSession`. They read the capped kinds registry without rewriting it. Unreadable, corrupt or newer metadata is an error, not proof of an ordinary task. |
| MCP update | Retarget an existing scheduled-message task; an ordinary task cannot be converted. Prompt/rrule replacements require review and submission in the existing panel under #628's product decision. |
| Panel | Displays the target and full body, edits the body/schedule and manages pause/delete/run-now. It cannot create or retarget scheduled messages. |
| Timeline tool card | Shows the requested target for a matching receipt (or a request still marked pending), plus a 120-character body excerpt. A mismatched replay hides the new requested target. This is not a full-body review or deterministic authorization UI. |
| CLI | List/show preserve the raw kind and target. `run` refuses this kind with a product-host remedy; creation and retargeting are not implemented. An update may replace its body only within 32,768 Unicode characters. Unknown kinds and unreadable kind metadata refuse prompt replacement. Other documented CLI mutations retain their existing scope. |

Self-addressing is allowed. Task access follows the existing local-user permission
model; there is no creating-session ownership check for retargeting. Tools must
follow explicit user intent and ask when the task identity is ambiguous, per #628.
This guidance does not prove intent or prevent prompt injection. Delivery injects
the raw body as a target instruction, without a machine-readable origin envelope
or a delivery audit line in the target ledger. CRUD auditing and the run result
provide limited provenance; they do not provide messaging's origin/audit parity.
Prompt/rrule tool replacement remains panel-only. No dedicated confirmation card,
definition backup, undo system, or new authorization protocol is added here.

### 10.2 Validation and persistence

- The body cap is **32,768 Unicode characters** after trimming. The MCP server
  applies it to all task kinds; the domain/panel conservatively enforce it for
  any kind not definitively ordinary chat, and the executor rechecks it for
  scheduled messages. CLI session-message replacements share the app constant.
- Target IDs have an ASCII letters/digits/`-`/`_` charset and a 128-character
  maximum. Leading `_` is reserved; case-insensitive `sched-`, `aux-`, `eval_`
  and `agentic_` targets are refused. Server, watcher and domain/executor apply
  identity checks. Exact directory-entry spelling and a regular non-symlink
  session file are required at server, domain and each delivery.
- **ACP/code classification is checked at domain and executor only**, from
  `session-agents.json`. A missing index is the store's normal empty-index case;
  a present unreadable/corrupt/newer index fails closed. This does not consult
  live AcpPool recovery, and cannot identify code ownership absent from the index.
- Create stages a **Paused** automation, persists workspace/model/kind+target,
  then activates only if requested. Workspace failure rolls back the definition.
  A crash before activation can leave a paused partial record, not an Active
  kind-less chat. Cleanup is best-effort when its own disk write fails.
- Update reads a mandatory original definition and a fresh readable kind registry
  before mutation. Its app AutomationManager mutex spans definition and target
  writes; executor dispatch reads the body and kind/target under that same mutex.
  A dispatch therefore sees an entire old or new pair. The lock is released before
  engine delivery; a later edit cannot recall a pair already captured.
- A target-save or workspace-save error attempts to restore the original mutable
  definition (name/body/schedule/model/status); store-level target failure restores
  its in-memory entry as well. Failure to obtain the original snapshot stops the
  update. These files are **not one crash-atomic transaction**: process death
  between writes, rollback-write failure, and external CLI/file writers outside
  the app mutex remain residual risks. Stop external writers while the app runs,
  inspect and repair the definition/target pair after a failed rollback.
- For scheduled messages, delivery uses the complete configuration current when
  execution starts. A queued TaskRecord's prompt is its enqueue-time snapshot;
  after editing it can differ from the body actually delivered. The result records
  the delivered destination, not an immutable copy of the delivered body.
- Production dispatch refuses unreadable/quarantined whole kinds metadata rather
  than downgrading to chat. An absent entry in a healthy registry still means chat;
  external removal of an individual entry cannot be distinguished from that case.
- `target_session` is additive and old records read normally. **Downgrading is not
  write-safe**: an older typed app can drop target fields when rewriting the
  registry. Stop the app before downgrade; preserve the registry privately and
  restore it before re-upgrade, or recreate affected tasks with their destinations. A
  scheduled-message entry without a target fails closed after re-upgrade.

### 10.3 Execution and history boundaries

- A completed delivery result means the engine accepted steer/new-turn handoff;
  it does not mean the target's model completed its response. The retained linked
  completed TaskRecord result stores the destination using a shared formatter and
  parser. Retargeting A to B leaves old retained rows showing A. Missing/pruned
  results, malformed text or removed kind metadata stay unknown. Deleted archives
  still lose this footer when kind metadata is removed. The panel shows a localized
  missing-target message for identifiable scheduled-message runs; it does not jump
  to the destination session.
- The delivery attempt shares a 30-second bound across steer and new-turn handoff.
  Cancellation is selected before a ready delivery result; after the delivery
  future returns success there is no await that could turn it into Canceled in
  this executor. Cancellation/timeout stops waiting on an in-flight future; it
  cannot promise to retract effects already handed to another task. If delivery
  and cancellation are both ready, the biased cancellation branch can record
  Canceled even after the engine accepted the message. This follows from branch
  ordering; the live acceptance did not exercise that race. No specific
  timeout-after-success window is asserted as verified.
- An accepted steer can still be lost before the target consumes it. This inherited
  EnginePool window permits Completed without consumption. A crash between engine
  acceptance and durable task/run reconciliation is not an exactly-once receipt;
  recovery or manual retry can duplicate delivery. There is no delivery-id dedupe
  journal, and automatic replay of every crash is not guaranteed.
- A recurring slot within the scheduler's 60-second grace is eligible only if
  there is no existing slot record or active run. An active run skips the slot;
  an older missed recurring slot jumps directly to the next future occurrence.
  A missed ONCE can run arbitrarily late. After it is enqueued, ONCE has no future
  slot and pauses even if delivery fails; retry requires an explicit new run or
  task. The next recurring slot is a new attempt, not guaranteed recovery of the
  failed message (the task may have been edited).
- Manual run-now has no in-flight check and can overlap a scheduled run. Pause
  stops future scheduling without canceling an existing delivery. The task's model
  binding is display-only for this kind: delivery uses the target's current live
  turn or its own next-turn model and permission settings.
- Disabling the MCP feature removes its tools without stopping existing scheduled
  messages; manage those in the panel. CRUD spool idempotency does not deduplicate
  delivery. Delete retains #628's ensure-absent success for retry after success.
- Watcher shape validation handles invalid fields before apply. The remaining
  string classifier recognizes narrow domain dispositions (wrong retarget kind,
  explicit missing task/target, ACP/code, expired ONCE). Generic I/O, parse and
  index-read failures use bounded retry. A typed domain-error migration remains
  debt; arbitrary OS text is not a permanent-error contract.

### 10.4 Verification scope and remaining debt

R19 adds CLI kind/target inspection and safe-update regressions, MCP read/list and
invalid-target tests, strict registry/index failure tests, target-store restore,
mandatory pre-update reads, app-lock body/target interleaving, and a real
TaskManager → executor → retained result → target-parser round trip. UI render
fixtures cover both destination rows and unknown-target text. A padded target is
normalized consistently in validation and receipt digests. Parent #628's paused
create/workspace rollback tests remain part of the combined branch.

These are deterministic local regressions. Exact commands, current counts, entry
sizes, live/manual acceptance and platform limitations are recorded in the PR's
latest verification section; historical review measurements are not current
candidate measurements. Parent #628's live A1/A4 ordinary-task acceptance does not
by itself verify this kind's delivery.

R19 Linux Tauri/Wry acceptance also exercised this kind with the configured real
model: a conversation created a self-targeted scheduled message through MCP; the
2026-10-21 11:04 UTC ONCE slot was enqueued at 11:04:06.469 UTC and produced the
expected model response. The actual panel showed the target, full body and
completed destination footer. An IPC/domain retarget followed by manual run-now
reached a second ordinary session and produced the expected real response, while
the old run retained its original destination. A further 30-second observation
left exactly two completed runs, one delivered body/reply per target, no scheduled
run conversation and a paused fixture task. Real stdio read/list also projected
the kind/target correctly. Test app and driver were stopped afterward. Live busy
steer, packaged release, Web UI and native Windows/macOS acceptance were not run.

Remaining debt is explicit: the live EnginePool steer/fallback order still uses
bounded source contracts rather than an extracted behavioral fake; per fire there
is synchronous session directory scanning, a full uncapped ACP index parse,
preferences I/O, and a capped full kinds-registry read on the single worker.
History projection still performs result/metadata lookups per row. No performance
optimization, exact-once receipt, origin envelope or native Windows/macOS manual
acceptance is claimed. The approved entry budget remains 506,000 B on both builds.
