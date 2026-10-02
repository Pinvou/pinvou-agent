# session-reader 内置工具（会话创建）设计与验收

> 状态：**已实施**（2026-09-30）。配套契约：`docs/builtin-toolset-contract.md` §9 注册表行 `create_session`。
> 基线：`feat/scheduled-messages`（含 app-automations CRUD 与定时消息，PR #628/#629）。实现完全复用该族已验证的 "MCP 工具 + spool + 应用侧 watcher + 结果标记短等待" 骨架，**零 CodeWhale 变更**。
> 用法：实施后按 §5 验收矩阵逐项验收；矩阵编号与测试映射一一对应。

---

## 1. 背景与目标

会话模型此前只能向**已存在**的会话发消息（`send_message_to_session`），无法新建会话。目标：在**任意会话**里，模型可通过内置工具创建新会话（可选附带首条消息立即启动），补全"开个会话去做 X，做完回报"的委派链路。

## 2. 设计决策记录

| 决策点 | 结论 | 理由 |
|---|---|---|
| 归属 | session-reader 族（契约 §2 "一族一 server"） | 会话域写操作归会话族；`send_message_to_session` 是先例；app-automations 是调度自动化族 |
| 工具形态 | 单个 L1 工具 `create_session`，`first_message` 为可选参数 | 契约 §4.2 偏好 one tool + mode；无需拆 create / create_and_chat |
| 焦点语义 | **绝不 `set_active`** | 工具创建的会话静默出现在列表（`session:list_changed` 双端自动刷新），由用户自己点开；这是与 Tauri 命令默认值唯一刻意相反之处 |
| 继承语义 | 一律"继承应用默认，不继承调用方状态" | 模型 = 应用新会话默认（`default_model_for_new_session`），显式 `model_id` 覆盖（须为已保存模型，未知即拒）；工作区 = 应用默认（`bridge.workspace`），显式 `workspace_path` 绑定（绝对路径 + 已存在目录，canonicalize）；mode/persona/知识集取全新默认。复用 `create_session_record` 语义自然获得，零新语义 |
| 首条消息 | 作为新会话的**纯文本**开篇 user turn（`deliver_messaging_turn`），不带跨会话头块 | 它是开篇指令，不是转达消息；与 `web_access_create_session_and_chat` 同构 |
| 显式标题 | `create_new` 后 `set_title` | 自动命名只在标题仍为默认"新对话"时触发，显式标题天然不被首转覆盖 |
| 审批 | L1 typed Ask 逐次确认 | 用户批准的即是确切的 title / first_message / workspace；同一规则使无人值守会话拒绝递归创建 |
| 失败语义 | 与 app-automations 族相同的 at-least-once | 标记紧随 `create_new` 写入；仅标记写失败重试（该窗口内崩溃可重复建会话，已接受）；建后步骤（标题/绑定/首投）尽力而为，绝不因失败重试出第二个会话——唯一例外：工作区绑定失败回滚删除整个空会话（镜像 `create_session` 命令），使整体可重试 |

## 3. 组件与数据流

| 组件 | 位置 | 职责 |
|---|---|---|
| MCP server | `pinvou3-app/resources/mcp-servers/session-reader/server.py`（改） | 工具面：校验（caps/绝对路径/isdir 探测/隔离前缀）、原子写 spool `~/.pinvou3/session-requests/spool/<sha256(from\|create\|key)|uuid>.json`、结果标记短等待 ≤5s（命中返回 sessionId；超时回 `delivery:"pending"`） |
| manifest | 同目录 `manifest.json`（改） | `mcp_tools` + `tool_features` 注册 `mcp_session-reader_create_session` → 新 feature `session-creation`（默认开启）；版本 1.1.0 → 1.2.0 |
| 创建 watcher | `pinvou3-app/src-tauri/src/features/session_creation/mod.rs`（新增） | 1s 轮询排水、重校验（spool 用户可写，不信任 server 侧）、毒文件隔离 `spool/failed/`、≤3 次重试、终态 14 天保留修剪 |
| 领域入口 | `PoolCreator`（同文件） | 校验/解析（workspace canonicalize、model_id 对已保存模型解析）→ `SessionStore::create_new`（面板同一条管线）→ `set_title` → `bind_session_workspace`（失败回滚）→ `deliver_messaging_turn`（30s 上限，失败仅记审计）→ 结果标记 → 审计 → `session:list_changed` |
| 审批规则 | `features/assistant/platform/bridge.rs`（改） | `SESSION_CREATE_TOOL` 常量 + `scope_deny_ruleset_with` 注册 ToolAskRule |
| 挂载 | `lib.rs`（改） | messaging watcher 旁同款 app-lifetime spawn |
| 契约文档 | `docs/builtin-toolset-contract.md`（改） | §9 注册行 |

审计：成功 → 请求者执行根 `session_create`（tool/session_id/title/workspace_bound/model_id/first_message_delivered）；失败/隔离 → `session_create_failed`。事件：`session:list_changed {id, action:"created"}`（与 `create_session` 命令同 payload 形状；已在 web access-policy 白名单与双端监听内，前端零改动）。

## 4. 已披露事项与边界

- **默认 Yolo + 后台首转**：新会话与面板新建一样默认 Yolo；`first_message` 首转在用户未打开该会话时后台运行——L1 写工具的 Ask 请求浮现在新会话界面、首转暂停等批准。与 `send_message_to_session` 投递空闲会话的既有行为一致，非新增暴露面；创建时的确认卡已展示 first_message 与 workspace。
- 前端零改动：时间线走通用 MCP 工具卡兜底；专属结果卡与"跳转到新会话"留待后续（P2）。
- 无新 Tauri 命令、无新事件 → access-policy 与协议测试无变更；web 端行为与桌面一致。
- 老用户升级即自动获得（内置包按内容逐字节比对重释放，`ensure_package_released`）。
- 零 fork 变更，无需更新 `docs/fork-modifications.md`。

## 5. 验收矩阵

| # | 场景 | 预期结果 | 验证方式 | 优先级 |
|---|---|---|---|---|
| A1 | 端到端创建 | 模型调用工具，≤5s 返回 sessionId/title；会话列表出现新会话且**不抢焦点** | 手工 QA（真模型） | P0 |
| A2 | 短等待同步返回 | watcher 写 `.done` 后 server 命中标记返回 `{ok,sessionId,title}` | ○python `test_marker_hit_returns_session_ids` | P0 |
| A3 | 超时回 pending | 无 watcher 时返回 `{ok:true, sessionId:null, delivery:"pending"}`，非错误 | ○python `test_wait_timeout_returns_pending_not_error` | P0 |
| A4 | 首条消息投递 | 新会话首 turn 为纯文本 user 消息（无跨会话头块），引擎立即启动 | 手工 + 代码路径（deliver_messaging_turn） | P0 |
| A5 | 列表实时刷新 | `session:list_changed` 驱动 Tauri/Web 双端刷新 | 手工双端 | P1 |
| B1 | 参数上限 | title>200 / first_message>32k / model_id>200 / key>128 拒绝；边界值通过 | ○python `test_title_and_message_caps` 等 | P0 |
| B2 | workspace 校验 | 相对路径 / 不存在 / 超长拒绝；合法目录通过（双层：server isdir 探测 + watcher canonicalize） | ○python `test_workspace_must_be_absolute_existing_dir` + ◇Rust | P0 |
| B3 | 隔离前缀 | sched-/aux-/eval_ 作为 from_session 全层拒绝（server + watcher 重校验） | ○python + ○Rust `validation_rejects_bad_shapes` | P0 |
| B4 | model_id 解析 | 非已保存模型 id → 失败标记（错误回传模型）；合法 id → 会话模型 sidecar 绑定 | ◇Rust PoolCreator + 手工 | P1 |
| B5 | 标题语义 | 显式 title 落库且不被首转自动命名覆盖；无 title + first_message → 自动命名 | 手工 + ○语义（apply_default_session_title 只在默认标题时触发） | P1 |
| C1 | 幂等重试 | 同 key 重试同一 spool 文件；已完成 key 返回记录结果 `duplicate:true` 不重建 | ○python `test_idempotency_key_reuses_one_spool_file` / `test_preexisting_marker_reports_duplicate_result` | P0 |
| C2 | watcher 幂等 | `.done` 成功标记抑制重建；失败标记允许重放 | ○Rust `done_marker_suppresses_recreation` / `failure_marker_lets_retry_reapply` | P0 |
| C3 | 毒文件隔离 | 篡改 spool（隔离前缀/越界/schema 漂移/超限）→ `failed/` + `{ok:false}` 标记，不创建 | ○Rust `tampered_spool_is_quarantined_without_creating` | P0 |
| C4 | 持久失败 | ≤3 次重试后隔离 + 失败标记（等待方收到失败而非悬挂） | ○Rust `persistent_failure_retries_then_quarantines_with_failure_marker` | P0 |
| C5 | id 不受信 | spool JSON `id` 字段不控制 watcher 路径（文件名为键） | ○Rust `id_field_is_never_trusted_for_watcher_paths` | P1 |
| D1 | Ask 规则 | 规则集含 `mcp_session-reader_create_session` Ask（无 command 约束）；读工具不加 | ○Rust `scope_deny_ruleset_asks_for_session_create` | P0 |
| D2 | 名称防漂移 | Ask 工具名与内嵌 manifest 注册名逐字节一致 | ○Rust（同上测试内 drift pin） | P0 |
| D3 | 审计 | 成功/失败各落 `session_create` / `session_create_failed` | ○Rust `creates_session_writes_marker_audits_and_notifies` | P1 |
| D4 | 错误脱敏 | 失败标记与 server 错误不含宿主绝对路径 | ○Rust（marker 断言）+ ○python `test_spool_errors_do_not_leak_host_paths` | P0 |
| E1 | 特性开关 | `session-creation` 关闭 → 目录移除该工具；在途调用回结构化 `feature_disabled` | ○Rust 注册表断言（union 语义）+ ○python FeatureGate 系列 | P0 |
| E2 | 注册表聚合 | feature 注册表含 `session-creation`，仅含该工具 | ○Rust `registry_aggregates_session_reader_features` | P0 |
| F1 | 默认安装/升级 | 既有 `~/.pinvou3` 启动后 server.py/manifest 自动重释放，工具可用 | ○`ensure_package_released` 机制（既有）+ 手工 | P1 |
| F2 | smoke 矩阵 | 13 包 tools/list 全等；session-reader 四工具；创建旅程（隔离拒绝/pending/spool/duplicate） | ○`scripts/mcp-server-contract-smoke.py` | P0 |
| F3 | 门禁 | cargo test/clippy/fmt、python unittest、smoke、architecture-guard、fork-guard --fast 全绿 | §6 | P0 |

○ = 已落地测试。

## 6. 测试映射

| 测试 | 覆盖 |
|---|---|
| `scripts/tests/test_session_reader_server.py`（CreateSession* 两组） | A2 A3 B1 B2 B3 C1 D4 E1(前置) |
| `scripts/mcp-server-contract-smoke.py` | F2（含创建旅程） |
| `features/session_creation/mod.rs` 内联测试 | B3 B4(部分) C2 C3 C4 C5 D3 D4 |
| `features/assistant/platform/bridge.rs` 规则集测试 | D1 D2 |
| `features/marketplace/{builtin,types}.rs` | E1 E2 |
| 手工 QA | A1 A4 A5 B4 B5 F1 |

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 提示注入批量开会话污染列表 | L1 逐次 Ask（确认卡含 title/first_message/workspace）+ 审计 + 时间线可见 + 特性开关可整体关闭 |
| 重复创建（标记写失败窗口） | 与 app-automations/messaging 相同的已接受 at-least-once 窗口；标记紧随 create_new 写入 |
| 建后步骤失败留下"半配置"会话 | 全部尽力而为 + 失败审计；绑定失败回滚重建（可重试）；首投失败可由用户/模型经消息通道补投 |
| 后台首转在默认 Yolo 下自动批准普通工具 | 与既有消息投递行为一致（先例已上线）；L1 写工具仍 Ask；已在 §4 披露 |
