# kimi 迁移 · 里程碑 0(PoC)结果

> 日期 2026-05-31 · 分支 worktree-kimi-base-migration · 目标:验"kimi SDK 驱动本地 Qwen3.6"两条红线
> 结论:**PoC 全绿,sidecar 方案实锤可行**。下一步是 ~2700 行 Rust 桥接层重写(架构岔路,待确认)。

## 环境(全部非侵入,装在 worktree scratch)
- kimi-code 源码:`_kimi_ref/`(MoonshotAI/kimi-code,纯 TS,无 Rust)
- Node 24.15.0:`.tooling/node24/`(系统是 Node 22,kimi 要 ≥24.15 且 `engine-strict=true`,故本地装)
- SDK 未发布 npm,从 monorepo 源码构建:`pnpm --filter "@moonshot-ai/kimi-code-sdk..." build` → `dist/index.mjs`(4.21MB,已内联 .md 工具描述)
- PoC 脚本/配置:`_kimi_ref/.poc/`

## 本地端点(从 pinvou3 源码探得 + 实测在线)
- 端点:`http://10.214.74.113:8000/v1`(GB10 开发机,实测在线;本地 127.0.0.1:8000 未跑 /v1)
- 模型 served-name:`qwen36_35b_256k`,max_model_len 262144

## kimi 接本地模型的配置(`.poc/home/config.toml`)
```toml
default_model = "qwen-local"
default_thinking = false
default_permission_mode = "yolo"
telemetry = false
[providers.local]
type = "openai"
base_url = "http://10.214.74.113:8000/v1"
api_key = "dummy-local-no-auth"
[models.qwen-local]
provider = "local"
model = "qwen36_35b_256k"
max_context_size = 262144
capabilities = ["tool_use"]   # ★ qwen36 不在 kimi capability registry,必须显式开,否则不发 tools
[thinking]
mode = "off"                  # ★ Qwen3.6 thinking 必须关,否则 SSE idle timeout
```

## 验证结果

| 测试 | 脚本 | 结果 |
|---|---|---|
| **工具循环** read+edit+bash | `.poc/run.ts` | ✅ 三工具全调用、文件真改(VERSION=2)、turn completed、~14s、无 error、76 个 delta 流式正常 |
| **deny hook** 拦 sudo | `.poc/run-deny.ts` + `.poc/home/hooks/block-sudo.mjs` | ✅ PreToolUse hook exit code 2 拦下 `sudo whoami`(isError=true),模型把拦截原因转达用户 |
| **大输出压测** Write 150 行 | `.poc/run-stress.ts` | ✅ 150/150 行无截断、turn completed、0 error、118.5s。**模型流式生成 5.7KB tool-arg 约 97s,kimi 未被 SSE idle timeout 杀** |

## 关键结论(更新能力对照表)
- **本地 Qwen3.6/vLLM**:🟢 开箱(改 config.toml),tool_use 需显式声明 capabilities。
- **deny hook(sudo 硬拦)**:🟢 PreToolUse 外部脚本,契约(stdin JSON `tool_input.command` + exit 2 拦)与 pinvou3 现有 `deny_sensitive_paths.sh` **完全同构**,几乎可直接搬。
- **SSE/截断红线**:🟢→🟡 中等量级(~5.7KB arg / ~97s 流)扛住了。**极端量级(64KB+ / >240s)仍未压到**,Rust 集成后需用真实长 PPT 场景复测。
- **SDK 集成形态**:`KimiHarness.createSession()` + `session.onEvent()` + `session.prompt()`,事件(assistant.delta / tool.call.started / tool.result / turn.ended)与 pinvou3 现有 Tauri 事件一一对应。

## 红线/待办(进入里程碑 1 前)
- 🟡 **Node 运行时分发**:确认了 kimi 需 Node≥24.15(比系统 22 新)。.deb 要打包 Node sidecar,按 deb 依赖规矩声明。
- 🟡 **极端大输出**未压测。
- ⛔ **scratch 清理**:`_kimi_ref/`、`.tooling/`、`.poc/` 均为 scratch,勿提交。

## 补验(sidecar 侧,不碰 Rust)
| 测试 | 脚本 | 结果 |
|---|---|---|
| **per-turn 动态注入**(最深红线) | `.poc/run-perturn.ts` + `hooks/perturn-sudo-state.mjs` (UserPromptSubmit) | ✅ 同 session 内中途把 `sudo_state` 由 off 改 on,模型轮1 答"关闭"、轮2 答"开启",**当轮实时生效**。完整替代 pinvou3 per-turn system-reminder |
| **system prompt 注入** | `.poc/run-prompt.ts` + `work/AGENTS.md` | ✅ AGENTS.md 注入特征指令,模型自称"我是 pinvou3 助手"。prompt 定制端到端生效(完全替换底座 prompt 可走自定义 profile 的 systemPromptPath,精确接线留集成期) |

## 里程碑 1 步①(已做):Node 桥接脚本 + JSONL 协议
- `.poc/kimi-bridge.mjs`:sidecar 桥接脚本,stdin 收 JSONL 命令(create_session/prompt/cancel/close),stdout 吐映射后的事件。
- `.poc/test-bridge.mjs`:模拟 Rust 端 spawn 子进程 + JSONL stdio,**实测跑通** `ready → session_created → tool_start/tool_end ×3(Read/Edit/Bash) → done(completed)`,0 error。
- **协议契约**(Rust engine.rs 将照此实现):
  - 出 → `{"t":"delta","session","text"}` / `{"t":"tool_start","id","name","args"}` / `{"t":"tool_end","id","name","isError","output"}` / `{"t":"done","reason"}` / `{"t":"error","message"}` / `ready` / `session_created` / `closed`
  - 入 → `{"op":"create_session","reqId","workDir","model","permission","thinking"}` / `{"op":"prompt","session","text"}` / `{"op":"cancel","session"}` / `{"op":"close"}`
  - 事件映射对齐 pinvou3:delta→chat:delta、tool_start→chat:tool_start、tool_end→chat:tool_end、done→chat:done

## Node sidecar 分发方案分析(决策输入,不碰 Rust)
**问题**:kimi 要 Node ≥24.15,比 Debian/Ubuntu apt 仓库的 nodejs 新太多 → **不能用 `deb depends: nodejs`**。

**现有 .deb 先例**(`tauri.conf.json` + process.md):
- 系统工具在 apt 里 → `deb depends`(poppler-utils / tesseract-ocr / pandoc / p7zip-full / python3)/ `recommends`(libreoffice)
- apt 没有/要特定版的小二进制 → vendor 进 app 资源(whisper-cli ~1MB 先例)
- 大模型文件 → 首次使用时下载到 `~/.pinvou3/`(ggml-small 466MB 先例),不进 deb

**推荐方案:走 SEA 单二进制 + Tauri `externalBin`(不声明 nodejs 依赖)**
- kimi-code 自带 SEA 构建(`apps/kimi-code` 的 `build:native:sea`,用 Node SEA + `postject`),产出内嵌 Node 运行时的单一自包含二进制——这正是 README "单一二进制、无需 Node" 的实现。
- 我们把 `kimi-bridge.mjs` + 已 bundle 的 SDK(tsdown 已能打成 4.2MB 单 mjs)同样做成 SEA 单二进制(~60–90MB,内嵌 Node 24),作为 Tauri `externalBin` sidecar 随 .deb 发。
- 对齐 pinvou3 既有 whisper-cli vendoring 先例(只是更大),且与 kimi 自身分发哲学一致。
- deb 体积 +~70MB(libreoffice recommends 已是重依赖,可接受)。**不新增任何 apt 依赖**。
- 备选(更省体积但脆):`deb depends: nodejs` —— 因版本太新基本不可行,排除。

## 里程碑 1 步②(已做,增量最小):Rust 侧驱动链打通
- `.poc/rust-sidecar-smoke/`:独立最小 crate(tokio + serde_json),**不依赖 DeepSeek-TUI submodule**。
- **为何独立而非就地改 engine.rs**:本 worktree 的 DeepSeek-TUI submodule **未 checkout**(空目录),src-tauri crate 因 path 依赖缺失**无法编译**。故先用独立 crate 证明 Rust↔sidecar 协议(可逆、编译得动),就地改 engine.rs 留到 submodule 可用的环境。
- **实测**:cargo build 通过 + 运行:Rust spawn `node kimi-bridge.mjs` → JSONL 发 create_session/prompt → 读事件映射 `chat:delta`/`chat:tool_start`/`chat:tool_end`/`chat:done` → 单 session 一轮 Read+Edit+Bash 全调用、文件改 `FLAG=after`、done=completed。✅
- **与 engine.rs 的同构**:spawn 子进程 ↔ `spawn_engine`;写 `{"op":"prompt"}` ↔ `Op::SendMessage`;读 JSONL + 映射 ↔ `spawn_event_forwarder` 的 `app.emit("chat:*")`(映射已逐条对齐 engine.rs:273-357)。

## 里程碑 1 步②+(已做):真实 crate 内集成
前置已解决:`git submodule update --init DeepSeek-TUI` 在 worktree 拉好,`cargo check --lib` 通过(53s)。

**已落地改动(pinvou3-app 真实源码,worktree 分支)**:
- 新增 `pinvou3-app/src-tauri/src/kimi_sidecar.rs`:`KimiSidecar`(spawn 子进程 + create_session/prompt/cancel/close)+ `SidecarEvent` enum + `map_event`(bridge JSONL → chat:* 语义)。mpsc 事件流,形态对齐 `EngineHandle`/`rx_event`,供 `spawn_event_forwarder` 复用。
- `src/lib.rs`:注册 `pub mod kimi_sidecar;`
- `src/engine.rs`:`pub use crate::kimi_sidecar::{KimiSidecar, SidecarEvent}` + 迁移说明注释(加法引入,未动现有 EngineHandle 路径,保证 build 绿)
- 新增 `src/bin/kimi_sidecar_smoke.rs`:真实 crate 内冒烟

**实测**:`cargo build --bin kimi_sidecar_smoke` 通过(增量 5.82s);运行:真实 crate 内 spawn sidecar → 单 session 一轮 Read/Edit/Bash → 事件映射 `chat:delta/tool_start/tool_end/done` → done=completed、文件 `FLAG=after`。✅

## 里程碑 1 步②++(已做):config 生成 + bridge 接入
- `kimi_sidecar.rs` 加 `render_kimi_config(model, base_url, api_key)`:pinvou3 本地 vLLM 设置 → kimi config.toml(openai provider / tool_use / thinking off)。
- `bridge/mod.rs` 加 `kimi_config_toml()`:与 `build_dt_config` 同源(env `DEEPSEEK_BASE_URL`/`DEEPSEEK_API_KEY` 优先 + 本地 fallback)。**触及 8 耦合文件之一**。
- 2 单测(`config_has_local_vllm_and_thinking_off` / `map_event_covers_chat_events`)`cargo test --lib` 通过,全 lib(88 既有测试)仍编译绿。

### 当前 Rust 侧改动总览(worktree 分支,未 commit)
| 文件 | 状态 |
|---|---|
| `src/kimi_sidecar.rs`(新) | KimiSidecar 驱动 + SidecarEvent + render_kimi_config + 2 单测 |
| `src/bin/kimi_sidecar_smoke.rs`(新) | 真实 crate 冒烟,实跑通 |
| `src/lib.rs` | 注册 mod |
| `src/engine.rs` | re-export 新驱动 + 注释 |
| `src/bridge/mod.rs` | kimi_config_toml() |
| build/test | ✅ cargo build + cargo test 全绿 |

## 里程碑 1 步③(进行中):核心切换 — 子步骤 a(已做)
- `engine.rs` 加 `spawn_sidecar_forwarder(app, rx: mpsc<SidecarEvent>, session_id)`:消费 SidecarEvent → `app.emit("chat:*")`,payload 与 `spawn_event_forwarder` 逐条对齐(delta/tool_start/tool_end/done)。加法,`cargo check` 绿(2.56s)。
- 这是 AppEngine 切换的"事件→前端"那一半。暂不含 plan_snapshot/phase/approval/user_input。

### 子步骤 a2(已做):降爆炸半径
- `engine.rs` 加 `AppEngine::shutdown()` 封装 `Op::Shutdown`;`engine_pool.rs::evict` 改调它(不再裸 `entry.engine.handle.send`),移除 engine_pool 的 `Op` import。**触及 engine_pool.rs(第 5/8 文件)**,`cargo check` 绿。
- 意义:外部对 `.handle` 的直接依赖再减一处,后续 `AppEngine` 换后端 enum 时改动面更小。

### 子步骤 b(已做):AppEngine 双后端切换 ✅ build 绿 + 84 单测过
- `AppEngine` 字段:`handle: Option<EngineHandle>` + `sidecar: Option<KimiSidecar>` + `kimi_session: Option<String>`。
- `spawn_for_session`:`PINVOU3_KIMI_SIDECAR` 开 → `spawn_kimi_for_session`(写 config.toml→spawn sidecar→握手 Ready/create_session/SessionCreated→`spawn_sidecar_forwarder`);默认仍 Native。
- 方法路由:`send_user_message`(Kimi→`sidecar.prompt`)、`shutdown`(Kimi→`sidecar.close`)、`sync_session`(Kimi→no-op,session 由 sidecar 自管);`edit_last_turn`/`compact`/`submit_user_input`/`cancel` Kimi 侧先 stub(后续增量)。
- 开发期路径走 env(`PINVOU3_KIMI_NODE`/`_BRIDGE`/`_HOME`)+ worktree 默认。
- `tests/l1_dialog_harness.rs` 3 处 `.handle` → `.as_ref().unwrap()`(适配 Option)。
- **`cargo test --lib` 84 passed / 0 failed**(含 forkguard + 新 kimi_sidecar 2 测),Native 路径零回归。

### 子步骤 d(部分已做):补 Kimi 侧 ops ✅ build 绿
- `compact`:bridge.mjs 加 `compact` 命令(→`session.compact()`)+ `KimiSidecar::compact` + `AppEngine::compact_now` 路由。
- `cancel`:`AppEngine::cancel_current`(同步入口)Kimi 走 spawned task 调 `sidecar.cancel`。
- 仍 stub:`edit_last_turn`(kimi 无直接等价,候选 `session.steer`/重 prompt)、`submit_user_input`(需双向协议:新增 host→sidecar 的 respond op + sidecar→host 的 question 事件,对接 kimi AskUserQuestion)。

### 子步骤 e-2(已做):安全 hook 接进 Kimi spawn ✅ 86 单测过
- `super_permission.rs` 加 `pretooluse_sudo_block_script()`(关闭态拦 sudo,python3 解析 tool_input.command,exit 2)。
- `kimi_sidecar.rs` 加 `render_hooks_toml(hooks_dir)`(PreToolUse + UserPromptSubmit 两段 `[[hooks]]`)。
- `engine.rs::spawn_kimi_for_session`:写 `home/hooks/{sudo-block.sh, userprompt-state.sh, sp-on.json, sp-off.json}`,config.toml 追加 hooks 段。→ **关闭态 sudo 硬拦 + per-turn 状态注入在 Kimi 后端实际生效**(机制 PoC 已验)。
- `cargo test --lib` **86 passed / 0 failed**。

### 子步骤 e-1(已做):super_permission per-turn → UserPromptSubmit hook 产物 ✅
- `super_permission.rs`:抽 `REMINDER_ON`/`REMINDER_OFF` 常量;加 `userprompt_state_json(enabled)`(serde_json 安全转义出 hook 输出 JSON)+ `userprompt_hook_script(on_json, off_json)`(bash:实时判 sudoers 文件→cat 对应 JSON,零 shell 转义风险)。+2 单测,`cargo test` 5 过。**触及 super_permission.rs(第 7/8 文件)**。
- 待接:spawn 时把 on/off JSON + 脚本写到 kimi home/hooks,并在 `kimi_config_toml()` 追加 `[[hooks]] UserPromptSubmit` + `PreToolUse`(sudo block)。

### 子步骤 e-3(已做):pinvou3 prompt 注入 Kimi(bundle.rs)✅
- `bridge/bundle.rs` 加 `kimi_agents_md()`(= INSTRUCTIONS_MD)。
- `spawn_kimi_for_session` 写 `{workdir}/AGENTS.md` → kimi `${KIMI_AGENTS_MD}` 自动合并进 system prompt(PoC 验过)。完全替换 base prompt 需自定义 profile(后续);当前叠加注入拿到 pinvou3 行为引导。`cargo check` 绿。

### 子步骤 e-4(已做):mode_state.rs AppMode→kimi 映射 ✅ 87 单测
- `bridge/mode_state.rs` 加 `SerializableMode::to_kimi() → (permission, plan_mode)`(Plan→("yolo",true) / Yolo→("yolo",false))+ 测试。

### Rust 侧迁移状态盘点(8 耦合文件)
| 文件 | 状态 |
|---|---|
| engine.rs | ✅ 双后端 AppEngine + kimi spawn 握手 + forwarder + ops 路由 |
| engine_pool.rs | ✅ shutdown 抽象去 `.handle` 裸访问 |
| bridge/mod.rs | ✅ `kimi_config_toml()` |
| bridge/bundle.rs | ✅ `kimi_agents_md()`(AGENTS.md 注入) |
| bridge/mode_state.rs | ✅ `to_kimi()` mode 映射 |
| super_permission.rs | ✅ per-turn + sudo-block hook 产物 |
| **commands.rs** | ⚪ **无需改**:用的是共享数据类型(Message/SavedSession/SkillRegistry),走 engine_pool(后端无关) |
| **bin/dump_system_prompt.rs** | ⚪ **非阻塞**:Native-only 调试 bin,Kimi 不适用,仍编译绿 |
| (新)kimi_sidecar.rs | ✅ 驱动 + 事件映射 + config + hooks 生成 + 单测 |

**结论**:核心功能迁移 Rust 侧已完成(6 文件实质改动 + 新模块,87 测试绿,Native 零回归)。余下 commands/dump_bin 不需为 Kimi 改。

### 子步骤 d-续(已做):user_input 双向协议 ✅ 87 测试 + 桥接回归过
- `kimi-bridge.mjs`:`session.setQuestionHandler` → 发 `{"t":"question"}` 事件 + 挂起 Promise;新增 `{"op":"answer",...}` 解挂。
- `KimiSidecar`:`SidecarEvent::Question` + `answer(request_id, answers)` + map_event。
- `engine.rs` forwarder:`Question` → `chat:user_input_required`(与 Native 同形状 `{session_id,id,questions}`)。
- `submit_user_input`:`UserInputResponse{answers:[{id,value}]}` → kimi `QuestionAnswers{id:value}` → `sidecar.answer`;`cancel_user_input` → null 解挂。
- 桥接回归 `test-bridge.mjs` 仍绿(question handler 未破坏正常流)。

### Kimi 后端 op 完成度:除 edit_last_turn 外全通
prompt / compact / cancel / shutdown / **user_input(双向)** / 事件流 / sudo 拦截 / per-turn 注入 / prompt 注入 / config 生成 全部 ✅。仅 `edit_last_turn` 仍 stub(kimi 无直接等价,候选 `session.steer`)。

### 仍未做
- `edit_last_turn`(kimi 无直接等价,需 steer/重 prompt 设计)。
- **GUI 实测**:`PINVOU3_KIMI_SIDECAR=1` 启动 Tauri app 跑真实对话——无头环境做不了,**需有显示环境**。重点验:user_input 气泡渲染(kimi QuestionItem 格式 vs 前端期望)、plan_mode、AGENTS.md 注入效果。

### 剩余(后续增量)
- c. Kimi 后端 runtime 验证(带 `PINVOU3_KIMI_SIDECAR=1` 跑 Tauri app 实测一轮 GUI 对话)。
- d-续. `edit_last_turn` + `user_input` 双向协议。
- e-续. 把 e-1 产物接进 config/spawn;`bridge/bundle.rs` system prompt → 自定义 profile;`bin/dump_system_prompt.rs` 适配/退役。
- d. sidecar/bridge 补 `submit_user_input`/`approve_tool_call`/`edit_last_turn`/`compact` 对等(kimi 侧:AskUserQuestion / permission / session.compact),需扩 kimi-bridge.mjs 协议。
- e. `super_permission.rs` per-turn → UserPromptSubmit hook(已 PoC 验过机制)。

## (原)下一步:把 `AppEngine` 内部从 `EngineHandle` 切到 `KimiSidecar`(`send_user_message` 写 JSONL、`spawn_event_forwarder` 改消费 `SidecarEvent` 并 `app.emit`),再 `engine_pool.rs`(每 session 一 sidecar)/ `bridge/mod.rs`(EngineConfig→config.toml 生成)/ `super_permission.rs`(per-turn → UserPromptSubmit hook)跟上。这一步会改动现有路径、可能短暂破坏 build,属下一个增量。

---
## (历史)原下一步说明:里程碑 1 步②③④,改 Rust 源码 = 架构岔路
② 改 `engine.rs`:`spawn_engine` → spawn `node kimi-bridge.mjs` 子进程 + 读写 stdio;`Op`→JSONL 命令;JSONL 事件→`Event`/Tauri emit。
③ `engine_pool.rs`(每 session 一进程或一进程多 session)/ `bridge/mod.rs`(EngineConfig→config.toml)跟上。
④ 编译 + 前端冒烟。
> 步②起开始修改 pinvou3-app 的 Rust 源码(不可逆程度上升),按 goal「关键架构岔路找我确认」停下等指示。
