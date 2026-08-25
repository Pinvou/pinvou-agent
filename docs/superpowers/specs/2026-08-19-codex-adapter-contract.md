# Codex Adapter 合同与兼容矩阵

> 状态：**草案（draft）**——方法/事件映射基于 codex-cli 0.139.0 第一方 JSON Schema；协议捕获任务（§6）完成后升级为 frozen。
> 日期：2026-08-19
> 上游：蓝图 §10（统一 Runtime）、§11（检测与认证）；决策冻结 D-07。
> **重要澄清**：OpenAI codex 的 `app-server` 与 CodeWhale（DeepSeek CLI 系 fork）自带的 `codewhale-app-server` 是**两套无关协议**（后者方法为 `thread/message`、SSE RuntimeEventEnvelope，见资产盘点文档）。本文档只针对前者。

## 1. 事实基础（第一方，可复核）

- 本机安装：codex-cli **0.139.0**（npm 全局，`C:\Users\c24894\AppData\Roaming\npm\codex`）。
- 协议 Schema 获取方式（任何机器可复现）：

```bash
codex app-server generate-json-schema --out <dir>
```

已生成顶层聚合 Schema 与 `v1/`、`v2/` 版本目录，存于本仓库 `tmp/spike-fsync/codex-schema/`（临时物；Adapter 开工时选择实际使用的方法子集作为 fixture 提交到 `agent-adapter-codex/tests/schema/`）。文件总数会随生成器扩展而漂移，不作为兼容合同。
- 传输：stdio JSON-RPC 2.0（与 Multica 洁净室调研观察一致，`docs/research/2026-08-18-multica-agent-cli-control.md`）。
- 帧格式：**换行分隔 JSON**（每消息一行；2026-08-19 实机探针确认，见 §6.1）。两个解析注意点：响应**不含 `jsonrpc` 字段**（形如 `{"id":..,"result":..}`），不得按 JSON-RPC 规范强制要求该字段；codex 的 stderr 诊断（如 models cache ERROR、MCP 错误）会与 stdout 混流，**Adapter 必须只从 stdout 读 JSON-RPC，stderr 单独走 R2 诊断通道**。
- stdout 同时承载文本增量、server→client 审批/输入请求、取消响应和终态，Adapter reader 必须持续 drain。不得以停止读取 stdout 的方式给 R1 背压；R1 先进入 Node `main` spool/合并路径，R0 进入独立 `control` spool/传输流。达到 hard pressure 时停止新 admission 并请求 interrupt/terminate，而不是堵塞混合协议管道。

## 1.1 零成本探针已确认的方法（2026-08-19，不消耗配额）

| 方法 | 结果 |
|---|---|
| `initialize` | ✅ 返回 `{userAgent, codexHome, platformFamily, platformOs}` |
| `account/read` | ✅ 返回 `{account:{type, email, planType}, requiresOpenaiAuth}`——登录态可经协议查询（Adapter 合同 §5 的 AuthExpired 判定有了实测依据） |
| `model/list` | ✅ 返回模型清单（含 supportedReasoningEfforts） |
| `thread/start` | ✅ 返回 `{thread:{id, sessionId, ephemeral, modelProvider, createdAt, ...}}`；threadId 即 native_session_id |
| `thread/list` | ✅ 返回历史线程（含 preview 摘要） |
| 附带发现 | thread/start 触发一串 `mcpServer/startupStatus/updated` 通知（本机含失败项）；`thread/started` 通知与 thread/start result 内容重复 |

**含义**：① MCP startup 通知在映射表中按 `vendor`/R2 处理（§3 已覆盖），但 **thread 创建延迟包含 MCP 启动**——Adapter 的 create 计时与诊断要区分"MCP 慢"与"codex 慢"；② 响应/通知重复意味着投影**幂等去重**（事件 schema §4.3 已有测试）在真实流上即刻需要，不是理论需求。

## 2. Adapter 使用的方法子集

Adapter 只依赖以下方法（全量 ~80 个 client 方法不使用；无关面越小，兼容矩阵越小）：

| runtime-api 操作（蓝图 §10.2） | codex 方法 | 关键参数 |
|---|---|---|
| probe | `codex --version` + `codex doctor`（shell） | — |
| auth_status | `account/read`、`account/rateLimits/read` | — |
| start_auth | `account/login/start` / `account/login/cancel` | — |
| create | `initialize` → `thread/start` | cwd, model?, sandbox?, approvalPolicy? |
| resume | `initialize` → `thread/resume` | threadId（即 native_session_id） |
| send / steer | `turn/start` / `turn/steer` | threadId, input, cwd?, effort?, model? |
| interrupt | `turn/interrupt` | threadId |
| import_context | `thread/inject_items`（v2，若可用） | — |
| close | 进程优雅关闭 / kill | — |
| subscribe_events | 被动接收通知（§3） | — |

- `turn/start` 参数含 `approvalPolicy` / `sandboxPolicy`：**每回合可覆写**，阶段 1 固定默认值，不暴露给用户配置。
- `thread/start` 的 `ephemeral` 参数可控制会话持久化；阶段 1 取默认（持久），native resume 才有意义。

## 3. 通知 → RuntimeEvent 映射表

阶段 1 映射下列已知通知。生成器中的通知集合会随版本增长，不把固定总数写成合同；未列通知按事件 schema §3.7 先判断是否可安全作为诊断通知保留，不能一律降级为 R2：

| codex 通知 | → RuntimeEvent kind | rate |
|---|---|---|
| `thread/started` | `attachment.started`（部分字段） | R0 |
| `turn/started` | `turn.started` | R0 |
| `turn/completed` | `turn.ended`(end_reason 映射见 §4) | R0 |
| `item/agentMessage/delta` | `text.delta` | R1 |
| `item/reasoning/textDelta`、`item/reasoning/summaryTextDelta` | `thinking.delta` | R1 |
| `item/plan/delta` | `plan.delta` | R1 |
| `item/started` | `tool.call.started`（按 item.type 分派） | R0/R1 |
| `item/completed`(agentMessage) | `message.completed` | R1 |
| `item/completed`(commandExecution/mcpToolCall/dynamicToolCall) | `tool.call.completed` | R1 |
| `item/commandExecution/outputDelta` | `tool.call.output_delta` | R1 |
| `item/fileChange/patchUpdated` | `file.change.completed` | R1 |
| `thread/tokenUsage/updated` | `usage.reported` | R1 |
| `error` | `error.raised`（fatal 判定见 §4） | R0 |
| `warning` / `configWarning` / `deprecationNotice` | `log.record` | R2 |
| `thread/compacted` | `log.record`（阶段 1 不展开） | R2 |
| `item/reasoning/summaryPartAdded` | 忽略（信息冗余于 delta） | R3 |
| `account/updated` | 内部 auth 状态更新，不外发 | — |
| 其他确认无需应答且不涉及控制/生命周期的通知 | `vendor`，保留原 method；语义未知时保守归 R1，版本化 allowlist 已确认是诊断/遥测时才归 R2/R3 | 依据通知语义 |

对阶段 1 未展开的 ThreadItem，只有确认不要求应答且不会改变审批、权限或生命周期语义时才进入 `vendor`。新增 item 类型必须通过版本捕获审查；不能依赖旧文档中的固定类型数量推断安全性。

## 4. 审批与输入请求（server→client request）

10 个 ServerRequest 中，阶段 1 必须处理：

| codex 请求 | → RuntimeEvent | 应答 |
|---|---|---|
| `item/commandExecution/requestApproval` | `approval.requested`(tool="command") | `{decision: "allow"\|"deny", ...}`（以捕获结果为准） |
| `item/fileChange/requestApproval` | `approval.requested`(tool="file_change") | 同上 |
| `item/permissions/requestApproval` | `approval.requested`(tool="permissions") | 同上 |
| `item/tool/requestUserInput` | `input.requested` | 按捕获 schema 应答 |
| `account/chatgptAuthTokens/refresh` | 不外发；Adapter 内部应答（token 刷新是 codex 自身职责） | — |

- `execCommandApproval` / `applyPatchApproval`（旧式）与 `item/*/requestApproval`（新式）并存：**捕获任务必须确认 0.139.0 实际走哪条**，Adapter 两套都实现，以运行时收到的为准。
- 审批超时策略：Pinvou 不代答，超时=拒绝（保守），在 `approval.requested.timeout_ms` 中透传 codex 的期限。
- 未知 server→client request 不得返回虚假成功或按 R2 忽略；Adapter 返回 `unsupported_control_event` 并停止当前 Turn，原始 method 进入诊断。

## 5. 错误归一化表

| codex 侧表现 | → AdapterError | → 对外 |
|---|---|---|
| JSON-RPC error code（如 thread 不存在） | `ProtocolError{code, method}` | `error.raised`（R0）；resume 失败→降级新建线程的策略由 Controller 决定（蓝图 §12.2） |
| 进程非零退出 | `ProcessExit{code}` | `attachment.ended(failed)` |
| `account/read` 显示未登录 / auth 通知 | `AuthExpired` | runtime 进入 `blocked_auth`（蓝图 §11），CLI 退出码 4 |
| stdin/stdout 管道断裂 | `ProcessExit`（区分 SIGPIPE/EOF） | 同上 |
| 5s 内 `initialize` 无响应 | `HandshakeTimeout` | `error.raised`，attachment failed |
| 方法返回 `-32601 Method not found` | `Unsupported{method}` | 能力降级（如 `steering=false`），**不视为 fatal**（蓝图 §10.2"不支持显式返回 unsupported"） |

即使 `main` 流已达到 high-water 或 Controller 暂停消费，Adapter 仍持续解析 stdout 并把 R0 写入 `control`；无法再耐久保存 R0 时必须产生 `stream.aborted(event_spool_exhausted)` 并终止 Runtime，不允许无界内存缓存或静默丢审批。
| `turn/completed.status=failed` 或 `error.codexErrorInfo=usageLimitExceeded` | `RuntimeFailed` / `QuotaExceeded` | Turn/Attachment 明确失败；测试运行无效，不得只因收到 `turn/completed` 判为成功 |

## 6. 协议捕获任务（M0/T1 的第一个任务，~1 人日，含 harness、fixture 整理与 S2 接入）

在实现 Adapter 前先构建 capture harness（一个最小 JSON-RPC driver，把全双工流量按行记录为 JSONL，含 HostMonotonicClock 时间戳）。

**已完成（2026-08-19 零成本探针，§1.1）**：帧格式（行分隔）、响应无 `jsonrpc` 字段、stderr 分离、initialize/account/read/model/list/thread/start/thread/list 方法行为、MCP startup 通知噪声、响应与通知重复。

**剩余（消耗配额的 turn 场景，属延迟 spike S2 一并执行）**：

2026-08-19 首次尝试的四个 turn 均受到 `usageLimitExceeded` 影响，A/B/C 没有 R1，审批也未触发；原始 JSONL 仅作为失败 fixture，不关闭下列任务。有效性规则见 latency spike §3.0.1。

1. ~~帧格式确认~~ ✅ 行分隔。
2. 场景捕获：新线程单 turn；`thread/resume` 续会话；带工具调用 + 审批的文件任务；`turn/interrupt` 打断；未登录状态下的 `account/read`。
3. 产出：本文档 §2/§3/§4 的"待捕获确认"标注全部关闭；捕获 JSONL 作为合同测试 fixture。
4. `execCommandApproval`/`applyPatchApproval` 旧式与 `item/*/requestApproval` 新式审批的实际走向（§4）。
5. `turn/start` 的 input 具体形态（字符串 vs 结构化 items）与 `turn/steer` 实际行为。

## 7. 版本兼容策略

- 支持范围：**CI 钉住版本（当前 0.139.0）为基准**；`probe()` 读取版本后按兼容矩阵放行。矩阵初值：`>= 0.139.0, < 0.150.0`（次版本漂移观察期），每个新钉住版本须重跑捕获与合同测试。
- Schema 的 `v1/`、`v2/` 目录说明协议自身在演进：Adapter 以捕获事实 + `initialize` 协商结果为准，**不按版本号猜测能力**（蓝图 §21）。
- 版本不满足：`probe` 返回明确 `unsupported` + 诊断（版本、期望范围），CLI 提示用户升级/降级，不静默尝试。
- 破坏性变更应对路径：app-server 不稳定时切 ACP 备选（决策冻结 D-07），切换是 Adapter 内部实现替换，不影响 runtime-api 与事件 schema。

## 8. 能力映射（RuntimeCapabilities 初值）

```text
interactive_chat = true
native_resume    = true        # thread/resume
tool_approval    = true        # item/*/requestApproval
steering         = true        # turn/steer（捕获确认前先报 false）
history_import   = v2 待确认   # thread/inject_items
elicitation      = 阶段 1 false（mcpServer/elicitation/request 不启用 MCP）
image_input      = 阶段 1 false
session_modes    = [interactive]
config_options   = [model, effort]
auth_flows       = [ExistingCredential, BrowserUrl, LocalInteractive]  # account/login/start 形态以捕获为准
```
