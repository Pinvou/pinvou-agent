# Runtime Event Schema v1

> 状态：**草案（draft）**——Adapter、projector、合同测试以此为共同输入；变更需走事件合同评审。
> 日期：2026-08-19
> 上游：蓝图 §13.3（信封）、§13.4（数据路径）、§13.5（R0–R3 分级）。
> 事实输入：codex-cli 0.139.0 app-server 官方 JSON Schema（第一方生成，含顶层与 `v1/`、`v2/` 版本目录；不以易漂移的文件总数作为合同）；桌面端 `AcpEventEnvelope{version,sessionId,turnId,seq,timestamp,event{type,data}}` 投影经验（见资产盘点文档）。

## 1. 信封（Envelope）

```json
{
  "protocol_version": 1,
  "schema_version": 1,
  "node_id": "node_...",
  "logical_session_id": "sess_...",
  "attachment_id": "att_...",
  "stream_id": "main",
  "turn_id": "turn_...",
  "seq": 42,
  "source_span": { "start": 101, "end": 103 },
  "timestamp": "2026-08-19T09:00:00.123Z",
  "rate_class": "R1",
  "kind": "text.delta",
  "payload": { "...": "..." },
  "vendor_extension": { "optional": true }
}
```

- 字段与蓝图 §13.3 一致，新增 `schema_version`（独立于传输协议版本）与 `rate_class`（显式 R0–R3，投影端不自行猜测）。
- `work_id` / `collaborative_run_id`：阶段 1 恒为 null，字段保留（阶段 4+ 委派任务使用）。
- `stream_id`：阶段 1 即冻结至少两个逻辑传输流：`"control"` 只承载 R0，`"main"` 承载 R1–R3。每个流独立分配传输 `seq`、累计 ACK、重放游标和 `source_span` 映射；ACK 游标 = `(attachment_id, stream_id)` 上最后**连续**的传输 `seq`。这不是性能优化，而是避免已编号 R1 队列让后到 R0 产生逻辑队头阻塞。
- `source_span`：可选，表示该传输事件由 Node spool 中哪一段连续 `source_seq` 合并/降级而来。它只用于 Node 回收 spool，不参与 Controller ACK 连续性判断。

## 2. 原始 spool、传输序号与合并规则

- Adapter 每读到一个 R0/R1 原始事件，Node 先按 rate class 路由到 `control` 或 `main`，再在 `(attachment_id, stream_id)` 内分配连续 `source_seq` 并追加对应 spool。R2/R3 可先按截断/latest-wins 策略决定是否接受；被接受的记录才分配 `source_seq`，R2 丢弃另写带时间窗/来源计数的 `diagnostic.gap`，R3 丢弃无需制造序号缺口。达到 durable barrier 的原始记录才可进入合并/传输阶段。
- `seq` 是合并、截断和 latest-wins 决策完成后分配的传输序号，在 `(node_id, attachment_id, stream_id)` 内严格单调**连续**。传输跳号才触发补拉；被合并或丢弃的 `source_seq` 不制造传输缺口。
- **可合并对**：同一 turn 内相邻的同 `kind` 的 `text.delta` / `thinking.delta`（拼接 `content`），合并后事件带 `merged_count` 与覆盖原始记录的 `source_span`。
- **永不可合并**：一切 R0；`tool.call.*`、`message.completed`、`usage.reported` 等结构化 R1。合并发生在 Node 原始记录达到 durable barrier 后，不跨 turn、不跨 rate_class。
- Controller 分别对 `control`、`main` 的传输 `seq` 做累计 ACK；Node 通过每流持久的 `seq -> source_span` 映射推进对应 raw spool 回收水位。映射未持久化时不得发送对应事件。`BatchAck` 可以在一条消息中携带两个流的水位，但不得把它们合并成单一连续序列。
- 若 Node 在 raw spool barrier 前崩溃，恢复时根据 active Turn/Attachment journal 产生 R0 `stream.gap(reason="uncommitted_tail")` 并把运行标记为不完整；“零静默丢失”不等于声称 group commit 前的内存尾部能在掉电后恢复。

## 3. kind 分类法

### 3.1 生命周期（R0，不合并不丢弃）

| kind | payload 必填字段 | 说明 |
|---|---|---|
| `attachment.started` | runtime_id, agent_kind, capabilities_snapshot | Attachment 进入 active |
| `attachment.ended` | end_reason: completed\|failed\|fenced\|interrupted, detail? | 终态，含错误摘要 |
| `turn.started` | user_input_ref | 每回合开始 |
| `turn.ended` | end_reason: completed\|interrupted\|error\|cancelled, error? | 回合终态 |

### 3.2 交互请求（R0）

| kind | payload | 说明 |
|---|---|---|
| `approval.requested` | approval_id, tool, summary, options[], timeout_ms? | 工具/命令/文件变更审批 |
| `approval.resolved` | approval_id, outcome: approved\|denied\|cancelled | 回写审批结果（含用户应答） |
| `input.requested` | input_id, prompt, schema? | Agent 主动索要补充输入 |
| `input.resolved` | input_id, value | 用户输入回写 |

### 3.3 错误与资源（R0）

| kind | payload | 说明 |
|---|---|---|
| `error.raised` | code, message, fatal: bool, source: adapter\|runtime\|node | `fatal=true` 时 attachment 随之进入 failed |
| `resource.ref_created` | ref（完整 ResourceRef 结构，蓝图 §17） | 产物引用登记 |
| `stream.aborted` | reason（如 `event_spool_exhausted`） | spool 紧急终止事件（蓝图 §13.5 第 3 条） |
| `stream.gap` | reason, affected_rate_classes[], known_source_span? | 可靠会话语义可能不完整；必须显式投影并使 Attachment degraded/failed |

### 3.4 内容（R1）

| kind | payload | codex 来源（映射参考） |
|---|---|---|
| `text.delta` | role: assistant, content, merged_count? | `item/agentMessage/delta` |
| `thinking.delta` | content, merged_count? | `item/reasoning/textDelta`、`item/reasoning/summaryTextDelta` |
| `plan.delta` | content, merged_count? | `item/plan/delta` |
| `message.completed` | role, content, item_id | `item/completed`(agentMessage) |
| `tool.call.started` | tool_id, name, args_json? | `item/started`(commandExecution/mcpToolCall/...) |
| `tool.call.args_delta` | tool_id, args_delta | 动态工具参数流 |
| `tool.call.output_delta` | tool_id, chunk | `item/commandExecution/outputDelta` |
| `tool.call.completed` | tool_id, result, is_error, exit_code? | `item/completed`(commandExecution 等) |
| `file.change.completed` | tool_id, patch, paths[] | `item/fileChange/patchUpdated` + `item/completed`(fileChange) |
| `usage.reported` | input_tokens, output_tokens, cached_tokens?, model? | `thread/tokenUsage/updated` |

#### 3.4.1 统一工具调用平面约束（后续阶段）

`tool.call.*` 与 `file.change.completed` 是跨 Runtime 恢复/切换的公共投影，不等同于某个厂商的原生
tool event。后续 Tool Registry 落地前，Adapter 至少必须保留以下不变量：

- `tool_id` 在同一 `(node_id, attachment_id, turn_id)` 内稳定，不能因 Runtime 私有 callback id
  变化而覆盖未完成工具调用；
- 工具事件必须能表达执行状态：started → args/output delta → completed；缺失或乱序时产生
  `error.raised` 或 `stream.gap`，不得伪造成功完成；
- `tool.call.completed.result` 只表示已完成调用的结果引用或摘要；恢复会话时不得重新执行已完成工具；
- 非幂等工具在 started 后、completed 前中断时必须标记 `uncertain`（可放入 payload 或
  vendor_extension，待 schema v2 固化字段），并要求恢复核验；
- Adapter 必须区分工具控制能力：
  - `enforced`：Control Plane 能在执行前审批/拦截；
  - `observed`：只能观测 Runtime 原生工具，不能完全阻止；
  - `unknown`：能力不明，不能作为可安全切换依据。

这些约束用于保证 Runtime 切换时的 Tool Set 指纹可比较：目标 Runtime 缺失关键 Portable Tool 或
从 `enforced` 降级到 `observed/unknown` 时，应阻止切换或要求用户显式接受降级。

### 3.5 诊断（R2，可截断，截断必须落 `diagnostic.gap`）

| kind | payload |
|---|---|
| `log.record` | source, level, message（截断时 `truncated: true, original_len`） |
| `diagnostic.gap` | reason, source_span 或时间窗（不得表示传输 seq 缺口） |

### 3.6 遥测（R3，latest-wins / 降采样允许）

| kind | payload |
|---|---|
| `progress.tick` | turn_id, phase, percent? |
| `resource.sample` | cpu, memory, sampled_at（Node 附带采样） |

### 3.7 未知事件与未知请求

- 已协商 schema 版本内、确认不要求应答且不涉及审批、权限、输入或生命周期的未知厂商通知可以映射为 `kind = "vendor"`，原始内容进 `vendor_extension`。上游厂商通知不携带 Pinvou `rate_class` 时，由 Adapter 保守赋类：只有经版本化 allowlist/协议语义确认是诊断或遥测时才分别使用 R2/R3；其余安全但语义未知的通知使用 R1，不能因“未知”自动降为可丢弃事件。
- 未知 server→client request、审批、权限、输入或生命周期事件不得降为 R2。Adapter 必须返回 `unsupported_control_event`、停止当前 Turn/Attachment 并保留原始 method，防止未来控制事件被静默跳过。
- 高于接收方能力的 `schema_version` 先依据协商结果拒绝或降级连接；不能把整个高版本事件一律重分类为可丢弃 `vendor/R2`。
- Adapter 遇到可安全保留的未知通知时，将 method 名写入 `vendor_extension.method`。

## 4. 黄金合同测试

1. **fixture 集**：每个 kind 至少 1 条真实形态样本（阶段 1 从 codex 协议捕获与桌面 ACP timeline 双来源取材），存 `protocol/tests/fixtures/events/*.json`。
2. **往返测试**：serde 反序列化 → 再序列化 → 逐字节等价（排除 timestamp）。
3. **序号连续性**：伪造传输 seq 跳号流，验证去重/补拉；合并 source span 不得被误判为传输缺口。
4. **合并测试**：3 条已 durable `text.delta` 合并为 1 条，`merged_count=3`、`source_span` 正确，内容拼接顺序稳定，ACK 后 raw spool 水位推进到 span 末端。
5. **未知通知**：安全的 `vendor` 通知过全链路（spool→WAL→projector）不丢不崩；验证未知安全通知默认 R1，allowlist 中的诊断/遥测通知分别保持 Adapter 赋予的 R2/R3。
6. **未知控制请求**：伪造未知 server request，验证 Adapter fail closed、产生明确错误且不继续 Turn。
7. **未提交尾部**：模拟 raw spool barrier 前崩溃，恢复后必须产生显式 gap/不完整终态。
8. **控制流无队头阻塞**：先让 `main` 达到 high-water 并制造未 ACK 积压，再注入 `approval.requested`/`turn.ended`；验证 `control` 使用独立 seq/ACK 发送且满足 R0 延迟门禁，`main` 的缺口不阻止 control ACK 推进。

## 5. 版本演进规则

- `schema_version` 只增不改。v1 → vN 允许：新增 kind；为已有 kind **新增可选字段**。禁止：删除 kind、修改已有字段语义、把可选改必填。
- 投影器按 `schema_version` 分派解析路径；握手只接受双方明确支持的 schema/feature 组合。单个未知通知按 §3.7 处理，未知控制事件 fail closed，不允许用“连接不崩溃”掩盖安全语义缺失。
- 每次演进必须同步更新 fixture 与映射表（Codex Adapter 合同文档 §3），并在 PR 中附"新增事件对旧投影的影响说明"。
- 每次演进必须运行 N-1 writer → N reader、N writer → N-1 reader golden；验证未知字段的二进制往返，并明确 JSON/vendor extension 是否保留未知字段。event kind 或字段发生不兼容变化时必须新增 type/version，禁止复用旧字段编号或旧语义。

## 6. 与桌面 ACP timeline 的关系

桌面 `AcpEventEnvelope` 的 `sessionId/turnId/seq/type/data` 语义被本 schema 继承（蓝图 §20 允许借鉴），但字段命名与分类法独立演进；两者不共享代码与存储（蓝图 §6.5）。桌面 timeline JSONL 的实战经验（追加写 + 原子快照、孤儿 turn 收口）转化为本 schema 的投影器实现约束，见实施规格文档任务 10。
