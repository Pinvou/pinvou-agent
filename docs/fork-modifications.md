# CodeWhale Fork 修改清单

> 本文是 pinvou3 对 CodeWhale 底座 fork 的单一现状清单。
> 基线、主题边界、守护指纹和每次 sync 结论都以本文与 `docs/fork-policy.md` 为准。
> English: [`docs/fork-modifications.en.md`](fork-modifications.en.md)

## 0. 当前状态（2026-08-03 · v0.9.0）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.0`，commit `d167c07c96282411956ea7f35ddb8227afa1402f` |
| 公开固定基线 | tag `pinvou-v0.9.0-r3`，commit `9a31dcdfad71172ab4fdf00a4d8bd106cfaa47da` |
| fork 分支 | `Pinvou/CodeWhale` 的 `pinvou3-clean`，当前 head `9a31dcdfad71` |
| 组织方式 | **6 个长期主题 commit + 8 个行为补充/维护 commit + 3 个公开基线/安全维护 commit**；公开历史从上游 `v0.9.0` 重放，不复用私有 fork SHA |
| drift | 对 `v0.9.0`：**+4632 / -611，58 文件** |
| 守护 | `scripts/fork-guard.sh`：v0.9 主题指纹 + 宿主 ShellManager 观察器与生命周期指纹 + submodule/app `forkguard_` 行为测试 |
| app 状态 | `pinvou3-tauri` 主库编译通过，lib test target 可完整编译；macOS 适配保留在父仓平台抽象中，不增加 fork drift |

### 软上限评估

当前 drift 超过 `fork-policy.md` 的 1500 行软上限，已触发强制评估，结论是**本轮不为了数字继续拆删**：

- 约 743 行是定时任务的稳定会话键、运行链接、保留/级联删除和小时锚点，拆到 app 会复制底座持久化模型。
- 约 665 行是宿主编排与结构化/文件产出完成闸，属于子 agent 生命周期的原子语义，放在 app 无法可靠判断真实完成。
- 约 633 行是 prompt/context/skill 来源密封；这是 pinvou3 单一 bundle 来源和静态前缀稳定性的产品边界。
- 约 965 行是工具面、写入上限和命令安全，其中包含结果式 golden 测试，不能只留字符串指纹。
- Shell 实时输出不再形成 fork drift：app 复用 v0.9.0 已公开的 session 级 `ShellManager`，以非破坏性完整快照计算前台/后台增量，并由主仓测试守护 UTF-8 边界和拥塞合并。
- 已移除 v0.8.65 时代被 v0.9.0 harvest 的 MCP env、Windows 子进程、旧 request_user_input 路由、旧 subagent 自建 mailbox/温度/工具面等补丁；没有机械照搬整包旧 fork。

后续减量优先级：先推动 T6 宿主接口和 T2 通用安全修复上游化，再评估 T4 automation 通用部分；T3/T5 的 pinvou3 产品语义继续留 fork。

## 1. 六个长期 fork 主题 + 一个父仓主题（T7）

### T1 `embed`：v0.9.0 宿主 library facade

- **commit**：`052784220 feat(embed): 建立 v0.9.0 宿主 library facade`
- **核心文件**：`crates/tui/src/lib.rs`
- **内容**：为上游 bin-first crate 建立 library 入口，公开 pinvou3-app 实际使用的 engine、tools、automation、route、prompt、MCP 等模块。
- **边界**：只做模块暴露，不在 facade 重写 Engine / ToolRegistry / Session。
- **维护风险**：上游增删模块时，`lib.rs` 不会自动跟随，必须以 app 编译和 lib tests 发现漂移。

### T2 `fork`：工具面、文件写入与执行安全

- **commit**：`092e3c885 feat(fork): 收敛工具面、文件写入与执行安全`；`cb93e0f44 feat(fork): append_file 输出 inline unified diff`。
- **核心文件**：`tools/pinvou3_blocklist.rs`、`core/engine/tool_catalog.rs`、`tools/file.rs`、`core/engine/dispatch.rs`、`tools/shell.rs`、`core/engine/turn_loop.rs`、`command_safety.rs`、审批策略相关文件。
- **内容**：
  - pinvou3 native 工具黑名单与 deferred activator 结果式 golden。
  - `write_file` / `append_file` 64KB 单次内容上限和缺字段修复提示。
  - `append_file` 与 `write_file` 一样输出 inline unified diff(尾部 context + 追加行,字节摘要保留在末尾);已存文件超过 512KB(`APPEND_FILE_INLINE_DIFF_LIMIT_BYTES`)或非 UTF-8 时回退 `[diff omitted]` / 纯字节摘要。
  - `disallowed_tools` 支持 `*` 前缀规则。
  - Dangerous 命令在所有模式阻断；审批缓存、workflow plan 审批保持 fail-closed。
- **为什么留 fork**：工具面是 pinvou3 产品定位；append_file 与大产物引导耦合。Shell 展示能力已经移到 app，不再作为本主题的 fork-distinct 代码维护。
- **守护**：`forkguard_blocklist_golden`、`forkguard_yolo_no_deferred_activator_first_class`、`forkguard_append_file_emits_inline_diff`、文件上限测试和命令安全测试。

### T3 `fork`：提示词密封与 context / skill 单一来源

- **commit**：`87aef33ce feat(fork): 密封提示词并收敛上下文与技能来源`
- **核心文件**：`prompts.rs`、`project_context.rs`、`project_context_cache.rs`、`skills/mod.rs`、`tools/skill.rs`、`working_set.rs`。
- **内容**：
  - 静态 prompt composer 由 app 接管，默认层和运行时策略按 composer gate 密封。
  - 不再扫描仓库 constitution / AGENTS / CLAUDE 等外部 project context；pinvou3 只用 app 注入的 inline instructions，项目规则（`AGENTS.md`）由 app 侧受限注入（仅绑项目代码会话，root→cwd 各层；不越过用户家目录——project_root 与 home 同函数归一化后比较，Windows 上边界真实生效；symlink/非普通文件拒读；归一化失败 fail-closed；见 `docs/code-native-agent.md` §8.5）。
  - skill 来源收敛到 `~/.pinvou3/bundle/skills`，并保留市场停用过滤。
  - 内部 `<system-reminder>` 不参与 Working Set 路径提取。
  - instructions/用户记忆 fragment 沿用 100KB 指令上限，避免被 v0.9 WorldState 默认 4KB 静默截断。
- **为什么留 fork**：这是 pinvou3 的单一知识/指令来源和 prefix-cache 稳定性约束，上游通用 CLI 不能默认采用。
- **守护**：static composer 前后字节测试、inline context 测试、skill union/停用测试、Working Set 两条回归；sync 后仍需跑 `dump_system_prompt` 前后 diff。

### T4 `fork`：定时任务执行与历史生命周期

- **commit**：`06da3dfac feat(fork): 收敛定时任务执行与历史生命周期`
- **核心文件**：`automation_manager.rs`、`task_manager.rs`、`tools/automation.rs`、`core/engine/turn_loop.rs`。
- **内容**：
  - automation 透传选定 model，并用 automation id 作为稳定 `conversation_key`。
  - task schema v4，兼容读取 v3；运行中的 thread/turn 链接及时落盘。
  - HOURLY 规则按创建时间形成稳定锚点；旧规则即使未显式写 `BYMINUTE`，跳过漏跑后也不漂移到 App 重启分钟。
  - 调度器对关机/休眠期间错过的时段默认直接跳到下一未来时段，不补跑历史；同一 automation 存在 queued/running run 时跳过当前时段，避免重叠和积压。
  - 只清理终态 run/task，保留活动运行并级联删除对应 artifacts。
  - `force_prompt` 工具不能被通用 auto-approve 绕过。
- **为什么留 fork**：pinvou3 的 hidden scheduled session 依赖稳定会话身份和历史级联语义；只放 app 会与 TaskManager 持久化竞态。
- **守护**：automation 回归、`worker_receives_persisted_conversation_key`、运行链接、保留、终态删除、小时锚点、漏跑跳过和同任务不重叠测试。

### T5 `fork`：宿主编排、工作流完成闸与可取消登录

- **commit**：`5e5513151 feat(fork): 适配宿主编排与工作流完成闸`
- **维护提交**：
  - `b6991463b fix: 修复宿主工具仅在 Plan 模式注册`
  - `c7dbe0353 fix(workflow): 修复工作流子代理写入权限丢失 (#19)`
  - `b2e3a83bf chore(ci): 补齐 Pinvou fork 公开分支门禁`
- **核心文件**：`core/engine.rs`、`core/engine/tool_setup.rs`、`core/engine/tests.rs`、`core/ops.rs`、`core/events.rs`、`tools/subagent/mod.rs`、`tools/subagent/tests.rs`、`mcp/oauth.rs`。
- **内容**：
  - `EngineConfig.extra_tools`、hard `tool_whitelist`、会话 reasoning effort 和动态 disallowed tools；宿主注入工具在 Plan / Agent / Yolo 三种 turn registry 中统一注册，避免非 Plan 分支提前返回时丢失 `kb_search`。
  - `SpawnSubAgent` 接受 role、allowed tools、max steps、output schema、expects-file-output。
  - `Custom` 工作流子 Agent 的显式工具白名单同时恢复父级允许的粗粒度能力；声明 `write_file`/`append_file` 后可以真实落盘，未声明工具仍由白名单拒绝，且只读父级不能被越权提升。
  - 合成 `submit_output` 工具；递归校验有限 JSON schema，只允许声明的安全相对路径落盘；最多 3 次催交后 fail-closed。
  - 文件产出型角色必须有成功的 `write_file` / `append_file` 才能完成；重试耗尽时把最后一次工具错误带入失败信封，宿主日志无需读取私有转录即可显示具体原因。
  - `AgentComplete` 携带 role/failed；宿主可 `CancelSubAgents`，批量取消所有 live agent；可靠 mailbox 在嵌套 Agent 的 `Started` 前发送 `ChildSpawned`，并在显式取消、超时自动回收、协作取消和中断路径于 abort/退出前发送一次与真实结果一致的终态。子任务若异常返回非终态 `Running`，会在发布任何终态之前统一归一为 `Failed`，确保 mailbox、manager 和 worker ledger 一致。中断会保留可重新派发的 checkpoint，但不保留原位恢复的 live task；宿主无需依赖可丢弃 UI 事件即可恢复父子谱系并收敛资源归属。
  - OAuth 登录支持 CancellationToken，返回前先 drop in-flight flow 和回调监听。
  - standalone exec 路径显式补齐 `tool_whitelist`、`reasoning_effort`、`extra_tools`，保证 fork 作为独立项目时全目标可编译。
- **为什么留 fork**：这些是宿主工作流的真实完成/取消语义，app 仅观察事件无法无竞态重建。
- **守护**：宿主额外工具全模式注册、结构化 schema/安全路径、Custom 显式写工具真实落盘、文件产出失败保留并脱敏最后工具错误、父子权限不可越权、批量取消、OAuth drop-before-return 回归。

### T6 `embed`：宿主路由、预算与 shared automation 接口

- **commit**：`dadbca0d6 feat(embed): 补齐宿主路由与定时任务接口`
- **核心文件**：`route_runtime.rs`、`route_budget.rs`、`automation_manager.rs`
- **内容**：
  - 向 embedder 公开字段私有的 `ResolvedRuntimeRoute` 和 `resolve_runtime_route`；只暴露非敏感 model receipt。
  - 新增 `resolve_runtime_route_with_limits`，让宿主把具体部署的 context/output facts 附到同一 route receipt，不要求任意 wire alias 进入静态模型目录。
  - route 已显式声明 output 时，以“请求意图与 route 上限的较小值”为准；只有 route 未声明时才使用模型名推断的 4K 兼容 fallback。
  - 公开 `reconcile_run_statuses_shared`，宿主不再持 automation mutex 等待 task-manager I/O。
- **为什么单列**：这是可上游化的宿主 API 面，与 pinvou3 私有产品逻辑解耦；后续最优先提上游。

### 公开基线与安全维护（不新增产品主题）

- `3e8e6e7f chore(security): 配置 fork 全历史密钥扫描`
- `23d4c9b5 docs(fork): 记录 Pinvou 公开基线`
- `070f4413 chore(fork): 清理内部项目注释`

这三个提交只增加公开 fork 的说明、全历史 Gitleaks 门禁与精确测试夹具白名单，并移除一处内部项目代号注释；不改变 T1–T6 的产品行为。`cb93e0f44` 是 T2 的行为补充，`749cafc9e` 至 `9a31dcdfa` 是 T5 的可靠 mailbox 生命周期补充，均不新增长期主题。父仓只固定到稳定标签 `pinvou-v0.9.0-r3`，不跟随维护分支漂移。

### T7 macOS 平台目标支持 (2026-07)

**不在 fork 内做任何改动** —— fork 已通过 `crates/tui/Cargo.toml:112` 声明
`[target.'cfg(target_os = "macos")'.dependencies]`，macos 平台原生编译通过。
pinvou3-app 在父仓内通过 `pinvou3-app/src-tauri/src/os/macos/` 实现平台抽象，
包括 `macos_path` / `macos_system` / `macos_dependency` / `macos_memory` /
`macos_permission` / `macos_update` 六个模块（对齐 `os/linux/` 与 `os/windows/`）。
其他 Mac 专属补丁（`pet_window.rs` collectionBehavior、`detach.rs` CoreGraphics 全局光标轮询、
`voice_asr.rs` engine_binary_name 等）也全在父仓内，
通过 `#[cfg(target_os = "macos")]` 闸住，对 Linux/Windows 主线零影响。

#### 语音识别：已剥离 SenseVoice，改用系统 Speech 框架（2026-07 二期）

- macOS 不再打包 SenseVoice 引擎，改用 `objc2-speech` crate 调系统 `SFSpeechURLRecognitionRequest`。
- 运行时查询当前语言对应识别器的 `supportsOnDeviceRecognition`：支持时要求端上识别，否则由 Apple Speech 在线服务处理；不能按 Apple Silicon / Intel 架构预判。
- Linux 仍保留 `sense-voice-main`，本节不再登记 macOS provenance。
- fork-guard 仍无需追踪（语音非 fork 改动）。

**无需新增 fork-guard 指纹**（fork 代码零改动）。`./scripts/fork-guard.sh --fast`
在 macOS 适配 PR 上仍然通过。

## 2. v0.9.0 已 harvest / 不再重打

以下能力在 v0.9.0 已有等价或更完整实现，本轮没有继续作为 fork-distinct patch：

- MCP stdio env placeholder、Windows GUI 子进程抑窗、Windows killed shell reader 收尾。
- MCP resources/templates 按实际集合 gate。
- 上游 subagent mailbox、父子消息、request_user_input、模型路由、运行时 worker 记录、温度和通用工具面。
- pwd/workspace 移出静态 system、手动 cache warmup、PDF panic guard、skills union 基础设施。
- 上游已完成的通用 provider、Hook v2、路由预算、InstructionSource、context compaction 基础能力。

原则：只保留 pinvou3 与上游**行为差异**，不因旧文档里曾有编号就继续复制代码。

## 3. app 对 v0.9.0 的适配

- `SendMessage` 与 `CompactContext` 都携带同一 provider 配置解析出的 route receipt 和 compaction policy，不再传裸 `model/provider`。
- `SavedModel` 持久化部署级 `context_window_tokens` / `max_output_tokens`；设置页可配置任意 OpenAI-compatible 推理引擎。实时 probe 与声明窗口取较小值，Compact、emergency budget 和实际请求共用同一 route profile。
- 默认本地 `qwen36_35b_256k` 迁移为 256K/24K；未知 vLLM alias 使用 128K/24K 保守档案，其他引擎不按模型名猜能力。
- scheduled profile 切模型时同时替换 route 与 compaction，避免模型/窗口不一致。
- `EngineConfig` 对 v0.9.0 新增 `fleet_roster`、`moraine_fallback`、`terminal_chrome_enabled` 显式评估后透传默认值。
- automation boot/run/reconcile 使用 shared API，不跨 await 持 manager mutex。
- `SyncSession` 显式设置 mode；新增 Event/Mailbox 字段用 `..` 向前兼容。
- `Cargo.lock` 随 submodule v0.9.0 依赖图更新。
- `dump_system_prompt` 兼容 v0.9 的 `SystemPrompt::Blocks`；内置中文技能名改用安全命令名 `visual-design`，避免被归一化成无意义的 `skill`。
- app 的 `ShellOutputMonitor` 复用 session 级共享 `ShellManager`：按命令和工具调用绑定新任务，以非消费式完整快照计算 stdout/stderr 增量，合并慢轮询期间的全部未发送内容，并在后台终态补齐去重后的输出尾部。
- `ShellOutputMonitor` 对运行中快照尾部的临时 `U+FFFD` 延迟一轮发送，避免 UTF-8 中文字符跨 reader chunk 时被永久写成替换符；底座的权限、安全分析、执行与 wait 游标保持原实现。
- `EnginePool` 以 session 级生命周期记录协调自然完成、异常断流、主动回收和缺失 Engine 的取消路径，确保同一 turn 只产生一次权威终态。
- app 层 `TurnShellTaskRegistry` 在 Engine 提交前以 RAII guard 建立 root-turn scope，并在权威 `TurnStarted` 到达后绑定真实 turn id；工具 task id 与可靠 mailbox 的 Agent 父子谱系共同记录归属，基线差集只兜底 ownerless root job，避免把旧 detached Agent 延迟创建的任务误判为当前 turn。`Completed`、`Failed`、`Cancelled` 和不保留 live task 的 `Interrupted` 都会结束 Agent 对 scope 的占用。主停止先把 scope 标记为取消，再取消模型生成；独立 blocking worker 清理当前及后续到达的所属任务，失败项有限重试并以有界 tombstone 留存。Shell 清理失败通过结构化终态标志交给前端三语展示，不覆盖 Engine 的 Interrupted 语义。正常完成和后台任务卡的单任务取消语义保持不变。

## 4. 守护与验收

### 快速 gate

```bash
./scripts/fork-guard.sh --fast
```

指纹按 T1–T6 主题组织。新增/修改 fork patch 必须在同一个父仓 PR 同步更新：代码、`forkguard_` 行为测试、指纹和本文。

### 必跑验证

```bash
# 底座
cargo check -p codewhale-tui --lib
cargo test -p codewhale-tui forkguard_ --lib -- --test-threads=1
cargo test -p codewhale-tui automation_manager::tests --lib -- --test-threads=1

# app
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib --no-run
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml scheduled_executor::tests --lib -- --test-threads=1

# prompt 静态层
cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml --bin dump_system_prompt --features dev-tools > /tmp/post-sync-prompt.txt
diff /tmp/pre-sync-prompt.txt /tmp/post-sync-prompt.txt
```

本地 `/tmp` 空间不足时，显式把 `TMPDIR` 和 `CARGO_TARGET_DIR` 指到项目盘；不要用清理用户目录解决构建问题。

## 5. Sync 历史

### v0.9.0 r3 Agent mailbox 生命周期（2026-08-03）

- `Pinvou/CodeWhale#6` 以 rebase merge 合入 `pinvou3-clean`，发布不可变标签 `pinvou-v0.9.0-r3@9a31dcdf`。
- 嵌套 Agent 在实际 `Started` 前可靠发布父子谱系；取消、中断、超时回收和自然完成只发布一次与权威结果一致的终态，并释放 manager 持有的 mailbox/task handle。
- 合入前修复了排队 Agent 被提前及重复标记 `Started` 的问题：创建路径只发布 `ChildSpawned`，执行路径在取得 launch permit 后发布唯一 `Started`。
- 验证：CodeWhale workspace 全目标检查、Pinvou fork 回归、40 项 `forkguard_`、终态/超时/竞态/launch-gate 定向测试、clippy、DCO、Gitleaks 与 contribution gate。

### LLM-facing 提示词文本审查修复（2026-08-02）

- fork 侧改动经 `Pinvou/CodeWhale#4`（分支 `fix/prompt-text-audit`，基线 `pinvou-v0.9.0-r2`）提交；合入发布新标签后，父仓再更新 gitlink 对齐。
- 纯提示词文本变更，零代码行为变化；fork 指纹锚点全部保留，`forkguard_blocklist_golden` 等结果式 golden 不受影响；两处既有测试断言随文本修正同步更新（`rlm.rs` 的错误示例 `SOURCE`→`content`、`prompts.rs` 的 edit_file 不再引用被 pinvou3 工具面隐藏的 apply_patch）。
- fork-distinct 文件中的文本修复（shell.rs T2、automation.rs T4、subagent T5、skills/mod.rs T3、tool_catalog.rs T2）只校正描述与既有实现的一致性，不新增 fork 语义，无需新增指纹。
- 生产层改动限于 bundle 内 wecomcli-doc / wecomcli-smartsheet / lark-task 三个 SKILL.md 的自相矛盾与笔误修复，以及 `runtime_bundle/platform/mod.rs` 两条过时注释；instructions.md、base.md、kb_tool.rs、ima.rs 审查后无需改动。
- 上游通用的文本-实现漂移（如 grep_files 虚假的 `.gitignore` 声明、agents/message 虚构的 "natural resume"、delegate 技能错误的 model_strength 默认值、wecomcli-doc 矛盾）已列入回馈上游候选。
- 验证：`cargo check -p codewhale-tui`、`cargo test -p codewhale-tui forkguard_ --lib` 及 tools/prompts/skills 模块测试、`cargo check`（pinvou3-tauri lib）、`./scripts/fork-guard.sh --fast`。

### v0.9.0 r2 append_file inline diff（2026-07-30）

- `Pinvou/CodeWhale#2` 以 squash commit `cb93e0f4466d60e306252ed08bbbe214f2def752` 合入 `pinvou3-clean`，并发布不可变标签 `pinvou-v0.9.0-r2`。
- `append_file` 输出 inline unified diff 和尾部字节摘要；旧文件超过 512KB 或非 UTF-8 时安全回退。
- CodeWhale CI 已通过全 workspace/all-targets 编译、77 项 `tools::file` 定向测试、`forkguard_` 回归、格式、DCO、Gitleaks 与贡献门禁。

### v0.9.0 公开固定基线（2026-07-24）

- 在 `Pinvou/CodeWhale` 保留上游完整 MIT 历史与作者归属，并发布不可变标签 `pinvou-v0.9.0-r1`。
- 对 5,448 个历史 commit 执行 Gitleaks；只精确放行两个上游公开测试夹具，扫描结果为 0 个泄漏。
- GitHub CI 已通过全 workspace/all-targets 编译、`forkguard_` 回归、格式、DCO、Gitleaks 与 CodeQL。
- 父仓 gitlink 固定到标签解引用 commit；`.gitmodules` 不再跟随 `pinvou3-clean` 浮动分支。

### v0.9.0 clean re-fork（2026-07-17）

- 没有在旧 `pinvou3-clean` 上直接 merge；从上游 tag `v0.9.0` 新建隔离 worktree 和 `codex/sync-v0.9.0`。
- 逐项判定“上游已有 / app 可解决 / 仍需 fork”，按耦合边界重打为 6 个主题 commit。
- 最终 drift `+3260/-539，53 文件`；超过软上限后完成强制评估，保留项和后续减量顺序见 §0。
- 编译迁移从 14 个 app API 错误收敛到 0；lib test target 完整编译通过。
- 已验证：route runtime/route budget 显式 limits 回归、128K/256K Compact 结果式回归、OpenAI-compatible 自定义引擎 profile、Tools golden、automation 18 项、structured output 2 项、OAuth 1 项、批量取消 1 项、app bridge/定时策略、scheduled executor 10 项、级联删除 1 项。
- `dump_system_prompt` 已对 v0.8.65 基线逐行 diff：主指令、技能目录和用户记忆保持完整；预期差异仅为 v0.9 WorldState marker 与 route 元数据。

### v0.8.65 及更早

旧版曾按 C1–C12、P、AUTO-lite、R、W 维护，并经历三次 clean re-fork。其详细编号只用于历史考古，不再是当前 fork 结构；需要追溯时查看 v0.8.65 基线前的 Git 历史。保留下来的经验只有：

- 大版本 sync 优先 clean re-fork，不把冲突解决结果直接当长期历史。
- prompt 必须做 dump 前后 diff；工具面必须做结果式 golden；全量 lib tests 能抓到非 `forkguard_` 的上游回归。
- 同名上游 API 不能按名字判定 harvest，必须逐字段比较语义。
