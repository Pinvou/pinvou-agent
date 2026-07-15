# Remote Control 一期架构设计

日期：2026-07-09

## 目标

一期只解决一个问题：手机扫码接入桌面端当前远控 room，能够选择或创建 session、远程查看会话、发送消息、处理基础确认，并与桌面端实时同步。

它不是底座远程控制，也不是飞书 / 微信 Bot Channel。一期的对象是 `session`，不是 `device`、`knowledge base` 或全局任务中心。

## 设计原则

- 本地执行权不外移：DeepSeek-TUI Engine、本地模型、文件系统和工具执行仍在本地 pinvou3 主机。
- 云端只做 relay：云端负责配对、鉴权、WebSocket 中继和连接状态，不默认保存用户消息全文、工具参数、文件内容。
- 不重复造底座：复用现有 `EnginePool`、`SessionStore`、`chat:*` Tauri 事件、`chat` / `submit_user_input` / `cancel_user_input` 命令。
- 一期只绑定当前远控 room：扫码链接不提供设备级能力；room 内可以列出、新建和切换 pinvou3 session，但不能切换桌面工作区、任意浏览文件或访问知识库。
- 断线可恢复：手机断线重连后，从本地重新取 session snapshot，而不是依赖云端缓存还原状态。

## 当前可复用基础

后端已有三块关键基础：

- `pinvou3-app/src-tauri/src/engine.rs`：每个 session 的 event forwarder 已把底座事件转成 `chat:delta`、`chat:tool_start`、`chat:tool_end`、`chat:user_input_required`、`chat:done` 等 Tauri 事件。
- `pinvou3-app/src-tauri/src/commands.rs`：已有 `chat`、`cancel_generation`、`submit_user_input`、`cancel_user_input` 等命令，且都支持 `session_id` 路由。
- `pinvou3-app/src-tauri/src/connector_cli.rs`：已有通用二维码生成函数 `make_qr(url)`，可直接用于 Remote Control 配对二维码。

因此一期新增层不应直接调用 DeepSeek-TUI Engine，而应挂在 pinvou3-app 的 Tauri bridge 外侧。

## 总体架构

```text
手机浏览器
  - remote session UI
  - WebSocket client
        |
        v
Pinvou Cloud Relay
  - pairing token
  - room routing
  - desktop/mobile ws 中继
  - 连接状态与审计元数据
        |
        v
pinvou3 desktop / local app
  - RemoteControlBridge
  - SessionSnapshotBuilder
  - chat:* event subscriber
  - command router
        |
        v
现有 pinvou3 runtime
  - EnginePool
  - SessionStore
  - DeepSeek-TUI Engine
  - 本地模型 / 工具 / 文件系统
```

一期需要新增三个模块：

1. 本地 `RemoteControlBridge`：运行在 `pinvou3-app` 内，负责生成配对、连接云端、把本地 session 事件转成 relay event，也把手机端指令转成本地 command。
2. 云端 `Pinvou Relay`：极薄 WebSocket relay，只维护 room、token、连接关系和有限审计。
3. 手机 Web Remote Control：由云端托管的移动端页面，渲染当前 session snapshot 和实时事件。

## 本地侧设计

### 新增模块

建议新增：

```text
pinvou3-app/src-tauri/src/remote_control/
  mod.rs
  manager.rs
  protocol.rs
  snapshot.rs
  relay_client.rs
```

职责：

- `manager.rs`：管理当前配对状态、active remote session、手机连接状态、停止远控。
- `protocol.rs`：定义 relay event 的 Rust 结构体和版本号。
- `snapshot.rs`：从 `SessionStore` 和前端可恢复数据中构造手机初始快照。
- `relay_client.rs`：维护本地到云端 relay 的 WebSocket 连接。

### Tauri commands

新增 commands：

```text
remote_control_start(session_id?: string) -> RemotePairingInfo
remote_control_stop(room_id: string) -> ()
remote_control_status() -> RemoteControlStatus
remote_control_refresh_qr(room_id: string) -> RemotePairingInfo
```

`remote_control_start` 行为：

1. 取 `session_id`，未传则使用 `SessionStore.active_id()`。
2. 创建 `room_id`、`pairing_token`、`desktop_secret`。
3. 本地连接云端 relay，并以 desktop 身份注册 room。
4. 生成手机访问 URL。
5. 使用 `connector_cli::make_qr(url)` 生成二维码 data URL。
6. 返回给桌面端弹窗展示。

返回结构：

```json
{
  "room_id": "rc_...",
  "session_id": "p3_...",
  "url": "https://remote.pinvou.ai/r/...",
  "qr_data_url": "data:image/svg+xml;base64,...",
  "status": "waiting_mobile"
}
```

### 本地事件转发

现有 engine forwarder 继续向 Tauri emit `chat:*`。一期新增的 `RemoteControlBridge` 订阅同一批事件，按 session 过滤后转发到 relay。

需要转发的事件：

```text
chat:delta
chat:tool_start
chat:tool_end
chat:plan_snapshot
chat:plan_ready
chat:user_input_required
chat:transient_error
chat:done
chat:usage
chat:compaction
artifact:disk
```

实时事件只转发 artifact card 摘要字段和路径尾部；手机可按需请求当前 session workspace/artifacts 内经过 canonical path 校验的受限预览，文件下载和任意文件浏览放到二期。

### 手机指令路由

手机端发来的 action 只允许以下几类：

```text
user_message
cancel_generation
submit_user_input
cancel_user_input
request_snapshot
disconnect
```

本地收到后映射到现有命令逻辑：

- `user_message` -> `EnginePool.send_user_message(session_id, content, mode)`
- `cancel_generation` -> `EnginePool.cancel(session_id)`
- `submit_user_input` -> `EnginePool.submit_user_input(session_id, tool_call_id, response)`
- `cancel_user_input` -> `EnginePool.cancel_user_input(session_id, tool_call_id)`
- `request_snapshot` -> `SessionSnapshotBuilder.build(session_id)`

注意：一期不建议让手机端经 Tauri command 字符串转发调用任意 command，必须做 allowlist，避免远端变成通用 RPC 后门。

## 云端 Relay 设计

### 职责

云端只做：

- 创建和校验配对 token。
- 维护 `room_id -> desktop_ws + mobile_ws`。
- 转发 desktop/mobile 双向事件。
- 记录最小审计：连接时间、断开时间、设备类型、错误码、事件计数。
- 托管手机 Web 页面。
- 公开健康检查只返回聚合状态，不暴露 `room_id`、`session_id` 或审计明细。

云端不做：

- 不持久化 session 全量消息。
- 不保存工具参数和工具输出全文。
- 不保存用户本地文件内容。
- 不直接访问用户本机端口。
- 不执行模型调用或工具调用。
- 不通过未鉴权接口暴露可用于接管房间的标识。

### Room 状态

```json
{
  "room_id": "rc_...",
  "session_id_hash": "...",
  "desktop_connected": true,
  "mobile_connected": false,
  "created_at": "...",
  "paired_at": null,
  "closed_at": null,
  "status": "waiting_mobile"
}
```

### 配对时序

```text
desktop -> relay: create_room(desktop_secret, session_id_hash)
relay -> desktop: room_created(room_id, pairing_url)
desktop: show QR
mobile -> relay: open pairing_url(pairing_token)
relay: validate token
relay -> desktop: mobile_join_requested
desktop -> relay: session_snapshot
relay -> mobile: session_snapshot
desktop <-> relay <-> mobile: live events
```

一期可以先默认扫码即连接，不加二次桌面确认；但桌面必须显示“手机已连接”，并提供停止按钮。更严格的“手机请求连接，桌面点允许”可作为安全增强项。

同一个 `room_id` 已存在时，desktop 重新注册只允许来自同一个桌面端：relay 必须校验 `desktop_secret`，校验失败时不得关闭旧 desktop、不得继承已连接 mobile，也不得向新连接透露房间状态。这个约束用于支持桌面端断线重连，同时挡住公网 relay 上的 room 抢占。

### Token 策略

- `pairing_token` 是当前 room 的能力凭证，与 room 同寿命，不使用固定 5-10 分钟 TTL。
- 持有二维码或完整链接即拥有连接权；同一 room 只保留一个 mobile，后来扫码或打开链接的设备接管控制，旧设备进入“已被占用”状态页。
- 手机刷新页面、重扫二维码或重新打开链接，均可使用同一个 `pairing_token` 恢复连接。
- “刷新二维码”会关闭旧 room、吊销旧 token 和旧手机连接，再创建全新的 room/token。
- “刷新二维码”、停止远控、旧 room/token 不可用统一进入手机端“远程连接已结束”页面；页面不猜测具体关闭原因，引导用户返回桌面确认并扫描当前二维码。
- 新二维码只在 URL fragment（`#token=...`）携带 token，避免进入 HTTP 和反向代理访问日志；手机页保留读取 query 的兼容逻辑，供旧 App 生成的链接继续使用；relay 只保存 token hash。

## 协议设计

所有 WebSocket 消息使用统一 envelope：

```json
{
  "v": 1,
  "id": "evt_...",
  "room_id": "rc_...",
  "session_id": "p3_...",
  "direction": "desktop_to_mobile",
  "type": "session_snapshot",
  "ts": "2026-07-09T10:00:00Z",
  "payload": {}
}
```

### Desktop -> Mobile

```text
session_snapshot
message_append
assistant_delta
tool_call_start
tool_call_end
plan_snapshot
plan_ready
user_input_required
session_status
usage_update
compaction_update
artifact_summary
error
mobile_connection_state
```

`session_snapshot` 最小结构：

```json
{
  "session": {
    "id": "p3_...",
    "title": "新对话",
    "mode": "yolo",
    "status": "idle"
  },
  "messages": [
    {
      "id": "m_...",
      "role": "user",
      "content": "你好",
      "created_at": "..."
    }
  ],
  "pending_user_inputs": [
    {
      "tool_call_id": "call_...",
      "questions": []
    }
  ],
  "running_tools": [],
  "artifacts": []
}
```

### Mobile -> Desktop

```text
user_message
cancel_generation
submit_user_input
cancel_user_input
request_snapshot
ping
disconnect
```

`user_message`：

```json
{
  "type": "user_message",
  "payload": {
    "content": "继续",
    "client_message_id": "cm_..."
  }
}
```

`submit_user_input`：

```json
{
  "type": "submit_user_input",
  "payload": {
    "tool_call_id": "call_...",
    "answers": [
      { "id": "q1", "label": "允许", "value": "allow" }
    ]
  }
}
```

## 手机 Web UI 一期范围

一期页面以单个 session 会话页为主，并提供轻量 session 选择面板：

- 顶部显示 session 标题、连接状态、桌面在线状态。
- 中间渲染消息流、工具执行状态、等待确认卡。
- 底部输入框支持发送文字和停止生成。
- `request_user_input` 渲染为确认卡，支持提交/取消。
- 断线时显示重连状态，重连成功后拉取 snapshot。
- 支持列出、新建和切换 pinvou3 session。
- 支持当前 session 受限 artifact 预览。

不做：

- 文件树。
- 知识库搜索。
- 设备列表。
- 设置页。
- 附件上传。
- 多手机同时控制。

## 桌面 UI 一期范围

桌面端新增“移动端远程控制”入口：

- 展示二维码、复制链接、刷新二维码、停止远控。
- 展示状态：等待手机、手机已连接、连接断开、已过期。
- 连接成功后在桌面明显提示“手机正在控制当前 session”。

建议入口先放在当前 session 顶部或侧栏工具区，不要放进全局设置。因为一期能力只绑定当前 session。

## 安全边界

一期必须实现：

- 高熵 room token，仅通过 HTTPS/WSS 传输，relay 只保存 hash。
- room 单 session 绑定。
- 已存在 room 的 desktop 重新注册必须校验 `desktop_secret`，防止未知桌面端抢占。
- 公开 `/healthz` 不暴露 room/session 明细，只返回聚合计数。
- mobile action allowlist。
- 桌面端停止远控后立即吊销 room。
- 云端不落全文消息和文件内容。
- 手机端不能调用任意 Tauri command。
- 手机端可以列出、新建、切换 pinvou3 session；扫码者应被视为临时拥有当前远控入口下的会话操作权。
- 手机端可以预览当前 session workspace/artifacts 内的受限文件；relay 不读取本地文件，预览内容由桌面端按 session 根目录校验后发送。
- 所有 mobile action 带 `client_message_id`，本地去重，避免断线重发造成重复消息。

一期暂不解决：

- 多设备长期绑定。
- 账号体系下的设备管理。
- 远程知识库搜索。
- 文件下载和任意文件浏览。
- Bot Channel 审批。

这些全部进入二期/三期。

## 可靠性策略

### 断线重连

- 手机 WebSocket 断开后自动重连。
- 手机重连继续使用与 room 同寿命的 `pairing_token`。
- relay 校验 room 未关闭且桌面仍在线。
- 手机发送 `request_snapshot`。
- 本地返回最新 `session_snapshot`，手机用 snapshot 覆盖本地临时状态。
- 桌面 WebSocket 意外断开时，relay 保留 room 和 mobile 15 秒；手机显示“桌面重连中”并暂停操作。同一 `desktop_secret` 在宽限期内恢复则继续原 room 并重拉 snapshot，超时才关闭 room。
- relay 每 15 秒发送 WebSocket ping；无 pong 的半开连接会被终止并进入对应重连流程。

### 消息去重

Mobile -> Desktop 的 action 必须带 `client_message_id`。本地 `RemoteControlBridge` 对每个 room 保留短期去重表，至少覆盖 10 分钟或最近 200 条 action。

### 背压

`assistant_delta` 可能高频。relay 可以不持久化 delta，但本地发送前应允许做 50-100ms 小窗口合并，减少手机端渲染压力。桌面本地 UI 仍走原始 `chat:delta`，不受影响。

### Relay 容量与入口保护

- Relay 默认最多保留 2000 个 room；已有 room 的桌面重连不受容量上限影响。
- 同一客户端默认每分钟最多新建 20 个 room；反向代理来源只在其 IP 被显式信任时读取 `X-Forwarded-For`，并采用代理追加的最后一个地址，避免客户端伪造首段地址绕过限流。
- Relay 默认最多同时保留 5000 条 WebSocket 连接，同一客户端默认每分钟最多发起 120 次 WebSocket 建连；达到容量时在 Upgrade 阶段拒绝新连接，避免未认证连接绕过 room 上限持续消耗资源。
- WebSocket 建立后必须在 10 秒内完成 `desktop_register` 或 `mobile_join`，否则 Relay 主动关闭；已认证连接不受该超时影响，仍按正常心跳和重连规则运行。
- 生产环境通过 `PINVOU_REMOTE_ALLOWED_PROXY_IPS` 只允许本机健康检查和指定反向代理访问 Relay，公网客户端统一走 `https/wss://pinvou.com`。
- WebSocket 单条消息默认上限为 4 MiB，可通过 `MAX_PAYLOAD_BYTES` 调整，服务端硬限制不超过 16 MiB。
- `/healthz` 额外返回 WebSocket 总连接数和未认证连接数，仍只提供聚合计数，不暴露 room、session 或客户端明细。

## 实施拆分

### M1：本地配对入口

- 新增 `remote_control` 模块骨架。
- 新增 `remote_control_start/stop/status/refresh_qr` commands。
- 复用 `connector_cli::make_qr` 生成二维码。
- 桌面 UI 弹窗展示二维码和状态。

### M2：relay 最小可用

- 实现 `create_room`、desktop ws、mobile ws。
- 实现 pairing URL 和 token 校验。
- 实现事件双向转发。
- 不落全文，只记录 room 状态和错误。

### M3：session snapshot

- 从 `SessionStore.load(session_id)` 构造消息快照。
- 增加运行态字段：busy、pending user input、running tools。
- 手机扫码后能看到当前 session 历史。

### M4：实时事件同步

- 本地桥订阅并过滤 `chat:*` 事件。
- 转发 delta、tool、done、user_input_required 等事件。
- 手机 UI 实时更新。

### M5：手机控制

- 手机发送 `user_message`。
- 本地路由到 `EnginePool.send_user_message`。
- 手机支持 `cancel_generation`。
- 手机支持 `submit_user_input` / `cancel_user_input`。

### M6：安全与验收

- token 过期、停止远控、断线重连。
- action allowlist 和去重。
- 云端日志脱敏。
- 桌面连接状态提示。

线上 relay 使用 `scripts/deploy-remote-relay.sh` 部署：脚本先确认现有公网基线正常，再运行自动化测试、备份、原子替换并重启 systemd。部署后会验证公网健康检查、新版手机页面标识和直连端口隔离；任何一项失败都会恢复本次部署前的备份，并再次执行本机和公网复检。

## 验收标准

一期完成必须满足：

1. 桌面端当前 session 可生成二维码。
2. 手机扫码打开 Web Remote Control。
3. 手机能看到当前 session 历史消息。
4. 桌面端继续生成时，手机能实时看到回复增量。
5. 手机发送消息后，本地当前 session 继续执行。
6. 手机能取消当前生成。
7. `request_user_input` 出现时，手机能提交选择并解锁本地 engine。
8. 手机断线重连后能恢复最新 session snapshot。
9. 二维码在 room 存续期间持续有效；刷新二维码后旧链接不可连接。
10. 停止远控后，手机端不能继续发送 action。

## 与二期的接口预留

一期协议不预留无实际语义的 `device_id` 字段；二期升级 device room 时再随协议版本新增。

一期 relay room 是 `session` 级；二期升级为：

```text
device room
  -> task/session channels
  -> approval channel
  -> knowledge search channel
```

一期 `session_snapshot` 不应塞入设备/知识库概念，避免提前污染边界。二期再新增：

```text
device_status
task_list
approval_list
kb_search_result
knowledge_card
source_card
```

## 关键决策

- 一期采用云端 relay，不做公网直连本地 web server。
- 一期采用手机 Web，不做原生 App。
- 一期只控制当前 session，不做设备级远程控制。
- 一期不修改 DeepSeek-TUI fork，不触碰底座 Engine。
- 一期不把云端变成业务状态源，业务状态以本地 `SessionStore` 和 runtime 为准。
