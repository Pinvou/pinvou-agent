# Pinvou TUI Codex Vertical Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让用户在交互终端执行 `pinvou` 后进入 Pinvou 自有全屏 TUI，并通过现有 Controller/Node/Codex Adapter 完成真实多轮聊天、流式输出、内联审批、输入请求、中止和 Runtime 选择。

**Architecture:** 先把当前“Node 只返回第一个 `text.delta`、Controller 人工补 `turn.ended`”的阶段 1 旁路升级为完整事件流，再新增只依赖协议与抽象 Backend 的 `pinvou-tui` crate。`pinvou-cli` 负责实现 Controller IPC Backend 和进程拉起，TUI 不依赖 Controller Core、Node 或 Adapter。

**Tech Stack:** Rust 1.89、Ratatui 0.30、Crossterm 0.29、Tokio 1.53、现有长度前缀 IPC、Codex app-server Adapter。

---

## 范围拆分

阶段 3 规格包含持久会话、模型目录、权限策略和多 Runtime 四个可独立演进的子系统。本计划只实现第一个可运行纵向切片：真实 Codex 聊天 TUI。它完成后继续编写并执行两份后续计划：

1. `pinvou-tui-session-model-permissions`：`/resume`、`/model`、`/permissions`、Workspace 默认选择和 cursor/snapshot 恢复；
2. `pinvou-tui-multi-runtime`：Claude Code、CodeBuddy、Kimi Code Adapter 与统一验收。

本计划不得将未实现的 `/resume`、`/model` 或三模式权限显示为可用，也不得用模拟事件宣称真实 Codex 闭环完成。

## 文件结构

### 新建

- `pinvou-cli/crates/tui/Cargo.toml`：独立 TUI crate 依赖。
- `pinvou-cli/crates/tui/src/lib.rs`：公开 `run`、Backend 和错误类型。
- `pinvou-cli/crates/tui/src/backend.rs`：与 Controller 实现无关的窄端口。
- `pinvou-cli/crates/tui/src/action.rs`：终端和 Runtime 动作。
- `pinvou-cli/crates/tui/src/model.rs`：纯 UI 状态、聊天块、overlay 与焦点。
- `pinvou-cli/crates/tui/src/update.rs`：确定性状态转换。
- `pinvou-cli/crates/tui/src/view.rs`：Ratatui 单列聊天渲染。
- `pinvou-cli/crates/tui/src/terminal.rs`：Raw Mode、备用屏幕和 Drop 恢复。
- `pinvou-cli/crates/tui/src/app.rs`：Tokio 事件循环、终端事件源和后台 Backend 任务。
- `pinvou-cli/crates/tui/src/commands.rs`：本切片支持的 slash commands。
- `pinvou-cli/crates/cli/src/distributed/tui_backend.rs`：真实 Controller IPC Backend。

### 修改

- `pinvou-cli/crates/runtime-api/src/adapter.rs`：保持 Adapter stream 可在锁外消费的合同注释和测试。
- `pinvou-cli/crates/node/src/session.rs`：以完整事件订阅替换单事件 `echo`。
- `pinvou-cli/crates/node/src/local_ipc.rs`：逐帧发送 Runtime 事件。
- `pinvou-cli/crates/controller/src/local_node_client.rs`：逐帧读取 Node 事件。
- `pinvou-cli/crates/controller/src/session.rs`：透传完整事件流，不合成终态。
- `pinvou-cli/crates/controller/src/local_ipc.rs`：对 `chat.start` 使用 streaming handler。
- `pinvou-cli/crates/cli/Cargo.toml`：distributed feature 引入 `pinvou-tui`。
- `pinvou-cli/crates/cli/src/lib.rs`：无参数 TTY 路由和不重复打印空输出。
- `pinvou-cli/crates/cli/src/main.rs`：执行 TUI 后不额外输出空行。
- `pinvou-cli/crates/cli/src/distributed/mod.rs`：导出 IPC 连接能力并保留脚本化 `chat` 回归。
- `pinvou-cli/scripts/check_distributed_dependencies.py`：把 `pinvou-tui` 加入正式 root。

### 测试

- `pinvou-cli/crates/node/tests/node_contract.rs`：Node 完整流和回合状态。
- `pinvou-cli/crates/controller/tests/streaming_chat_contract.rs`：Controller 不丢事件、不伪造终态。
- `pinvou-cli/crates/tui/tests/app_contract.rs`：Fake Backend 的真实 UI 状态闭环。
- `pinvou-cli/crates/cli/tests/distributed_cli_contract.rs`：命令路由与 Backend IPC。
- `pinvou-cli/crates/cli/tests/tui_pty_contract.rs`：TTY/非 TTY 与终端恢复。

---

### Task 1: 冻结完整 Runtime 事件流合同

**Files:**
- Modify: `pinvou-cli/crates/node/src/session.rs`
- Modify: `pinvou-cli/crates/node/src/local_ipc.rs`
- Test: `pinvou-cli/crates/node/tests/node_contract.rs`

- [ ] **Step 1: 写出 Node 必须转发全部事件的失败测试**

在 `node_contract.rs` 增加一个 Runtime Host，它依次产生 `turn.started`、两个 `text.delta`、`tool.call.started`、`tool.call.completed`、`turn.ended`，断言 `NodeSession::stream_bound` 原序输出全部事件：

```rust
#[test]
fn node_streams_every_runtime_event_until_the_runtime_terminal_event() {
    let session = NodeSession::with_runtime(
        "node-a",
        Arc::new(ScriptedRuntime::new(vec![
            runtime_event("turn.started", 1),
            runtime_event("text.delta", 2),
            runtime_event("text.delta", 3),
            runtime_event("tool.call.started", 4),
            runtime_event("tool.call.completed", 5),
            runtime_event("turn.ended", 6),
        ])),
    ).unwrap();
    let request = bound_request("chat.start", json!({"prompt":"hello"}));
    let mut events = Vec::new();

    session.stream_bound(request, |message| {
        events.push(RuntimeEventEnvelope::from_value(message.payload().clone()).unwrap());
        Ok(())
    }).unwrap();

    assert_eq!(events.iter().map(|event| event.event_kind()).collect::<Vec<_>>(), vec![
        RuntimeEventKind::TurnStarted,
        RuntimeEventKind::TextDelta,
        RuntimeEventKind::TextDelta,
        RuntimeEventKind::ToolCallStarted,
        RuntimeEventKind::ToolCallCompleted,
        RuntimeEventKind::TurnEnded,
    ]);
}
```

- [ ] **Step 2: 运行测试并确认当前单事件接口失败**

Run:

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-node --test node_contract node_streams_every_runtime_event
```

Expected: FAIL，提示 `NodeSession::stream_bound` 或流式 Runtime Host 方法不存在。

- [ ] **Step 3: 将 `NodeRuntimeHost::echo` 替换为拥有型事件流**

在 `node/src/session.rs` 定义 Node 侧流类型并实现 Adapter Host。调用 `subscribe_events` 后必须释放 `AdapterRuntimeState` mutex，避免审批/中止连接被锁死：

```rust
pub type NodeRuntimeEventStream =
    Box<dyn Iterator<Item = Result<RuntimeEventEnvelope, NodeError>> + Send>;

pub trait NodeRuntimeHost: Send + Sync + std::fmt::Debug {
    fn start_turn(&self, node_id: &str, prompt: &str, seq: u64)
        -> Result<NodeRuntimeEventStream, NodeError>;
    // detect/approval/input/interrupt 保持现有窄接口。
}

impl NodeRuntimeHost for AdapterRuntimeHost {
    fn start_turn(&self, _: &str, prompt: &str, _: u64)
        -> Result<NodeRuntimeEventStream, NodeError>
    {
        let subscription = {
            let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
            let session = ensure_adapter_session(&mut inner)?;
            inner.adapter.send(&session, RuntimeCommand::text(prompt)?)?;
            inner.adapter.subscribe_events(&session)?
        };
        Ok(Box::new(subscription.map(|event| event.map_err(NodeError::from))))
    }
}
```

给 Echo Runtime 返回 `once(Ok(event))`，保持现有 deterministic 测试能力。

- [ ] **Step 4: 增加 `NodeSession::stream_bound` 并逐帧写 IPC**

`stream_bound` 只接受已经绑定正确 `instance_id` 的 `chat.start`，用 `ActiveTurnGuard` 覆盖整个订阅生命周期，并把每个 envelope 包成 `runtime.event`：

```rust
pub fn stream_bound(
    &self,
    request: IpcMessage,
    mut emit: impl FnMut(IpcMessage) -> Result<(), NodeError>,
) -> Result<(), NodeError> {
    validate_bound_request(&request, &self.instance_id, "chat.start")?;
    let prompt = required_non_empty_string(request.payload(), "prompt")?;
    let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
    let _active = ActiveTurnGuard::enter(Arc::clone(&self.active_turn), seq)?;
    for event in self.current_runtime_host()?.start_turn(&self.instance_id, prompt, seq)? {
        let event = event?;
        emit(IpcMessage::event("runtime.event", serde_json::to_value(event)
            .map_err(|_| NodeError::InvalidMessage)?)
            .map_err(|_| NodeError::InvalidMessage)?)?;
    }
    Ok(())
}
```

在 `node/src/local_ipc.rs` 中对 `chat.start` 调用它，并在每个 event 后 `write_all + flush`；其他请求继续使用 `handle`。

- [ ] **Step 5: 验证 Node 流与审批并发测试通过**

补充测试：流停在 `approval.requested` 时，另一个线程调用 `resolve_approval` 不应卡在 Adapter mutex。运行：

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-node
```

Expected: PASS。

- [ ] **Step 6: 提交 Node streaming seam**

```powershell
git add pinvou-cli/crates/node/src/session.rs pinvou-cli/crates/node/src/local_ipc.rs pinvou-cli/crates/node/tests/node_contract.rs
git commit -s -m "feat(runtime): 完整转发节点运行时事件流"
```

---

### Task 2: Controller 逐事件透传且不伪造终态

**Files:**
- Modify: `pinvou-cli/crates/controller/src/local_node_client.rs`
- Modify: `pinvou-cli/crates/controller/src/session.rs`
- Modify: `pinvou-cli/crates/controller/src/local_ipc.rs`
- Create: `pinvou-cli/crates/controller/tests/streaming_chat_contract.rs`

- [ ] **Step 1: 写失败测试证明 Controller 当前只返回首个 delta**

测试构造六个 Node `runtime.event` 帧，调用新的 `ControllerSession::stream_bound`，断言六个 payload 完全相同且最后一个由 Runtime 提供：

```rust
#[test]
fn controller_forwards_node_stream_without_synthesizing_turn_end() {
    let runtime_events = scripted_turn_events();
    let session = controller_with_scripted_node(runtime_events.clone());
    let mut forwarded = Vec::new();

    session.stream_bound(chat_request("hello"), |message| {
        forwarded.push(message);
        Ok(())
    }).unwrap();

    assert_eq!(forwarded.len(), runtime_events.len());
    assert_eq!(forwarded.last().unwrap().payload(),
               &serde_json::to_value(runtime_events.last().unwrap()).unwrap());
}
```

- [ ] **Step 2: 运行测试并确认失败**

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-controller --test streaming_chat_contract
```

Expected: FAIL，当前 Controller 只有 `LocalNodeClient::echo` 和合成 `turn_ended_after`。

- [ ] **Step 3: 给 LocalNodeClient 增加流式读取方法**

```rust
pub fn stream_chat(
    &mut self,
    prompt: &str,
    mut emit: impl FnMut(RuntimeEventEnvelope) -> Result<(), ControllerError>,
) -> Result<(), ControllerError> {
    self.send_request("chat.start", serde_json::json!({"prompt": prompt}))?;
    loop {
        let message: IpcMessage = read_frame(&mut self.stream)
            .map_err(|_| ControllerError::InvalidMessage)?;
        if message.kind() == IpcMessageKind::Err {
            return Err(controller_error_from_wire(message.payload()));
        }
        if message.kind() != IpcMessageKind::Evt
            || message.topic() != Some("runtime.event")
        {
            return Err(ControllerError::InvalidMessage);
        }
        let event = RuntimeEventEnvelope::from_value(message.payload().clone())
            .map_err(|_| ControllerError::InvalidMessage)?;
        let ended = event.event_kind() == RuntimeEventKind::TurnEnded;
        emit(event)?;
        if ended { return Ok(()); }
    }
}
```

把 `request` 的写帧部分提取成 `send_request`，普通 request 仍在发送后读取一个 response。

- [ ] **Step 4: 给 ControllerSession/Local IPC 增加 streaming handler**

`ControllerSession::stream_bound` 验证 `chat.start` 和 Controller `instance_id`，连接 Node 后调用 `stream_chat`，每个事件立即封装并 emit。删除生产路径中的 `turn_ended_after`；仅保留需要它的旧测试 fixture 时应把 fixture 改成显式终态事件。

`controller/src/local_ipc.rs` 的连接 worker 遇到 `chat.start` 时使用 streaming handler，其他请求仍走 `handle_bound_many`。

- [ ] **Step 5: 运行 Controller 与原 CLI streaming 回归**

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-controller -p pinvou-cli --no-default-features --features pinvou-cli/distributed
```

Expected: PASS；现有 `pinvou chat` 继续工作，但现在能看到完整 Runtime 事件流。

- [ ] **Step 6: 提交 Controller streaming pass-through**

```powershell
git add pinvou-cli/crates/controller pinvou-cli/crates/cli/src/distributed/mod.rs pinvou-cli/crates/cli/tests/distributed_cli_contract.rs
git commit -s -m "feat(controller): 透传完整聊天事件流"
```

---

### Task 3: 创建独立 TUI crate 和纯状态转换

**Files:**
- Create: `pinvou-cli/crates/tui/Cargo.toml`
- Create: `pinvou-cli/crates/tui/src/lib.rs`
- Create: `pinvou-cli/crates/tui/src/backend.rs`
- Create: `pinvou-cli/crates/tui/src/action.rs`
- Create: `pinvou-cli/crates/tui/src/model.rs`
- Create: `pinvou-cli/crates/tui/src/update.rs`
- Create: `pinvou-cli/crates/tui/src/commands.rs`
- Test: module tests in the files above

- [ ] **Step 1: 添加 crate manifest**

```toml
[package]
name = "pinvou-tui"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
crossterm = "0.29.0"
pinvou-protocol = { path = "../protocol" }
ratatui = "0.30.0"
thiserror.workspace = true
tokio = { version = "1.53.1", features = ["rt", "macros", "sync", "time"] }
```

- [ ] **Step 2: 先写 Model reducer 失败测试**

```rust
#[test]
fn streaming_turn_approval_and_completion_are_projected_deterministically() {
    let mut model = Model::new(workspace(), runtime("codex"));
    update(&mut model, Action::Submit("hello".into()));
    update(&mut model, Action::Runtime(runtime_event("turn.started")));
    update(&mut model, Action::Runtime(text_delta("hel")));
    update(&mut model, Action::Runtime(text_delta("lo")));
    update(&mut model, Action::Runtime(approval("approval-1", "run cargo test")));
    assert!(matches!(model.interaction, Interaction::Approval { .. }));
    update(&mut model, Action::ApprovalChosen(ApprovalDecision::AllowOnce));
    update(&mut model, Action::Runtime(runtime_event("turn.ended")));
    assert_eq!(model.transcript.assistant_text(), "hello");
    assert_eq!(model.turn, TurnState::Idle);
}
```

- [ ] **Step 3: 定义 Backend、Action 与 Model**

Backend 不暴露 Controller/Core 类型：

```rust
pub trait Backend: Send + Sync + 'static {
    fn workspace(&self) -> Result<PathBuf, BackendError>;
    fn runtime_list(&self) -> Result<RuntimeList, BackendError>;
    fn stream_turn(
        &self,
        prompt: String,
        emit: Box<dyn FnMut(RuntimeEventEnvelope) -> Result<(), BackendError> + Send>,
    ) -> Result<(), BackendError>;
    fn resolve_approval(&self, approval_id: String, accepted: bool)
        -> Result<(), BackendError>;
    fn resolve_input(&self, input_id: String, value: String)
        -> Result<(), BackendError>;
    fn interrupt(&self, turn_id: String) -> Result<(), BackendError>;
    fn switch_runtime(&self, runtime: String) -> Result<RuntimeStatus, BackendError>;
}
```

`Model` 至少包含 workspace、runtime、connection、turn、transcript、composer、interaction、overlay、status_message 和 should_quit。Runtime 原始事件只在 `Action::Runtime` 入口出现，View 不解析 JSON。

- [ ] **Step 4: 实现 reducer 与本切片 slash commands**

支持 `/help`、`/runtime`、`/exit`；普通文本产生 `Effect::StartTurn`。未实现命令返回明确错误：

```rust
pub enum SlashCommand { Help, Runtime, Exit }

pub fn parse(input: &str) -> Result<Option<SlashCommand>, CommandError> {
    match input.trim() {
        value if !value.starts_with('/') => Ok(None),
        "/help" => Ok(Some(SlashCommand::Help)),
        "/runtime" => Ok(Some(SlashCommand::Runtime)),
        "/exit" | "/quit" => Ok(Some(SlashCommand::Exit)),
        value => Err(CommandError::Unknown(value.to_owned())),
    }
}
```

不要显示 `/resume`、`/model`、`/permissions` 为可用命令。

- [ ] **Step 5: 运行 TUI 纯逻辑测试**

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-tui
```

Expected: PASS，不需要真实终端或 Controller。

- [ ] **Step 6: 提交 TUI state core**

```powershell
git add pinvou-cli/crates/tui
git commit -s -m "feat(tui): 建立聊天状态与后端端口"
```

---

### Task 4: 实现 Terminal Guard 和 Claude Code 风格渲染

**Files:**
- Create: `pinvou-cli/crates/tui/src/terminal.rs`
- Create: `pinvou-cli/crates/tui/src/view.rs`
- Test: module tests in both files

- [ ] **Step 1: 写 Terminal Guard 幂等恢复测试**

使用 `TerminalOps` fake 记录调用顺序，覆盖初始化中途失败和正常 Drop：

```rust
#[test]
fn guard_restores_every_enabled_terminal_mode_once() {
    let ops = RecordingTerminalOps::default();
    {
        let _guard = TerminalGuard::enter(ops.clone()).unwrap();
    }
    assert_eq!(ops.calls(), vec![
        "enable_raw", "enter_alt", "hide_cursor", "enable_paste",
        "disable_paste", "show_cursor", "leave_alt", "disable_raw",
    ]);
}
```

- [ ] **Step 2: 实现 Terminal Guard**

Guard 按已成功启用的步骤记录 flags，Drop 逆序恢复。生产 `CrosstermOps` 使用 `enable_raw_mode`、`EnterAlternateScreen`、`Hide`、`EnableBracketedPaste`；恢复错误写入 stderr 前必须先离开备用屏幕。

- [ ] **Step 3: 写 View buffer 快照测试**

用 `ratatui::backend::TestBackend` 渲染 100x30：

```rust
#[test]
fn chat_view_keeps_transcript_composer_and_status_visible() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &populated_model())).unwrap();
    let screen = buffer_text(terminal.backend().buffer());
    assert!(screen.contains("Pinvou Agent"));
    assert!(screen.contains("run cargo test"));
    assert!(screen.contains("Ctrl+R runtime"));
}
```

- [ ] **Step 4: 实现单列 View**

布局固定为 welcome/context、滚动 transcript、composer、status 四段。连续文本流只用缩进和颜色；审批、输入请求和错误使用轻量边框。小于 60x16 时显示最低尺寸提示但仍允许退出。

- [ ] **Step 5: 运行渲染与终端测试**

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-tui terminal view
```

Expected: PASS。

- [ ] **Step 6: 提交终端与 View**

```powershell
git add pinvou-cli/crates/tui/src/terminal.rs pinvou-cli/crates/tui/src/view.rs
git commit -s -m "feat(tui): 实现终端恢复与聊天界面"
```

---

### Task 5: 实现异步事件循环、编辑器和交互审批

**Files:**
- Create: `pinvou-cli/crates/tui/src/app.rs`
- Modify: `pinvou-cli/crates/tui/src/lib.rs`
- Create: `pinvou-cli/crates/tui/tests/app_contract.rs`

- [ ] **Step 1: 写 Fake Backend 端到端失败测试**

用动作脚本代替真实键盘，验证输入、流、审批、第二轮和退出：

```rust
#[tokio::test(flavor = "current_thread")]
async fn app_completes_two_turns_and_one_inline_approval() {
    let backend = Arc::new(FakeBackend::scripted(two_turns_with_approval()));
    let actions = vec![
        key_text("first"), key_enter(),
        approval_key('1'),
        key_text("second"), key_enter(),
        ctrl_c(),
    ];
    let result = run_with_driver(backend.clone(), actions).await.unwrap();
    assert_eq!(backend.prompts(), ["first", "second"]);
    assert_eq!(backend.approvals(), [("approval-1", true)]);
    assert!(result.detached);
}
```

- [ ] **Step 2: 实现唯一 Crossterm 读取任务**

只允许 `terminal_input_task` 调用 `crossterm::event::read()`；把 Key、Paste、Resize 映射为 `Action` 后发送到 Tokio mpsc。禁止 View 或 Backend 读取终端。

- [ ] **Step 3: 实现 effect runner**

`Effect::StartTurn` 使用 `spawn_blocking` 调用 Backend，并把每个 Runtime event 发回 Action channel。审批、输入和中止使用独立 Backend 调用，因此不会被 streaming connection 阻塞。

- [ ] **Step 4: 实现键盘语义**

- 普通模式：字符编辑、Backspace、Enter 发送、粘贴插入、上下滚动；
- Streaming：Esc 调用 interrupt；
- Approval：`1` Allow once，`3` Deny；当前 IPC 只有一次性布尔决策，不显示“允许规则”；
- Input request：Enter 提交回答；
- Runtime overlay：上下选择、Enter 执行 Prepare/Commit、Esc 关闭；
- Ctrl+C：detach 并退出 TUI，不终止后台任务。

- [ ] **Step 5: 运行 app contract**

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-tui --test app_contract
```

Expected: PASS。

- [ ] **Step 6: 提交事件循环**

```powershell
git add pinvou-cli/crates/tui/src/app.rs pinvou-cli/crates/tui/src/lib.rs pinvou-cli/crates/tui/tests/app_contract.rs
git commit -s -m "feat(tui): 接通聊天事件循环与审批"
```

---

### Task 6: 将真实 Controller IPC 接到 TUI Backend

**Files:**
- Create: `pinvou-cli/crates/cli/src/distributed/tui_backend.rs`
- Modify: `pinvou-cli/crates/cli/src/distributed/mod.rs`
- Modify: `pinvou-cli/crates/cli/Cargo.toml`
- Test: `pinvou-cli/crates/cli/tests/distributed_cli_contract.rs`

- [ ] **Step 1: 写 Backend wire 合同失败测试**

FakeDuplex 注入完整事件流，断言 `stream_turn` 发出 `chat.start` 并逐条 emit；`resolve_approval`、`resolve_input`、`interrupt` 和 Runtime Prepare/Commit 使用独立请求：

```rust
#[test]
fn tui_backend_streams_events_and_uses_separate_control_requests() {
    let harness = BackendHarness::new(scripted_controller_frames());
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    harness.backend.stream_turn("hello".into(), Box::new(move |event| {
        captured.lock().unwrap().push(event);
        Ok(())
    })).unwrap();
    assert_eq!(events.lock().unwrap().len(), 6);
    assert_eq!(harness.requests()[0].method(), Some("chat.start"));
}
```

- [ ] **Step 2: 增加 CLI feature 依赖**

```toml
[features]
distributed = [
  "dep:ctrlc",
  "dep:pinvou-controller",
  "dep:pinvou-protocol",
  "dep:pinvou-tui",
]

[dependencies]
pinvou-tui = { path = "../tui", optional = true }
```

- [ ] **Step 3: 实现 `ControllerTuiBackend`**

Backend 保存 workspace 路径，不保存可并发共享的 wire。每个普通命令调用 `ensure_controller()` 获取短连接；`stream_turn` 获取独立 streaming connection并读到真实 `turn.ended`。Runtime 切换严格复用 `detect -> prepare -> commit -> detect`，不调用已废弃的直接 `runtime.switch`。

- [ ] **Step 4: 映射 Backend 错误**

把 `StableExitCode` 和原始 message 保存进 `BackendError`，TUI 显示可操作错误；BlockedAuth、Cancelled、ControllerUnavailable 不折叠成通用字符串。

- [ ] **Step 5: 运行 CLI Backend 测试**

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-cli --no-default-features --features pinvou-cli/distributed tui_backend
```

Expected: PASS。

- [ ] **Step 6: 提交真实 Backend**

```powershell
git add pinvou-cli/crates/cli/Cargo.toml pinvou-cli/crates/cli/src/distributed
git commit -s -m "feat(tui): 接入控制器聊天后端"
```

---

### Task 7: 让 `pinvou` 成为唯一 TUI 入口

**Files:**
- Modify: `pinvou-cli/crates/cli/src/lib.rs`
- Modify: `pinvou-cli/crates/cli/src/main.rs`
- Modify: `pinvou-cli/crates/cli/tests/distributed_cli_contract.rs`
- Create: `pinvou-cli/crates/cli/tests/tui_pty_contract.rs`

- [ ] **Step 1: 更新命令路由失败测试**

```rust
#[test]
fn no_arguments_route_to_tui_only_in_the_distributed_product() {
    assert_eq!(parse_args(["pinvou"]).unwrap().command(), &CliCommand::Tui);
    assert_eq!(parse_args(["pinvou", "--help"]).unwrap().command(), &CliCommand::Help);
}
```

增加非 TTY 测试，断言 exit code 2 且错误包含“当前不是交互终端，请使用具体子命令”。明确断言 `parse_args(["pinvou", "tui"])` 是未知命令。

- [ ] **Step 2: 实现 `CliCommand::Tui` 和无参数解析**

distributed feature 下空参数解析为 `Tui`；help/version 和显式子命令保持原行为。删除帮助中的 `pinvou chat` 主推荐，只保留为兼容/诊断命令并标明 advanced。

- [ ] **Step 3: 执行 TUI 且不额外打印空行**

`execute(Tui)` 先做 TTY 检查，再创建 `ControllerTuiBackend` 并调用 `pinvou_tui::run`。`main.rs` 只在 stdout 非空时 `println!`：

```rust
if !outcome.stdout.is_empty() {
    println!("{}", outcome.stdout);
}
```

- [ ] **Step 4: PTY 测试退出后终端恢复**

使用仓库已有测试运行方式启动 `pinvou`，发送 Ctrl+C，断言进程成功退出且输出包含 LeaveAlternateScreen/Show Cursor 对应序列；管道执行无参数必须在 1 秒内退出而非挂起。

- [ ] **Step 5: 运行命令路由与 PTY 测试**

```powershell
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-cli --no-default-features --features pinvou-cli/distributed --test distributed_cli_contract --test tui_pty_contract
```

Expected: PASS。

- [ ] **Step 6: 提交产品入口**

```powershell
git add pinvou-cli/crates/cli/src/lib.rs pinvou-cli/crates/cli/src/main.rs pinvou-cli/crates/cli/tests
git commit -s -m "feat(cli): 默认启动 Pinvou TUI"
```

---

### Task 8: 更新分布式依赖门禁并完成自动验证

**Files:**
- Modify: `pinvou-cli/scripts/check_distributed_dependencies.py`
- Modify: `pinvou-cli/Cargo.lock`

- [ ] **Step 1: 将 `pinvou-tui` 加入正式 roots**

```python
DEFAULT_ROOTS = (
    "pinvou-cli",
    "pinvou-tui",
    "pinvou-controller",
    "pinvou-node",
    "pinvou-protocol",
    "pinvou-seglog",
    "pinvou-runtime-api",
    "pinvou-agent-adapter-codex",
)
```

- [ ] **Step 2: 更新 lockfile 并证明闭包不含 CodeWhale/Tauri**

```powershell
cargo check --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-cli -p pinvou-tui --no-default-features --features pinvou-cli/distributed
python pinvou-cli/scripts/check_distributed_dependencies.py
```

Expected: 两条命令 PASS，正式闭包只包含批准的 Pinvou crates 与 Ratatui/Crossterm/Tokio 等通用依赖。

- [ ] **Step 3: 运行完整自动门禁**

```powershell
cargo fmt --manifest-path pinvou-cli/Cargo.toml --all -- --check
cargo test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-runtime-api -p pinvou-agent-adapter-codex -p pinvou-node -p pinvou-controller -p pinvou-tui -p pinvou-cli --no-default-features --features pinvou-cli/distributed
cargo check --manifest-path pinvou-cli/Cargo.toml --offline --release -p pinvou-cli -p pinvou-controller -p pinvou-node --no-default-features --features pinvou-cli/distributed
python pinvou-cli/scripts/check_stage1_zero_diff.py --base b40302c6d4b8258a4edb469444be95c6a3a7e506
git diff --check
C:/Users/c24894/.codex/hooks/gsd-check-update.cmd
```

Expected: 全部 PASS。若 stage1 zero-diff 脚本仍把“新增 TUI crate”视为阶段 1 禁止项，不得放宽旧门禁；新增独立 stage3 门禁入口，并继续让旧 stage1 job 使用旧 roots。

- [ ] **Step 4: 提交门禁更新**

```powershell
git add pinvou-cli/Cargo.lock pinvou-cli/scripts/check_distributed_dependencies.py
git commit -s -m "ci(tui): 纳入分布式终端依赖门禁"
```

---

### Task 9: 真实 Codex TUI 验收

**Files:**
- Modify: `docs/superpowers/specs/2026-08-24-pinvou-tui-stage3-design.md`

- [ ] **Step 1: 确认本机 Codex 探测和认证**

```powershell
cargo run --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-cli --no-default-features --features pinvou-cli/distributed -- runtime detect codex
```

Expected: `status: available`、`auth: authenticated`、`interactive_chat: yes`、`tool_approval: yes`。

- [ ] **Step 2: 把当前 Runtime 切换到 Codex**

```powershell
cargo run --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-cli --no-default-features --features pinvou-cli/distributed -- runtime switch codex
```

Expected: Prepare/Commit 成功，后续 detect 仍为 available。

- [ ] **Step 3: 从真实终端启动唯一入口**

```powershell
pinvou-cli\target\debug\pinvou.exe
```

Expected: 进入 Pinvou 自有全屏 TUI；标题、Workspace、Codex 状态、输入区和底部快捷键可见，没有 `You:` 行式循环，也没有 CodeWhale 标识。

- [ ] **Step 4: 完成真实多轮与审批用例**

依次执行：

1. “只回复 READY”；
2. “读取当前目录 Cargo.toml，并告诉我 workspace member 配置”；
3. 触发一个需要审批但无破坏性的命令，例如读取 Git 状态；
4. 在审批卡片选择 Allow once；
5. 发起一个较长回合后按 Esc 中止；
6. 输入 `/runtime`，确认列表和当前 Codex 状态，再 Esc 返回；
7. Ctrl+C detach 退出。

Expected: 两轮响应均完整显示；工具事件、审批、取消和退出都不破坏终端；TUI 不要求退回 `pinvou chat` 完成关键步骤。

- [ ] **Step 5: 记录验收边界**

在阶段 3 设计文档末尾增加“实施状态”小节，记录实际命令、真实 Runtime 版本、通过/失败结果，并明确写“Codex TUI 纵向切片可用”；同时列出下一计划必须完成的 `/resume`、`/model`、`/permissions` 和 cursor/snapshot，不能宣称完整阶段 3 完成。

- [ ] **Step 6: 提交真实验收中产生的必要修复和记录**

```powershell
git add docs/superpowers/specs/2026-08-24-pinvou-tui-stage3-design.md
git commit -s -m "test(tui): 验证 Codex 真实聊天闭环"
```

若验收发现代码缺陷，回到对应任务先写失败测试、修复并提交，再重新执行 Task 9；本步骤只提交验收记录。如果没有文档变化，不创建空提交。

---

## 计划自检结论

- 规格覆盖：本计划覆盖独立 TUI、唯一 `pinvou` 入口、完整流式聊天、工具事件、审批、输入、中止、Runtime 选择、终端恢复和真实 Codex 验收。
- 明确后置：`/resume`、`/model`、`/permissions`、Workspace 级选择、cursor/snapshot 和多 Runtime 不在本计划伪实现，由两份后续计划完成。
- 边界一致：`pinvou-tui` 只依赖抽象 Backend 和协议，不依赖 Controller Core、Node、Adapter、Desktop、Tauri 或 CodeWhale。
- 类型一致：Runtime 事件统一使用 `RuntimeEventEnvelope`；TUI 只消费归一化事件；Controller/Node streaming handler 均以真实 `turn.ended` 终止。
- 无完成夸大：只有 Task 9 通过后才能称“Codex TUI 纵向切片可用”。
