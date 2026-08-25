# 既有资产盘点与 AGENTS.md 修约建议

> 状态：草案（draft）。日期：2026-08-19。
> 目的：蓝图 §20 允许"借鉴已验证语义，不反向依赖实现"；本文档把"可借鉴什么、以什么形态借鉴"落到具体资产，并给出 AGENTS.md 的修约提案。
> 事实来源：`pinvou3-app/src-tauri/src/features/` 与 `CodeWhale/` 实读（2026-08-19）。

## 1. 盘点结果

### 1.1 CodeWhale submodule（修正一个关键认知）

- CodeWhale 是 **DeepSeek CLI 系 fork**（workspace 0.9.5，crate 全部改名 `codewhale-*`；pinvou3-app sessions 直接使用 `deepseek_tui::SessionManager`），**不是 OpenAI Codex fork**。
- 其 `crates/app-server` 是 CodeWhale 自有协议：`thread/message`、`thread/resume`、SSE `RuntimeEventEnvelope{seq,event,thread_id,turn_id,item_id,payload}`、`item.started|delta|completed`、`approval.required|decided`。**与 OpenAI codex 的 `codex app-server` 是两套无关协议**——任何文档/代码不得混用这两个"app-server"概念。
- 对新 CLI 的直接价值：CodeWhale 已经是可独立安装、独立升级的 CLI，其 headless/机器接口可以作为未来 `agent-adapter-codewhale` 的协议事实输入。新 Adapter 必须通过子进程和版本化 wire contract 接入，与 Codex、Claude Code、CodeBuddy 等外部 CLI 地位相同；**不得依赖 `codewhale-protocol` 或任何 `codewhale-*` crate，不开白名单例外，也不要求修改 CodeWhale fork**。现有接口不能满足的能力显式标记 `unsupported`，等待 CodeWhale 自身独立发布通用机器接口后再适配。

### 1.2 pinvou3-app codex_acp（~15k 行，直接经验输入）

- 控制**真实 Codex CLI，走 ACP**（spawn npm 适配器 `@agentclientprotocol/codex-acp` v1.1.5，stdio JSON-RPC，Rust 侧 `agent_client_protocol` crate）。
- 可迁移的**经验**（非代码）：
  - 登录态：OAuth URL 白名单解析（auth.openai.com）+ 设备码提取 + 15s 轮询探测（`login.rs`）→ 新 Adapter 的 `auth_status`/`start_auth` 实现参考；
  - 审批：pending map + oneshot 应答、取消即 Cancelled（`mod.rs`）→ 新 Adapter 审批应答的并发模型参考；
  - 恢复：能力探测后 `LoadSessionRequest`，期间**抑制 replay 事件**、失败改建新会话；孤儿 turn 收口（`events.rs`）→ 新 Adapter `resume` 的事件边界处理参考；
  - `~/.codex/config.toml` 受管 `pv-*` 表写入（仅 `env_key`、`wire_api="responses"` 硬约束）→ 阶段 1 **不沿用**（蓝图 §11：Pinvou 不代管第三方配置；新 Adapter 零写 codex 配置）。
- ACP 路线整体保留为 app-server 的降级备选（决策冻结 D-07）。

### 1.3 sessions 事件投影

- 桌面事件模型 `AcpEventEnvelope{version,sessionId,turnId,seq,timestamp,event{type,data}}`（`events.rs`）——与蓝图 §20"已验证语义"一致，已被事件 schema v1 继承并扩展（rate_class、schema_version、流游标）。
- 持久化：JSONL timeline 追加 + 原子 state 快照，**无 SQLite**。新 CLI 改用 SQLite（决策冻结 D-04）是**有意识的偏离**，理由已记录在 D-04；桌面经验中"追加写 + 原子快照 + 孤儿 turn 收口"三条实现约束转入新 projector 设计（实施规格 T10）。

### 1.4 multiagent

- 形态是 CodeWhale 底座内的会话级主动委派（ADR-0006），子代理转录只读投影于 `.codewhale/state/subagent-transcripts/`。与蓝图 CollaborativeRun（Controller 侧多 Node 编排）是**不同抽象层**，经验不可迁移。阶段 8 设计时不应引用本资产作为先例。

### 1.5 remote-control-relay / WebUI v2

- 方向与新 Remote Node 相反（手机浏览器 → 桌面权威；Relay 盲转发）。可借鉴：配对/信任 UX、Relay 鉴权与审计协议设计。不可复用代码（Relay 是 Node.js，且属于 Desktop 体系）。阶段 4 的 Node 配对与 WebAccess 的信任模型是**两套独立信任域**，文档与 UI 措辞必须区分。

### 1.6 pinvou-cli（GAIA benchmark，6 crate ~16.4k 行）

- 与新 distributed 子图同 workspace 共存方案见实施规格 §2；benchmark crate 零改动，`cli` crate 增独立 `distributed/` 模块。
- `agent-backend-api`（一次性 benchmark 后端抽象）与 `runtime-api`（长生命周期交互 Runtime）**保持分离**（蓝图 §6 已定），本文档确认现状无冲突。

## 2. 复用决策总表

| 资产 | 复用形态 | 禁止事项 |
|---|---|---|
| CodeWhale CLI 公开机器协议 | 未来 `agent-adapter-codewhale` 的黑盒/协议事实来源 | 链接任何 codewhale-* crate、读内部 Store、要求修改 CodeWhale 源码 |
| codex_acp 登录/审批/恢复经验 | 设计输入写进 Codex Adapter 合同 | 拷贝代码、复用 AcpPool |
| AcpEventEnvelope 语义 | 事件 schema v1 继承字段语义 | 共享存储/类型定义 |
| timeline JSONL 实现约束 | projector 实现约束（T10） | — |
| remote-control 信任 UX | 阶段 4 文案/流程参考 | 代码复用、信任域混用 |
| GAIA benchmark crate | 同 workspace 邻居 | 交叉依赖、互相改构建 |

## 3. AGENTS.md 修约提案（随实施规格 PR#1 提交）

### 3.1 "CodeWhale 与 fork 边界"表增补一行

```markdown
| 分布式 Runtime / Node 子图 | `pinvou-cli/crates/{controller,node,protocol,runtime-api,agent-adapter-*}`；禁止依赖 Tauri、`pinvou3-app`、任何 `codewhale-*` crate 与 `product-backend`；CodeWhale 仅可作为外部 CLI 由 Adapter 通过版本化机器协议接入，不设编译依赖例外 |
```

### 3.2 "项目事实"增补

```markdown
- `pinvou-cli/` 同时承载 GAIA benchmark 与 distributed 子图（controller/node/runtime）；两条路径构建与 CI 互相独立，不得交叉依赖。设计文档见 `docs/superpowers/specs/`。
```

### 3.3 修约时机

**不在本次提交**——AGENTS.md 修改属于仓库公约变更，应在阶段 1 立项（实施规格 PR#1）时随代码一并评审，避免公约先行于战略确认（当前战略前提见蓝图评审结论：本文档集整体仍处于"待用户批准"状态）。

## 4. 治理成本显式化（评审时不可回避）

蓝图新增的是独立的 Controller/Node 控制面，不再创建或嵌入一套 CodeWhale Engine。Desktop 继续维持现有嵌入式 CodeWhale 路径；distributed 子图只负责任务、连接、事件可靠性、资源和外部 CLI Adapter，CodeWhale 自己继续负责模型调用、工具循环、Session、Skills、MCP 与 Compaction。这样会产生两个产品进程栈，但依赖方向单向、主工程零改动，治理成本集中在版本化 Adapter 合同而不是 fork 编译耦合。

本路线的资产复用仅限协议事实、黑盒行为和独立文档经验；任何 `pinvou3-app/`、`CodeWhale/`、主工程 lockfile、打包清单或默认运行行为变更都不属于实施范围。
