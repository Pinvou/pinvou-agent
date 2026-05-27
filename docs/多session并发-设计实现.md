# 多 session 并发：设计与实现

> 创建：2026-05-27
> 状态：已落地 + GUI 冒烟通过（commit e3f64a7）
> 范围：pinvou3-app（前端 + Rust wrapper），未改 DeepSeek-TUI 底座

---

## 1. 背景：为什么改

旧模型整个进程**只有一个 Engine**（`lib.rs` setup 时 `spawn_engine` 一次），切 session 靠 `Op::SyncSession` 把这唯一引擎的内部状态（messages / workspace / system_prompt）**整体替换**。后果：

1. 同一时刻引擎只能服务一个 session，**无法并发**；
2. 切走正在生成的 session 时，流式事件不带 session_id，会漏进新 session 视图 → 「状态乱掉」。

目标：**多 session 同时对话、各自跑完任务**。

底座 `spawn_engine`（`DeepSeek-TUI/crates/tui/src/core/engine.rs`）是独立工厂，每调一次造一套完全独立的 Engine + channel + agent loop。所以「每 session 一个引擎」是**复用底座**、非自建调度（守 CLAUDE.md 约束 1）。

---

## 2. 三个必须解开的全局耦合

| 耦合点 | 旧实现 | 并发下的问题 |
|---|---|---|
| 单 Engine 实例 | 1 个 `AppEngine` 存 Tauri State | 串行,无法并发 |
| 事件无 session 归属 | `app.emit` 多数不带 session_id | 后台引擎事件漏进 active 视图 |
| **全局 `bundle/instructions.md`** | `rewrite_instructions_for_session` 写共享文件 | 多引擎 rehydrate 互相覆写 → system_prompt 串台、产物写错 workspace |

第 3 个最隐蔽：engine 的 `rehydrate_latest_canonical_state()` 会从 `EngineConfig.instructions` 指向的 disk 文件**重读覆盖** system_prompt。旧代码所有 session 共用 `bundle.instructions_md` 一个文件，单引擎时无所谓，多引擎并发必然串台。

---

## 3. 实现

### 3.1 后端

**引擎专属 config**（`bridge/mod.rs`、`bridge/paths.rs`）
- 每 session 一份 `~/.pinvou3/sessions/<id>/instructions.md`（`session_instructions_path` + `write_session_instructions`），渲染时把 `{{PINVOU3_WORKSPACE}}` 占位符替换成该 session 的 workspace。
- `build_engine_config_for_session(id)`：在 `build_engine_config()` 基础上覆盖 `workspace` + `instructions` 为 session 专属。**不再引用全局 `bundle/instructions.md`。**
- 旧 `rewrite_instructions_for_session` 标 `#[deprecated]` 保留兼容。

**EnginePool**（新增 `engine_pool.rs`，Tauri State）
- `get_or_spawn(sid)`：命中复用；未命中 → 写 session instructions → `build_engine_config_for_session` → `spawn_engine` → 若有磁盘历史则一次性 `SyncSession` 注水 → 启专属 forwarder。**lazy spawn**（首条消息才起）。
- `handle_for(sid)`：只查不 spawn（cancel/submit 用）。
- `evict(sid)`：删 session 时回收（cancel 在跑的 turn + `Op::Shutdown` + abort forwarder）。
- 用 `tokio::Mutex` 全程持锁 spawn，杜绝同 session 并发 spawn 两个引擎。

**事件转发器**（`engine.rs::spawn_event_forwarder`）
- 加 `session_id` 参数，**所有 emit 的 payload 都带 `session_id`**。
- TurnComplete 的 mode 判据（plan_ready / M2 自驱 / M3 文本兜底）用本 forwarder 的 `session_id`，**不再读全局 `store.active_id()`**。
- `plan_tracker` 天然 per-engine（每次 spawn 各一份）。

**command 路由**（`commands.rs`）
- chat / cancel_generation / edit_last_turn / compact_now / submit_user_input / cancel_user_input / accept_plan 带 `session_id`（`Option`,兼容回退 active）路由到池。
- create_session / start_skill_session / load_session **删掉** `sync_session`（lazy，切换不再替换引擎）。
- delete_session 加 `pool.evict`。
- set_super_permission 改为 `refresh_all_instructions()`（重写所有活引擎的 instructions，下个 turn 生效）。

`SyncSession` 新角色：从「切换时替换状态」退化为「冷启动注水历史」——仅在某 session 首次需要引擎、且已有磁盘历史时调一次。

### 3.2 前端（`tauri-bridge.js`）

**工作集 + per-session 缓冲**模型：
- active session 的工作集 = 现有 `state.*` + 模块级 stream 全局（逻辑零改动）。
- 后台 session 工作集存 `sessionStates[id]`。
- 13 个事件监听器经 `onSessionEvent(e, fn)` 按 `payload.session_id` 路由：active 直接跑；后台用 `runSyncOnSession` 临时切工作集跑**同步**逻辑再切回（期间 `suppressNotify`，避免把后台渲染成 active）。
- `chat:done` 特殊：同步收尾（flush / busy=false / 品悟卡 / mode 复位）走路由；**异步收尾**（discard_plan / 品悟终审 / 落盘 / 刷新列表）按显式 `sid` 路由，不依赖工作集——所以后台 session 跑完也能正确落盘。
- `switchToSession` **不再 cancel** 旧 session（后台继续跑）；已有 buffer 直接换工作集，没有则 `load_session` + `rerenderFromMessages`。
- `notify()` 投影 active 工作集到 `state.*`，并建 `state.sessionBusy` 供会话列表「工作中」转圈（`index.html`）。

---

## 4. 当前边界

- **active-only 渲染**：后台 session 的流式输出不实时显示，切回才 `rerenderFromMessages`；会话列表用小圆点转圈表示「工作中」。后端已支持完整并发，**后台流式实时视图是后续可选项**（未做）。
- vLLM 单端点靠 continuous batching 扛 N 并发，个位数 session 可接受；不自建调度（守 CLAUDE.md）。
- 专属气泡（plan_card / user_input / pinvou_actions）不进 messages，切换还原仍靠 `rerenderFromMessages`（见 commit 历史 / memory）。

## 5. 验证

- `cargo test --lib`（含 `engine_config_for_session_paths_are_isolated` 路径隔离单测）、L1 harness 编译、`./scripts/fork-guard.sh` 全绿。
- GUI 冒烟：A 发长任务 → 切 B 发任务 → 两者并发跑、产物各进各自 workspace、切换不串台、列表两项都转圈。
