# DeepSeek-TUI Fork 修改清单

> 本文是 pinvou3 对 DeepSeek-TUI（CodeWhale）底座 fork 的单一现状清单。
> 基线、主题边界、守护指纹和每次 sync 结论都以本文与 `docs/fork-policy.md` 为准。

## 0. 当前状态（2026-07-23 · v0.9.0）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.0`，commit `d167c07c96282411956ea7f35ddb8227afa1402f` |
| fork 分支 | `pinvou3-clean`，当前 head `c32bb73f4605` |
| 组织方式 | **6 个长期主题 commit**，按耦合边界维护；不再保留 C1–C12 / W1–W13 批量编号 |
| drift | 对 `v0.9.0`：**+3668 / -558，53 文件** |
| 守护 | `scripts/fork-guard.sh`：v0.9 主题指纹 + 宿主 ShellManager 观察器与生命周期指纹 + submodule/app `forkguard_` 行为测试 |
| app 状态 | `pinvou3-tauri` 主库编译通过，lib test target 可完整编译 |

### 软上限评估

当前 drift 超过 `fork-policy.md` 的 1500 行软上限，已触发强制评估，结论是**本轮不为了数字继续拆删**：

- 约 743 行是定时任务的稳定会话键、运行链接、保留/级联删除和小时锚点，拆到 app 会复制底座持久化模型。
- 约 665 行是宿主编排与结构化/文件产出完成闸，属于子 agent 生命周期的原子语义，放在 app 无法可靠判断真实完成。
- 约 633 行是 prompt/context/skill 来源密封；这是 pinvou3 单一 bundle 来源和静态前缀稳定性的产品边界。
- 约 965 行是工具面、写入上限和命令安全，其中包含结果式 golden 测试，不能只留字符串指纹。
- Shell 实时输出不再形成 fork drift：app 复用 `c32bb73f` 已公开的 session 级 `ShellManager`，以非破坏性完整快照计算前台/后台增量，并由主仓测试守护 UTF-8 边界和拥塞合并。
- 已移除 v0.8.65 时代被 v0.9.0 harvest 的 MCP env、Windows 子进程、旧 request_user_input 路由、旧 subagent 自建 mailbox/温度/工具面等补丁；没有机械照搬整包旧 fork。

后续减量优先级：先推动 T6 宿主接口和 T2 通用安全修复上游化，再评估 T4 automation 通用部分；T3/T5 的 pinvou3 产品语义继续留 fork。

## 1. 六个长期主题

### T1 `embed`：v0.9.0 宿主 library facade

- **commit**：`99a84a092 feat(embed): 建立 v0.9.0 宿主 library facade`
- **核心文件**：`crates/tui/src/lib.rs`
- **内容**：为上游 bin-first crate 建立 library 入口，公开 pinvou3-app 实际使用的 engine、tools、automation、route、prompt、MCP 等模块。
- **边界**：只做模块暴露，不在 facade 重写 Engine / ToolRegistry / Session。
- **维护风险**：上游增删模块时，`lib.rs` 不会自动跟随，必须以 app 编译和 lib tests 发现漂移。

### T2 `fork`：工具面、文件写入与执行安全

- **commit**：`9e136cb11 feat(fork): 收敛工具面、文件写入与执行安全`。
- **核心文件**：`tools/pinvou3_blocklist.rs`、`core/engine/tool_catalog.rs`、`tools/file.rs`、`core/engine/dispatch.rs`、`tools/shell.rs`、`core/engine/turn_loop.rs`、`command_safety.rs`、审批策略相关文件。
- **内容**：
  - pinvou3 native 工具黑名单与 deferred activator 结果式 golden。
  - `write_file` / `append_file` 64KB 单次内容上限和缺字段修复提示。
  - `disallowed_tools` 支持 `*` 前缀规则。
  - Dangerous 命令在所有模式阻断；审批缓存、workflow plan 审批保持 fail-closed。
- **为什么留 fork**：工具面是 pinvou3 产品定位；append_file 与大产物引导耦合。Shell 展示能力已经移到 app，不再作为本主题的 fork-distinct 代码维护。
- **守护**：`forkguard_blocklist_golden`、`forkguard_yolo_no_deferred_activator_first_class`、文件上限测试和命令安全测试。

### T3 `fork`：提示词密封与 context / skill 单一来源

- **commit**：`14ae5a151 feat(fork): 密封提示词并收敛上下文与技能来源`
- **核心文件**：`prompts.rs`、`project_context.rs`、`project_context_cache.rs`、`skills/mod.rs`、`tools/skill.rs`、`working_set.rs`。
- **内容**：
  - 静态 prompt composer 由 app 接管，默认层和运行时策略按 composer gate 密封。
  - 不再扫描仓库 constitution / AGENTS / CLAUDE 等外部 project context；pinvou3 只用 app 注入的 inline instructions。
  - skill 来源收敛到 `~/.pinvou3/bundle/skills`，并保留市场停用过滤。
  - 内部 `<system-reminder>` 不参与 Working Set 路径提取。
  - instructions/用户记忆 fragment 沿用 100KB 指令上限，避免被 v0.9 WorldState 默认 4KB 静默截断。
- **为什么留 fork**：这是 pinvou3 的单一知识/指令来源和 prefix-cache 稳定性约束，上游通用 CLI 不能默认采用。
- **守护**：static composer 前后字节测试、inline context 测试、skill union/停用测试、Working Set 两条回归；sync 后仍需跑 `dump_system_prompt` 前后 diff。

### T4 `fork`：定时任务执行与历史生命周期

- **commit**：`27293bd3f feat(fork): 收敛定时任务执行与历史生命周期`
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

- **commit**：`add065123 feat(fork): 适配宿主编排与工作流完成闸`
- **核心文件**：`core/engine.rs`、`core/engine/tool_setup.rs`、`core/engine/tests.rs`、`core/ops.rs`、`core/events.rs`、`tools/subagent/mod.rs`、`tools/subagent/tests.rs`、`mcp/oauth.rs`。
- **内容**：
  - `EngineConfig.extra_tools`、hard `tool_whitelist`、会话 reasoning effort 和动态 disallowed tools；宿主注入工具在 Plan / Agent / Yolo 三种 turn registry 中统一注册，避免非 Plan 分支提前返回时丢失 `kb_search`。
  - `SpawnSubAgent` 接受 role、allowed tools、max steps、output schema、expects-file-output。
  - `Custom` 工作流子 Agent 的显式工具白名单同时恢复父级允许的粗粒度能力；声明 `write_file`/`append_file` 后可以真实落盘，未声明工具仍由白名单拒绝，且只读父级不能被越权提升。
  - 合成 `submit_output` 工具；递归校验有限 JSON schema，只允许声明的安全相对路径落盘；最多 3 次催交后 fail-closed。
  - 文件产出型角色必须有成功的 `write_file` / `append_file` 才能完成；重试耗尽时把最后一次工具错误带入失败信封，宿主日志无需读取私有转录即可显示具体原因。
  - `AgentComplete` 携带 role/failed；宿主可 `CancelSubAgents`，批量取消所有 live agent。
  - OAuth 登录支持 CancellationToken，返回前先 drop in-flight flow 和回调监听。
- **为什么留 fork**：这些是宿主工作流的真实完成/取消语义，app 仅观察事件无法无竞态重建。
- **守护**：宿主额外工具全模式注册、结构化 schema/安全路径、Custom 显式写工具真实落盘、文件产出失败保留并脱敏最后工具错误、父子权限不可越权、批量取消、OAuth drop-before-return 回归。

### T6 `embed`：宿主路由、预算与 shared automation 接口

- **commit**：`4cff0b9e6 feat(embed): 补齐宿主路由与定时任务接口`
- **核心文件**：`route_runtime.rs`、`route_budget.rs`、`automation_manager.rs`
- **内容**：
  - 向 embedder 公开字段私有的 `ResolvedRuntimeRoute` 和 `resolve_runtime_route`；只暴露非敏感 model receipt。
  - 新增 `resolve_runtime_route_with_limits`，让宿主把具体部署的 context/output facts 附到同一 route receipt，不要求任意 wire alias 进入静态模型目录。
  - route 已显式声明 output 时，以“请求意图与 route 上限的较小值”为准；只有 route 未声明时才使用模型名推断的 4K 兼容 fallback。
  - 公开 `reconcile_run_statuses_shared`，宿主不再持 automation mutex 等待 task-manager I/O。
- **为什么单列**：这是可上游化的宿主 API 面，与 pinvou3 私有产品逻辑解耦；后续最优先提上游。

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
cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml --bin dump_system_prompt > /tmp/post-sync-prompt.txt
diff /tmp/pre-sync-prompt.txt /tmp/post-sync-prompt.txt
```

本地 `/tmp` 空间不足时，显式把 `TMPDIR` 和 `CARGO_TARGET_DIR` 指到项目盘；不要用清理用户目录解决构建问题。

## 5. Sync 历史

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
