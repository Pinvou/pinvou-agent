# kimi 底座迁移 · 移交文档

> 分支 `worktree-kimi-base-migration` · 状态:Rust + Node 桥接迁移**功能完成**(除 `edit_last_turn`),
> build 绿 / 87 单测过 / Native 零回归。**仅差 GUI 实测**(需显示环境)。
> 配套:可行性分析 `docs/archived/底座替换评估-kimi-code.md`;PoC + 增量记录 `docs/archived/kimi-迁移-里程碑0-PoC结果.md`。

---

## 0. 运行时验证已覆盖到哪(headless 最大化)
`cargo run --bin kimi_integration_smoke` ✅ 通过:用 app 的**真实生成器**产出 config.toml + hooks
(sudo-block / userprompt-state)+ AGENTS.md,spawn sidecar 跑一轮 → Read+Edit+Bash 全调用、
文件真改、done=completed、0 error。**证明 `spawn_kimi_for_session` 写盘的全部产物运行时有效**。
唯一未覆盖:Tauri `app.emit`→前端渲染(需显示环境,见 §2 GUI 实测)。
其它 bin:`kimi_sidecar_smoke`(KimiSidecar 直驱)、`_kimi_ref/.poc/{run,run-deny,run-perturn,run-prompt}.ts`(PoC)。

## 1. 一句话现状
pinvou3-app 现在是**双后端**:默认走原 DeepSeek-TUI(`EngineHandle`),设 `PINVOU3_KIMI_SIDECAR=1`
则改走 kimi Node sidecar(`@moonshot-ai/kimi-code-sdk`)。前端 / 事件协议不变,Native 行为零回归。

## 2. 怎么开 kimi 后端 + GUI 实测(待你在桌面环境做)
前置(开发期,worktree 内已就绪):
- Node 24:`.tooling/node24/`(系统 Node 22 太旧,kimi 要 ≥24.15)
- kimi SDK 已构建:`_kimi_ref/packages/node-sdk/dist/index.mjs`
- 桥接脚本:`_kimi_ref/.poc/kimi-bridge.mjs`
- 本地 vLLM:`http://10.214.74.113:8000/v1` 模型 `qwen36_35b_256k`(实测在线)

跑:
```bash
export PINVOU3_KIMI_SIDECAR=1
export DEEPSEEK_BASE_URL=http://10.214.74.113:8000/v1   # 不设则默认 127.0.0.1:8000
# 开发期 sidecar 路径默认指向本 worktree,可用 env 覆盖:
# PINVOU3_KIMI_NODE / PINVOU3_KIMI_BRIDGE / PINVOU3_KIMI_HOME
./pinvou3-app/run-dev.sh
```
**重点验**:① 发消息→工具循环→流式回显;② request_user_input 气泡渲染
(已在 forwarder 做 kimi `QuestionItem`→前端 schema 转换、前端零改动,GUI 确认渲染 + 选项回填 +
答案是否被 kimi 正确接收);③ 关闭态发 sudo 被拦 + 开关切换下一轮生效;④ AGENTS.md 注入后
模型是否带 pinvou3 行为;⑤ 大产物长流不截断。

## 3. 改动清单(都在 worktree 分支,未 commit)
### 新增
- `pinvou3-app/src-tauri/src/kimi_sidecar.rs` —— sidecar 驱动:`KimiSidecar`
  (spawn/prompt/compact/cancel/answer/close)+ `SidecarEvent` + `map_event` +
  `render_kimi_config` + `render_hooks_toml` + 单测。
- `pinvou3-app/src-tauri/src/bin/kimi_sidecar_smoke.rs` —— 真实 crate 冒烟(已跑通)。
### 改动(8 耦合文件)
| 文件 | 改动 |
|---|---|
| `engine.rs` | 双后端 `AppEngine`(`handle:Option` + `sidecar:Option` + `kimi_session`);`spawn_kimi_for_session`(写 config/hooks/AGENTS.md + 握手);`spawn_sidecar_forwarder`(SidecarEvent→chat:*);所有 op 路由;env 辅助 |
| `engine_pool.rs` | `evict` 改用 `AppEngine::shutdown()`,去 `.handle` 裸访问 |
| `bridge/mod.rs` | `kimi_config_toml()` |
| `bridge/bundle.rs` | `kimi_agents_md()` |
| `bridge/mode_state.rs` | `SerializableMode::to_kimi()` |
| `super_permission.rs` | `userprompt_state_json` / `userprompt_hook_script` / `pretooluse_sudo_block_script` + 测试 |
| `commands.rs` | **无需改**(共享数据类型,走后端无关 engine_pool) |
| `bin/dump_system_prompt.rs` | **无需改**(Native-only 调试 bin) |
| `lib.rs` | 注册 `pub mod kimi_sidecar` |
| `tests/l1_dialog_harness.rs` | 适配 `handle: Option`(3 处) |
### scratch(勿提交)
`_kimi_ref/`(kimi 源码 + `.poc/` 桥接&PoC)、`.tooling/node24/`、`.poc/rust-sidecar-smoke/`。

## 4. JSONL 桥接协议(Rust ↔ kimi-bridge.mjs)
入(Rust→bridge):`create_session` / `prompt` / `compact` / `cancel` / `answer`(requestId+answers)/ `close`
出(bridge→Rust):`ready` / `session_created` / `delta` / `tool_start` / `tool_end` / `done` /
`question`(id+questions)/ `error` / `closed`。
事件→前端映射:delta→`chat:delta`、tool_start→`chat:tool_start`、tool_end→`chat:tool_end`、
done→`chat:done`、question→`chat:user_input_required`。

## 5. 剩余 TODO(按优先级)
1. **GUI 实测**(见 §2)—— 唯一真正的验证缺口,过了才算端到端可用。
2. **edit_last_turn**:kimi 无直接等价,候选 `session.steer` 或"截断+重 prompt"。当前 stub(返回 Ok + log)。
3. **去开发期硬编码路径**:`engine.rs` 里 `KIMI_REF` 常量是 worktree 绝对路径,产品化要换成
   resources 相对路径 + SEA 单二进制(见 §6)。
4. **完整替换 base prompt**:当前 AGENTS.md 是"叠加注入"。要完全替换 kimi base prompt,
   走自定义 agent profile 的 `systemPromptPath`(kimi 机制已查实,接线未做)。
5. **plan_mode 接线**:`to_kimi()` 已给 `(permission, plan_mode)`,但 `create_session` 目前
   硬编码 `("yolo","off")`;要把 mode 的 plan_mode 透传(create_session 加 planMode 参数)。
6. **弱模型 SSE/截断稳健性**:PoC 中等量级已扛住;GUI 长 PPT 场景要复测,不稳则改 kimi TS / 提 PR
   (别在 Rust 层硬补 —— 见可行性报告红线)。

## 6. 产品化(分发)
- kimi 要 Node ≥24.15,比 apt 新 → **不能 `deb depends: nodejs`**。
- 方案:用 kimi 自带 SEA 构建(`apps/kimi-code` 的 `build:native:sea`,Node SEA + postject)把
  桥接脚本 + SDK 打成内嵌 Node 的单二进制(~70MB),作 Tauri `externalBin` sidecar。
  对齐 pinvou3 现有 whisper-cli vendoring 先例,**不新增 apt 依赖**。详见可行性报告 §分发。

## 7. 回退
全部改动在 worktree 分支且未 commit;丢弃 worktree 即回到原 DeepSeek-TUI 底座。
即便保留改动,不设 `PINVOU3_KIMI_SIDECAR` 时默认仍走 Native,行为不变。
