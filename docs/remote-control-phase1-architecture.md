# 完整 WebUI v2 架构

> 本文保留旧文件名，内容描述当前 WebUI v2。v2 直接替代旧的 Session 级手机
> Remote 页面和协议，不提供兼容层。

## 1. 目标与边界

WebUI 与 Tauri 桌面端使用同一套 React 界面源码，电脑浏览器和手机浏览器都打开这套
完整界面。桌面端仍是唯一执行与数据权威：Session、EnginePool、模型、工具、知识库、
附件路径和产物都留在桌面主机；Relay 不保存这些业务数据，也不执行应用命令。

WebUI 当前刻意隐藏以下仅适合桌面环境的入口：

- OAuth、扫码、SSO、外部取 key 等授权入口；
- 超级权限、应用更新/重启、依赖及本地模型组件安装；
- 桌面系统窗口、打开本地目录、标题栏、窗口分离和桌宠；
- Web 访问链接自身的启用、停用和刷新入口。

已授权连接器的状态查询和使用、工具/技能商店只读浏览、对话、计划、工作流、知识库、
记忆、定时任务、监控、每 Session 模型选择及 Session 产物仍属于 WebUI 业务面。全局模型
增删改/测试、连接器开关及工具/技能安装卸载当前在 Web 隐藏，避免显示无法执行的入口。

## 2. 组件关系

```text
同一套 React UI
├─ Tauri 桌面入口 ── Tauri invoke/event ── Rust + EnginePool（权威）
└─ 浏览器入口 ───── WebBridge RPC/event ─┐
                                         │ WebSocket v2
桌面 WebAccessManager ────────────────────┤
                                         ▼
                                  Relay（鉴权与盲转发）
```

代码位置：

- `pinvou3-app/src/`：共享 React UI、平台能力门控、浏览器 bridge；
- `pinvou3-app/src/web-access-policy.json`：Web 可调用命令和可订阅事件白名单；
- `pinvou3-app/src-tauri/src/remote_control/`：持久 endpoint、RPC 去重、事件序列与重放；
- `remote-control-relay/server.js`：静态站点和 WebSocket v2 Relay；
- `remote-control-relay/PROTOCOL.md`：线上消息格式的单一协议说明。

## 3. 构建与页面更新

桌面和 Web 使用相同源码，但分别产出：

```bash
npm --prefix pinvou3-app run build:ui   # 桌面 UI
npm --prefix pinvou3-app run build:web  # remote-control-relay/web/dist
```

Web 构建以 `/pinvou3/remote/` 为默认 base path。Relay 对 HTML 禁止缓存，对固定名脚本
要求重新验证，对带 hash 的静态资源使用 immutable 缓存，因此替换 Web dist 后浏览器可在
下次加载获得新版本，而不受已安装桌面版本的 UI 资源约束。

这里仅说明构建能力。**正式发布流程目前尚未启用 Web dist 的打包或部署**；是否把该产物
接入现有发布脚本，需要单独评审后再做，不能把本地 `build:web` 视为已上线。

## 4. 持久访问链接

桌面首次启用 Web 访问时生成：

- `endpoint_id`：桌面实例的持久标识；
- `access_token`：浏览器访问凭据；
- `desktop_secret`：桌面向 Relay 注册和撤销 endpoint 的凭据。

配置保存在 `~/.pinvou3/web-access.json`，RPC 幂等台账和待撤销 endpoint 台账保存在同目录的
私有文件中；这些文件都以临时文件写入、同步后原子替换，Unix 下权限固定为 `0600`。进程级
独占锁保证同一份持久配置只由一个桌面进程拥有。启用后桌面重启会自动恢复连接；桌面临时
离线时 endpoint 仍可保留，浏览器显示等待状态。停用或刷新先持久记录撤销意图，再删除/替换
当前配置；待撤销台账有界且只保存 `relay_url`、`endpoint_id` 和 `desktop_secret`，不保存浏览器
`access_token`。与当前配置仍完全匹配的记录视为“已准备但尚未提交”，不会误撤销仍在使用的
链接；配置删除或替换后，独立于 active endpoint 的 worker 持续重试。Relay 离线或应用在 ACK
前退出时，下次启动继续重放；只有收到 `desktop_endpoint_revoked`、明确的
`endpoint_not_found` 或 endpoint 已被替换的终态后才清除记录。

用户粘贴的链接形如：

```text
https://host/pinvou3/remote/#endpoint=...&token=...
```

凭据只放在 URL fragment 中，fragment 不会进入 HTTP 请求、访问日志或静态资源 URL。
浏览器加载后才把它用于 WebSocket 首帧鉴权。

## 5. Relay v2

Relay 只维护 endpoint 的连接和凭据 hash：

1. 桌面发送 `desktop_endpoint_register`；
2. 浏览器发送 `web_client_join`，同时带 `stream_epoch` 和 `after_seq` 游标；
3. Relay 分配 `lease_id`，并盲转发白名单消息类型；
4. 同一 endpoint 同时只保留一个 Web lease，新客户端接管后旧客户端收到
   `endpoint_replaced`；
5. 桌面发送 `desktop_endpoint_revoke` 后，浏览器收到 `endpoint_revoked`。

浏览器到桌面只允许：

- `rpc_request`
- `event_subscribe`
- `event_unsubscribe`
- `client_ready`

桌面到浏览器只允许：

- `rpc_response`
- `event`
- `stream_reset`
- `desktop_snapshot`

Relay 不分配业务事件序号、不维护 Session snapshot、不解释 RPC command，也不保留用户消息
或附件。访问 token 和桌面 secret 只以 SHA-256 hash 留在 Relay 内存中。

## 6. WebBridge 与权限

浏览器先同步加载 `web-bootstrap.js`，再加载原有 `tauri-bridge.js`。WebBootstrap 提供受限的
Tauri 形状接口，让共享 UI 继续通过 `invoke/listen` 工作；真正的调用会成为 v2 RPC。

权限在两端收口：

- 浏览器只发送 `web-access-policy.json` 允许的 command/event；
- Rust WebAccessManager 再次校验同一白名单；
- 桌面 WebView 代理只执行 Rust 已接受的业务命令并回传结果；
- Relay 只按协议消息类型转发，不获得任意 Tauri invoke 能力。

每次 `desktop_snapshot` 都携带已安装桌面后端的协议版本、command 和 event 能力。中央 WebUI
把线上 policy 与这份能力取交集：未知能力默认关闭，语义入口（主机文件选择、产物下载、
浏览器语音、Session 模型切换等）由实际 command 集合派生，因此较旧桌面缺少新命令时会
降级或隐藏，而不是点击后才失败。桌面主 WebView 在宣布 bridge ready 前预装全部允许事件的
转发器，关闭订阅握手中的丢事件窗口；分离窗口无权代理 RPC。

浏览器导航、当前选中的 Session、草稿、主题和语言属于每个客户端自己的 UI 状态；底层
Session 数据和执行状态由桌面共享。同一 Session 同时只接受一个 Engine 回合：后端在提交
边界拒绝第二个并发回合。`TurnReservation` 在解析 Web 附件 handle、消费一次性 skill/persona
注入及落暂存文件之前完成原子准入；任何提交前错误都会释放回合和恢复一次性状态。提交成功
后依次广播带 `base_transcript_revision` 的 `chat:user_message` 与 `chat:turn_started`，另一 UI
在首 token 前即可显示用户气泡并进入忙碌态。

Engine 的 `SessionUpdated` 是 transcript 唯一权威来源：Rust 把仅供模型使用的附件正文、skill /
persona 注入替换为 UI display message 后原子落盘，发送 `chat:transcript_committed`，并保证最终
commit 先于 `chat:done`。发起页面即使刷新、关闭或被另一浏览器接管，回合仍会完整保存；桌面
和 WebUI 都不再用各自的内存消息覆盖权威 transcript。WebUI 不重新实现 Engine、Session 或
流式处理。

## 7. 幂等、流与恢复

每个 RPC 都带稳定的 `client_request_id`。桌面在派发前把 command 与规范化 args 的指纹写入
endpoint 级、最多 512 条的持久台账；同 ID 同参数复用结果或 tombstone，同 ID 不同参数直接
返回冲突。桌面 WebView 每次加载会生成新的 generation，请求只有在 `rpc_begin` ACK 已持久化
后才允许调用业务命令。WebView 重载时，尚未 ACK 的请求可交给新 generation 重发；已经 ACK
但没有完成结果的请求返回 `outcome_unknown`，不会盲目重复执行。这样浏览器和桌面断线重试
都不会把有副作用的动作静默执行两次。

RPC command 限制为短 ASCII 标识符，普通参数上限 1 MiB、普通响应上限 2 MiB、并发执行上限
32；错误文本、完成缓存和持久 tombstone 都有界。Session、产物和音频使用专用分块/紧凑
包装器，不靠放宽普通 RPC。未 ACK 的持久 tombstone 可安全恢复派发；ACK 台账无法写入时
请求在执行业务命令前形成确定错误响应。

Relay client 入站只接受不超过 2 MiB 的文本帧，并以 32 槽原始 JSON 文本队列施加背压；
解析发生在出队之后，因此队列不会保存 32 份膨胀后的 `serde_json::Value`。桌面出站完整帧
上限为 `4 MiB - 64 KiB`，只序列化一次，并让 2048 槽发送 channel 与 2048 条断线 FIFO
共享 128 MiB 字节预算。断线 FIFO 满时停止抽取 channel，绝不淘汰中间帧；上游随后通过
显式 stream reset 恢复。Shutdown 不与数据排队竞争；饱和时的 reset 采用单 worker、只保留
最新一条的合并队列，额外内存最多两个 64 KiB 控制帧。

桌面为事件流维护：

- 随桌面进程生成的 `stream_epoch`；
- 单调递增的 `seq`；
- 最多 1024 个完整事件帧、合计最多 16 MiB 的内存 journal。

浏览器在 `sessionStorage` 保存 `{stream_epoch, after_seq}`。重连时桌面按游标重放；epoch
不一致或游标已超出 journal 时发送 `stream_reset`/`desktop_snapshot`，浏览器再重新拉取权威
状态。首次 `client_ready(state_ready=false)` 只协商桌面能力并建立游标；共享 UI 拉完 durable
Session 索引后再发送 `state_ready=true`，桌面才重放初始化窗口内的业务事件，避免事件尾巴先于
基线 hydration。Relay 本身不承担这份恢复状态。

完整 event envelope 超过帧上限、journal 游标过旧，或 snapshot/replay/live enqueue 失败时，
桌面都会原子换 epoch、停止当前 lease 的 live delivery 并发送小型 `stream_reset`。超限事件
本身不推进 seq、不入 journal；live enqueue 失败时最后一个关键事件迁入新 epoch，避免最终
`chat:done` 或 `chat:user_input_required` 被静默丢失。

## 8. 文件、产物与语音

- Web 附件选择浏览桌面主机允许范围内的文件，不上传浏览器本机文件；
- 粘贴图片和浏览器拖放本机路径不作为 Web 附件入口；
- 附件在桌面解析后只向浏览器返回有界元数据和一次性 opaque handle，解析后的正文不经过
  Relay；附件、Session 上传/下载缓存均有数量、总字节和过期边界；
- 长 Session 通过 opaque download id 分块读取。正常回合不向 Web 暴露 transcript 上传命令；
  revision 仅用于标识 Rust 已提交的权威快照及事件幂等，旧 UI 的无 revision 覆盖写会被拒绝；
- 只有当前 Session 允许范围内的产物可以分块读取和下载，Rust 负责路径校验；
- 麦克风采集发生在当前浏览器设备，音频通过 allowlisted RPC 交给桌面 ASR；缺少桌面 ASR
  组件时只提示去桌面安装，WebUI 不显示安装操作。

## 9. 验证

协议单测：

```bash
npm --prefix remote-control-relay test
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml remote_control:: -- --test-threads=1
```

共享 WebUI smoke：

```bash
npm --prefix pinvou3-app run test:webui
```

该 smoke 构建真实共享 WebUI，启动真实本地 Relay，用 WebSocket 模拟桌面 endpoint，并验证
1440×900 与 390×844 渲染、fragment 凭据隔离、单客户端接管、RPC 往返以及事件游标重连。
缺少 Chromium/Edge 时按统一 runner 约定以 exit 2 跳过。
