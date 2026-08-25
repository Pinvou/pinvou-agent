# Pinvou TUI Session、Model、Permissions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让用户直接运行 `pinvou` 后，在真实 TUI 内完成当前 Workspace 会话恢复、当前 Runtime 模型切换和三档权限切换，并在退出重开后恢复一致状态。

**Architecture:** `runtime-api` 定义跨 Runtime 的稳定领域合同；Codex Adapter 只负责原生能力映射；Node 暴露 Runtime 操作；Controller 持有 Logical Session、Workspace Preferences、WAL cursor 与 prepare/commit 原子切换；TUI 只依赖统一 Backend。所有活动状态变更都在回合边界执行，失败时保留旧会话、模型、权限和 Attachment。

**Tech Stack:** Rust 1.89、Serde/JSON、现有长度前缀 IPC、Seglog、Codex app-server、Tokio、Ratatui/Crossterm。

**Build environment:** Windows 上使用 `D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe`，并设置新的 D 盘 `CARGO_TARGET_DIR`，避免再次占用 C 盘。

---

## 范围与完成边界

本计划只修改 `pinvou-cli/` 与本计划文档，不修改 Desktop、Tauri、CodeWhale gitlink 或产品后端。首个可交付闭环以 Codex Runtime 为准；其他 Runtime 通过 capability 返回 unsupported/partial，不伪造能力。

完成时必须满足：

- `/resume` 读取当前 Workspace 的真实 Logical Session，恢复 snapshot 后只补放 cursor 之后的事件；
- `/model` 来自 Runtime 动态目录，prepare/commit 失败不改变当前模型；
- `/permissions` 使用 `request`、`assisted`、`full_access` 产品语义，并显示 Runtime 实际控制强度；
- `pinvou` 仍默认直接启动 TUI，新会话默认权限为 `request`；
- 至少一次真实 Codex 多轮聊天完成“聊天 → 切模型 → 改权限 → 工具调用 → 退出 → 重开 → 恢复”。

## 任务 1：冻结统一领域合同

**Files:**
- Modify: `pinvou-cli/crates/runtime-api/src/model.rs`
- Modify: `pinvou-cli/crates/runtime-api/src/adapter.rs`
- Modify: `pinvou-cli/crates/runtime-api/src/lib.rs`
- Test: `pinvou-cli/crates/runtime-api/tests/runtime_contract.rs`

- [x] 先写序列化和不变量失败测试，覆盖 `SessionDescriptor`、`SessionSnapshot`、`ModelCatalog`、`ApprovalProfile`、`PermissionCapability` 与稳定 snake_case JSON。
- [x] 为 `LogicalSessionId`、`ModelId` 增加非空校验；prepare token 在 Controller 任务中实现；`ModelCatalog` 必须恰有零或一个默认项，当前项必须存在于可用目录。
- [x] 给 `RuntimeCapabilities` 增加 `session_listing`、`model_catalog`、`model_switching`、`permission_profiles` 证据字段，同时保持旧 JSON 缺字段可反序列化。
- [x] 扩展 `AgentRuntimeAdapter`：

```rust
fn list_sessions(&mut self, operation: RuntimeOperation) -> Result<Vec<SessionDescriptor>, AdapterError>;
fn read_session(&mut self, operation: RuntimeOperation) -> Result<SessionSnapshot, AdapterError>;
fn list_models(&mut self, operation: RuntimeOperation) -> Result<ModelCatalog, AdapterError>;
fn inspect_permissions(&mut self, operation: RuntimeOperation) -> Result<PermissionCapability, AdapterError>;
```

默认实现必须返回 `AdapterError::unsupported`，避免破坏其他 Adapter。

- [x] 运行：

```powershell
$env:PATH='D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin;'+$env:PATH
$env:CARGO_TARGET_DIR='D:\pinvou-cargo-target-session-model-permissions'
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-runtime-api --test runtime_contract
```

Expected: PASS。

## 任务 2：Codex 原生 Session、Model、Permissions 映射

**Files:**
- Modify: `pinvou-cli/crates/agent-adapter-codex/src/process.rs`
- Modify: `pinvou-cli/crates/agent-adapter-codex/src/projector.rs`
- Test: `pinvou-cli/crates/agent-adapter-codex/tests/adapter_contract.rs`
- Modify: `pinvou-cli/crates/agent-adapter-codex/tests/schema/used-methods.json`

- [ ] 写黑盒失败测试，脚本化 app-server 响应 `thread/list`、`thread/read`、`model/list`，断言 Adapter 返回统一描述且不泄漏未知原始字段。
- [ ] `create`、`resume` 与 `send` 从 `RuntimeOperation.options` 读取经过验证的 `model_id` 和 `approval_profile`，统一映射：
  - `request` → `approvalPolicy=on-request` + `workspaceWrite`；
  - `assisted` → `approvalPolicy=on-failure`（Runtime 不支持时返回 partial，而非静默提升）；
  - `full_access` → `approvalPolicy=never` + `danger-full-access`，仅接受显式确认标记。
- [ ] `list_sessions` 只列当前 cwd 对应 Workspace 的 Codex threads；`read_session` 规范化用户、助手、工具和终态，不执行任何工具。
- [ ] `list_models` 支持分页直到 `nextCursor` 为空，返回稳定 ID、显示名、默认/可用标志。
- [ ] 将探测得到的模型与权限能力写入 capability evidence；未知 Codex 版本返回 partial/unsupported。
- [ ] 运行 Adapter 合约测试：

```powershell
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-agent-adapter-codex --test adapter_contract
```

Expected: PASS；不运行付费 `live_smoke`。

## 任务 3：Node 统一操作闭环

**Files:**
- Modify: `pinvou-cli/crates/node/src/session.rs`
- Modify: `pinvou-cli/crates/node/src/local_ipc.rs`
- Test: `pinvou-cli/crates/node/tests/node_contract.rs`

- [ ] 为 `NodeRuntimeHost` 写失败测试并增加 `session.list`、`session.read`、`session.resume`、`model.list`、`permissions.inspect`。
- [ ] `AdapterRuntimeHost` 维护当前 Attachment（Runtime session ID、runtime、model、permission、epoch）；resume 成功后才原子替换旧 Attachment。
- [ ] `chat.start` 接受 Controller 传入的 Logical Session、model 与 permission 快照；请求 epoch 与当前 Attachment 不一致时拒绝。
- [ ] 将统一错误映射为固定安全文案与稳定错误码，原始 app-server stderr 只进入脱敏诊断日志。
- [ ] 运行：

```powershell
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-node --test node_contract
```

Expected: PASS。

## 任务 4：Controller 持久 Logical Session 与 Workspace Preferences

**Files:**
- Create: `pinvou-cli/crates/controller/src/session_store.rs`
- Create: `pinvou-cli/crates/controller/src/workspace_store.rs`
- Modify: `pinvou-cli/crates/controller/src/lib.rs`
- Modify: `pinvou-cli/crates/controller/src/paths.rs`
- Modify: `pinvou-cli/crates/controller/src/wal.rs`
- Test: `pinvou-cli/crates/controller/tests/session_store_contract.rs`

- [ ] 先写临时数据根测试：创建会话、追加事件、生成 snapshot、重开 store，并断言 cursor 后事件只应用一次。
- [ ] 保存版本化 `metadata.json`、`snapshot.json`、`events.seglog`；metadata/snapshot 使用同目录临时文件、flush 后原子替换。
- [ ] Workspace key 使用规范化绝对路径的不可逆 hash；preferences 保存 runtime、`model_by_runtime`、approval profile，但不保存凭据或原始 Workspace 路径到索引名。
- [ ] 对损坏 JSON、sequence 缺口、重复 sequence、未知 schema 返回可分类错误；不得猜测恢复成功。
- [ ] 保证落盘顺序为 WAL durable → metadata/snapshot 替换 → 返回成功。
- [ ] 运行：

```powershell
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-controller --test session_store_contract
```

Expected: PASS。

## 任务 5：Controller prepare/commit 与聊天事件持久化

**Files:**
- Modify: `pinvou-cli/crates/controller/src/session.rs`
- Modify: `pinvou-cli/crates/controller/src/local_node_client.rs`
- Modify: `pinvou-cli/crates/controller/src/local_ipc.rs`
- Modify: `pinvou-cli/crates/controller/src/daemon.rs`
- Test: `pinvou-cli/crates/controller/tests/controller_contract.rs`
- Test: `pinvou-cli/crates/controller/tests/streaming_chat_contract.rs`

- [ ] 写失败测试覆盖 `session.list`、`session.resume.prepare/commit`、`model.list/switch.prepare/commit`、`permissions.inspect/switch.prepare/commit`。
- [ ] token 绑定 Controller instance nonce、Logical Session、Attachment epoch、目标值和 capability evidence version；重启、重复提交或任一字段变化都拒绝。
- [ ] `chat.start` 在转发每个 Node event 前先追加 Logical Session WAL；terminal event 后更新 metadata/snapshot 与 Workspace 最近会话索引。
- [ ] resume commit 顺序：读 snapshot → 补放 cursor 后事件 → 校验 Attachment → Node native resume → 成功后切换活动会话；失败保持原会话。
- [ ] model/permission commit 只允许 idle；成功后更新 Attachment epoch 和 Workspace Preferences，失败保持旧状态。
- [ ] 运行 Controller 合约：

```powershell
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-controller --test controller_contract
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-controller --test streaming_chat_contract
```

Expected: PASS。

## 任务 6：扩展 TUI Backend 与 CLI IPC Adapter

**Files:**
- Modify: `pinvou-cli/crates/tui/src/backend.rs`
- Modify: `pinvou-cli/crates/cli/src/distributed/tui_backend.rs`
- Modify: `pinvou-cli/crates/cli/src/distributed/mod.rs`
- Test: `pinvou-cli/crates/cli/tests/distributed_cli_contract.rs`

- [ ] 在 TUI-owned port 定义 `SessionList`、`ModelList`、`PermissionStatus` 和三种 switch/resume 结果，不让 TUI 依赖 Controller crate。
- [ ] Backend 方法对外仍是一次语义操作；CLI Adapter 内部完成 prepare 后立即 commit，并校验两阶段响应关联。
- [ ] `stream_turn` 附带活动 Logical Session；Controller 重连不得自动重发非幂等操作。
- [ ] 测试 IPC 方法序列、超时、prepare 成功但 commit 失败、过期 token，断言安全错误且旧状态不变。
- [ ] 运行：

```powershell
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-cli --test distributed_cli_contract
```

Expected: PASS。

## 任务 7：TUI `/resume`、`/model`、`/permissions` 状态机

**Files:**
- Modify: `pinvou-cli/crates/tui/src/commands.rs`
- Modify: `pinvou-cli/crates/tui/src/model.rs`
- Modify: `pinvou-cli/crates/tui/src/action.rs`
- Modify: `pinvou-cli/crates/tui/src/update.rs`
- Test: `pinvou-cli/crates/tui/tests/app_contract.rs`

- [ ] 把命令表扩展为 `/resume`、`/model`、`/permissions`，删除“这些命令必须未知”的旧断言。
- [ ] 为三个 overlay 分别保存候选列表、选中项、搜索文本、operation token、loading/error；任一时刻只允许一个 overlay。
- [ ] 流式回合、审批或输入请求期间拒绝打开/提交切换；Escape 只取消 overlay，Enter 才提交。
- [ ] resume 成功后一次性替换 transcript、active session、runtime、model、permission 与 cursor；失败完整保留旧 Model。
- [ ] `full_access` 增加二次确认状态；partial/unsupported 必须直接显示，不允许以成功颜色渲染。
- [ ] 运行纯状态机测试：

```powershell
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-tui --test app_contract
```

Expected: PASS。

## 任务 8：TUI 渲染、键盘与异步 Effect

**Files:**
- Modify: `pinvou-cli/crates/tui/src/view.rs`
- Modify: `pinvou-cli/crates/tui/src/renderer.rs`
- Modify: `pinvou-cli/crates/tui/src/app.rs`
- Modify: `pinvou-cli/crates/tui/tests/production_contract.rs`
- Test: `pinvou-cli/crates/cli/tests/tui_pty_contract.rs`

- [ ] 渲染 Claude Code 风格单列 overlay：标题、筛选框、候选、当前/默认/unsupported 标记和底部按键提示。
- [ ] `/resume` 支持普通字符搜索、Backspace、Up/Down、Enter、Escape；另两个 overlay 支持 Up/Down、Enter、Escape。
- [ ] 后台调用沿用有界 channel 与 control lease；退出时 detach 本地请求，不中止远端回合、不隐式提交切换。
- [ ] PTY 测试验证三个命令可打开真实 overlay、取消后输入恢复、退出后 Raw Mode/备用屏幕恢复。
- [ ] 运行：

```powershell
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-tui --test production_contract
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe test --manifest-path pinvou-cli/Cargo.toml --offline -p pinvou-cli --test tui_pty_contract
```

Expected: PASS。

## 任务 9：必要门禁与一次真实闭环验收

**Files:**
- Modify when behavior changes: `pinvou-cli/README.md`
- Modify: `docs/superpowers/specs/2026-08-25-pinvou-tui-session-model-permissions-design.md`

- [ ] 运行格式化与只涉及本阶段 packages 的 Clippy：

```powershell
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe fmt --manifest-path pinvou-cli/Cargo.toml --all -- --check
D:\RustNdk\.rustup\toolchains\1.97.1-x86_64-pc-windows-msvc\bin\cargo.exe clippy --manifest-path pinvou-cli/Cargo.toml --offline --no-default-features --features pinvou-cli/distributed -p pinvou-runtime-api -p pinvou-agent-adapter-codex -p pinvou-node -p pinvou-controller -p pinvou-tui -p pinvou-cli -- -D warnings
```

- [ ] 运行正式分布式边界门禁：

```powershell
python pinvou-cli/scripts/check_distributed_dependencies.py
python pinvou-cli/scripts/check_stage1_zero_diff.py
```

- [ ] 构建一次正式 CLI 后做真实 Windows PTY 验收：`pinvou` 启动 TUI；聊天两轮；`/model` 切换；`/permissions` 依次验证 request 与 full_access 风险确认；执行一个只读工具；退出；重开；`/resume` 恢复同一会话；确认已完成工具不重放。
- [ ] 若付费/登录环境不可用，只把该项如实标记为未验证，不用 fake backend 代替真实验收。
- [ ] 更新设计状态、README 命令说明和已知限制；检查 `git diff --check`，只提交范围内文件。

---

## 提交策略

每个可独立回滚的纵向任务使用一条带 DCO 的提交，格式示例：

```text
feat(runtime): 定义会话模型权限统一合同
feat(codex): 映射原生会话模型权限能力
feat(controller): 持久化逻辑会话与工作区选择
feat(tui): 补齐会话模型权限交互闭环
test(cli): 验证 TUI 恢复切换真实链路
```

不得提交 `tmp/`、用户现有研究文档或其他无关未跟踪文件。
