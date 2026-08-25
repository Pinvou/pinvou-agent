# CodeWhale CLI Adapter 解耦合同

> 状态：设计冻结（仅文档；不授权修改代码）。
> 日期：2026-08-19
> 上游：分布式 Node Runtime 蓝图 §6.5、§10、§20。
> 目标：让 CodeWhale 与 Codex、Claude Code、CodeBuddy 等一样，作为独立 Agent CLI 接入 Pinvou CLI；不修改主工程或 CodeWhale。

## 1. 硬边界

新路径固定为：

```text
pinvou CLI/TUI
  -> pinvou-controller
  -> pinvou-node / Runtime Host
  -> AgentRuntimeAdapter
  -> agent-adapter-codewhale
  -> 独立 codewhale 可执行文件
```

以下条件全部是发布门禁：

1. `agent-adapter-codewhale`、Controller、Node、protocol 和正式 `pinvou` binary 不得链接 `codewhale-*` crate、`pinvou3-app`、Tauri 或 `pinvou-product-backend`。
2. Adapter 不读取 CodeWhale 内部数据库、Rust 类型、私有模块或 submodule 源码路径；只依赖公开 CLI/wire 行为。
3. Adapter 不修改 `CodeWhale/`，也不要求增加 fork patch 才能完成 Pinvou 阶段验收。CodeWhale 新能力必须先在其自身版本中独立发布，再由 Adapter 通过能力协商使用。
4. 本路线不修改 `pinvou3-app/`、现有 Desktop 配置/数据、主工程 lockfile、打包清单、默认 feature 或运行行为。
5. CodeWhale 的认证、Provider、Skills、MCP、Session、Compaction、工具循环和工作区副作用仍由 CodeWhale 进程所有；Pinvou 只拥有 Logical Session、Attachment、调度、事件账本、ResourceRef 和 Node 连接。

## 2. 可用的公开入口与选择规则

当前 CodeWhale 自身文档公开了以下机器入口：

- `codewhale doctor --json`：离线健康、版本和能力探测；
- `codewhale app-server --stdio`：无监听的换行分隔 JSON-RPC 控制通道；
- `codewhale app-server --http`：loopback HTTP/SSE `/v1/*` Runtime API；
- `codewhale exec`：一次性 headless/stream-json worker；
- `codewhale serve --acp`：能力受限的 ACP 入口。

Adapter 选择顺序：

1. `probe` 先调用 `doctor --json` 和无模型调用的 `app-server --stdio` health/capabilities，记录 executable identity、version、commit、方法集和认证状态；探测成功不等于可完成交互任务。
2. 长生命周期交互优先选择能覆盖 create/resume/send/events/approval/input/interrupt/usage 的官方机器接口。若 `--stdio` 的已协商方法集完整，优先使用它以避免本地监听；否则使用只绑定 `127.0.0.1`、带每进程随机凭据的 HTTP/SSE Adapter。
3. `codewhale exec` 只用于明确的一次性 AgentTask，不伪装成支持 native resume 的交互 Runtime。
4. ACP 当前没有暴露完整工具、文件写入、checkpoint replay 和 session loading 时，不得为了协议统一而选择 ACP 并静默丢能力。
5. 禁止使用 `--mobile` 或 `0.0.0.0` 作为 Node 内部 Adapter 通道。

具体首选 transport 必须由黑盒协议 spike 和合同 fixture 决定，不能通过导入 CodeWhale Rust 类型消除解析工作。

## 3. Adapter interface 映射

| Pinvou interface | CodeWhale 公开行为 | 约束 |
|---|---|---|
| `probe/capabilities` | `doctor --json`、stdio `healthz/capabilities`、`/v1/runtime/info` | 版本和 feature bits 显式记录；未知 commit fail closed 或降级 |
| `auth_status/start_auth` | CodeWhale 自身 auth/account CLI | Pinvou 不接收或复制 Provider Token；需本地交互时返回引导 |
| `create/resume` | thread/session create、resume | native id 只保存为 Runtime Attachment，不成为 Logical Session 权威 ID |
| `send/subscribe_events` | stdio thread message 或 HTTP turn + replay/live SSE | 原生 seq 只是 source sequence；先归一化并落 Node spool |
| `approve/respond_input` | Runtime approval/input surface | 未知控制请求 fail closed，不降为日志 |
| `steer/interrupt` | 已协商方法或 endpoint | 不支持时返回 `unsupported`；不得用杀进程伪装成功 interrupt |
| `close` | shutdown/子进程监督 | 先 drain 终态和 spool，再回收进程；超时产生明确不确定结果 |

## 4. 进程、背压和安全

- Node 为每个活动 Attachment 监督 CodeWhale 子进程；Pinvou TUI/CLI 不直接启动 CodeWhale。
- stdout 同时出现 R0 控制和 R1 内容时必须持续 drain，R0 写入独立 `control` stream，R1 写入 `main`；禁止通过停止读取 stdout 背压。
- HTTP/SSE 模式仅绑定 loopback，认证材料只在父子进程受控通道中传递，不进入 argv、日志、事件或 Controller Store；禁止 Pinvou 连接 CodeWhale 的 mobile/LAN surface。
- CodeWhale 工作区必须受到 Node allowed roots、WorkspaceWriteGrant 和阶段 8 `WorkspaceIsolationProvider` 的外层约束。Grant epoch 只能保护 Pinvou 账本；CodeWhale 直接写普通文件系统时不存在硬 fencing，因此未知旧进程未终止前不得重授同一路径。
- Adapter 保存协议诊断但脱敏 Provider、Cookie、Authorization、路径中的秘密和用户私有内容。

## 5. 兼容与测试

每个受支持 CodeWhale 版本至少具有：

1. 黑盒 executable fixture：health/capabilities、create、resume、send、事件重放、审批、输入、interrupt、usage、shutdown；
2. 不同版本/commit 的 capability drift 测试，不按产品名硬编码能力；
3. stdout 高输出时注入审批/interrupt，证明 reader 持续 drain 且 `control` 无队头阻塞；
4. CodeWhale 崩溃、协议半帧、终态缺失、认证失效和 workspace 不可用的统一错误映射；
5. resolved Cargo dependency graph 断言不存在 `codewhale-*`、Tauri、`pinvou3-app`、`pinvou-product-backend`；
6. `pinvou3-app/`、`CodeWhale/`、主工程 lockfile/打包清单零 diff，并验证既有 Desktop/benchmark 合同不变。

## 6. 阶段位置

CodeWhale Adapter 不进入阶段 1 的 Codex 数据路径，也不要求修改阶段 2 已冻结的首批三 Adapter 验收。它作为后续 Adapter 批次加入；开工条件是本合同的黑盒 spike 证明至少一种公开机器入口覆盖所需能力。若不满足，只交付一次性 `exec` AgentTask 或报告 `unsupported`，不通过嵌入 CodeWhale Engine 绕过缺口。
