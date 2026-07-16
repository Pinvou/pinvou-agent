# DeepSeek-TUI Fork 修改清单

> pinvou3 对 `DeepSeek-TUI`(已 rebrand `CodeWhale`)底座所有 fork 修改的**单一现状清单**。
> 用途:① sync 后查 patch 存活 ② 交接 / onboarding ③ 上游 PR 定位改动点。
> 配套:`scripts/fork-guard.sh`(指纹 + 回归测试守卫)、`docs/fork-policy.md`(维护策略 + sync 流程 + PR 状态)。
>
> **当前基线**:submodule 指向 `pinvou3-clean@8832469c`(fork PR #16 merge commit,包含 fork PR #14/#15)+ W13 宿主取消后台 Agent(+75/−0,4 文件)。基线包含 scheduled-lite/C12/P2 后续补丁(+701/−17,7 文件)、小时调度时间锚点(+109/−10,1 文件;见 §4)与 W13。`.gitmodules` 追踪 `pinvou3-clean`。

---

## 0. 当前状态速览(2026-07-16 · v0.8.65)

| 项 | 值 |
|---|---|
| submodule 分支 | 当前基线 **`pinvou3-clean@8832469c`**(fork PR #16 merge commit,包含 fork PR #14/#15;`.gitmodules` 追踪 `pinvou3-clean`);durable scheduler 重实现留在 `codex/scheduled-tasks` 备查;备份 `backup/v0.8.65-merge-result`(merge 树)、`backup/pre-reclean-trial-tip`(旧 fork tip `6b3059da`)、`backup/pre-v0.8.65-sync`(旧远程 pinvou3-clean `4518f845`) |
| fork drift | `8073aa9b` 基线原 drift **+7070/−4930,54 文件**(vs v0.8.65)+ scheduled-lite/C12/P2 后续 **+701/−17,7 文件** + 小时调度锚点 **+109/−10,1 文件** + W13 宿主取消后台 Agent **+75/−0,4 文件**。durable scheduler 的 +13744/−5646 已撤回,仍保持轻 fork(fork-policy §0) |
| 历史 | v0.8.65 clean re-fork 的 C1–C12 + R + W 主题,以及后续 blocklist/compact/cancellation 修复;2026-07-13 fork PR #9 撤自动 warmup;同日 durable scheduled runtime(原 fork PR #8)以 AUTO-lite 最小补丁重做并撤回重实现(见 §4);2026-07-16 增加 P2 可取消 OAuth 登录、小时调度起点与 W13 宿主批量取消后台 Agent；同日向 upstream v0.8.68 主线提 [#4379](https://github.com/Hmbown/CodeWhale/pull/4379) / [#4381](https://github.com/Hmbown/CodeWhale/pull/4381) |
| LLM 暴露 native 工具 | **20 个**(全量注册 − 黑名单;原 23,2026-07-03 纯办公定位再砍 git_status/git_diff/diagnostics)。**tool_search 已禁用**(⚠️2026-07-03 修:v0.8.65 折叠单名后门控名与双旧名对不上一度漏注入,已补裸名,详见 C2)。MCP `mcp_pinvou_present_artifact` 另接,共 21 入口 |
| fork-guard | 指纹 + 回归测试(`scripts/fork-guard.sh`;**v0.8.65 撤 P pwd-move 2 条**=上游已 harvest;+MKT skill 停用 3 条;**AUTO 重型指纹 18 条撤、AUTO-lite 现为 16 条**);AUTO-lite 定向回归覆盖 automation model/conversation key、小时调度锚点、schema v4/v3 兼容、运行链接、终态保留、engine force_prompt 与 app scheduled Yolo 链路 |
| system prompt | dump 逐字节稳定;per-turn `<runtime_prompt>` tag + goal continuation 均已 gate |
| v0.8.65 决策 | **W 全保 fork**(三省六部 harness 命脉,不换上游单 agent);**P 已被上游 harvest**;**决策③**:token-budget scope-gate **不港**(fork 用步数上限)、`MAX_SPAWN_DEPTH_CEILING` **用上游 8**;**skills 收窄到只 `~/.pinvou3/bundle/skills`**(去 `.agents/skills`) |

---

## 1. fork 结构(C1–C12 + P1–P2 + AUTO-lite + R + W 逻辑主题)

> 逻辑分组,对应主题 commit。看某文件 fork-distinct 改动:`git -C DeepSeek-TUI diff v0.8.60..HEAD -- <file>`。
> 冲突易出血优先级(sync review 顺序):**prompts.rs(C5+C7) > turn_loop.rs(C7) > subagent/mod.rs(W) > tool_catalog.rs(C2) > project_context.rs(C5)**。

### C1 `lib` library facade
- **文件**:`crates/tui/src/lib.rs`(整文件——上游只有 `main.rs`,无 lib target)
- **改动**:`pub mod` 暴露内部模块 + `#[cfg(test)] pub mod test_support`,让 pinvou3-app 以 `deepseek_tui::*` as-library 调用 + `cargo test --lib` 能跑
- **⚠️ 维护**:上游每加/删模块要**手动同步 `pub mod`**(上游无 lib.rs,3-way 不会自动改)。孤儿 `pub mod` 会编译错(v0.8.51 `cycle_manager` / v0.8.60 `prompt_persist` 删除即此坑);`acp_server` 依赖 bin 专属符号不能进 lib
- 上游 PR:❌ pinvou3 专用

### C2 `tools` blocklist 工具门控
- **文件**:`tools/pinvou3_blocklist.rs`(新建,**81 条黑名单**)、`core/engine/tool_catalog.rs`、`tools/registry.rs`、`tools/mod.rs`
- **哲学**:上游(v0.8.47 起)是 **allowlist**;pinvou3 相反——**显示全部、只隐藏黑名单**,给 Qwen3.6 精简到 **20 工具**(2026-07-03 纯办公定位:git_status/git_diff/diagnostics 也隐藏)
- **关键**:`pinvou3_should_defer_native_tool(name, mode, always_load)` **mode-aware**:Yolo 只 defer 黑名单。`request_user_input` 跨所有 mode 硬保留(否则 GUI 不出选择气泡);`image_analyze` 放出(需 bridge 开 `VisionModel` feature);`checklist_*` 有意可见。`PINVOU3_BLOCKLIST_OVERRIDE` env 供 L1 harness 解锁
- **⚠️ tool_search 防御**:blocklist 是「defer 不删除」,工具仍在 catalog。上游 `tool_search`(`ensure_advanced_tooling` 注入)能让模型**搜索激活被 blocklist 的 deferred 工具**→ 击穿门控。修法:`tool_search` 进 blocklist + **注入处 gate**(`is_pinvou3_hidden(TOOL_SEARCH_NAME)` 为真不注入)→ catalog 根本不含
- **⚠️ v0.8.65 单名折叠 + 2026-07-03 修**:上游 v0.8.57 是双工具 `tool_search_tool_regex/bm25`,**v0.8.65 折叠成单名 `tool_search`**(门控 `TOOL_SEARCH_NAME="tool_search"`)。sync 后 blocklist 仍只有双旧名 → `is_pinvou3_hidden("tool_search")` 为 false → **门控失效、tool_search 漏注入且首轮可见**(`defer_loading=false`),模型可反向激活全部 deferred 工具。**端到端实测坐实**(改回归断言为精确名跑 `ensure_advanced_tooling(Yolo)`,catalog=[…,`tool_search`])。守护为何漏抓:fork-guard 指纹查废弃双旧名(空防、恒在)、回归测试断言查 `starts_with("tool_search_tool")`(单名恒真通过)。**修**:补裸名 `tool_search`(保留双旧名前向兼容)+ 测试断言改精确名 `== "tool_search"` + fork-guard 指纹改查裸名。详见 `docs/工具表精简方案.md` §8.2
- **测试**:`pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default`、`forkguard_tool_search_not_injected_*`(断言已改精确名);**golden 守护(2026-07-03,结果式)** `forkguard_blocklist_golden`(blocklist 精确名单 == golden)+ `forkguard_yolo_no_deferred_activator_first_class`(注入层 active 集精确相等)——堵 sync 改名/新增/折叠漂移,负向验证过(删 tool_search 三测全 fail);验收清单 3.2/3.4 已从"grep 预判"改为"跑 golden"
- 上游 PR:❌ 哲学相反

### P1 `mcp` list_mcp_resources 空转 gate(2026-07-03)
- **文件**:`crates/tui/src/mcp.rs`(`to_api_tools`)
- **改动**:上游 `list_mcp_resources` / `list_mcp_resource_templates` 注入条件是 `!self.config.servers.is_empty()`——只要注册任何 MCP server 就注入,与是否真暴露 resources 无关。pinvou3 的 MCP server 全 tools-only(present_artifact/gongwen/weather/iwencai 的 `resources/list` 均返回 `-32601`),这两个元工具**永久空转**、纯占工具槽+token、52 session 零调用。改为按 `all_resources()` / `all_resource_templates()` 非空**分别 gate**,与下方 `mcp_read_resource`(`!resources.is_empty()`)一致
- **测试**:`mcp::tests` 全过(P1 不破坏);fork-guard 指纹 `P1 list_mcp_resources 按 resources 非空 gate`
- 上游 PR:🟢 **已在 upstream v0.8.68 主线等价存在**(2026-07-16 复核):按 `all_resources()` / `all_resource_templates()` 分别 gate，故不重复提 PR；下次 sync 按文件级 diff harvest 后撤 P1 指纹。

### P2 `mcp/oauth` 可取消的 OAuth 登录(2026-07-16)
- **文件**:`crates/tui/src/mcp/oauth.rs`;宿主接线在 `pinvou3-app/src-tauri/src/commands.rs` 与工具商店前端
- **改动**:保留原 `perform_oauth_login_for_server` API,新增接收 `CancellationToken` 的 `perform_oauth_login_for_server_with_cancel`。取消会 drop 正在执行的 OAuth future,从而触发 `CallbackServerGuard::drop` / `Server::unblock`,并在返回前停止旧回调监听。宿主按 `tool_id + request_id` 编排:新授权先取消并等待旧授权退出;显式取消也等待底座 future 退出;取消早于 start 注册的竞态由 pending cancellation 兜住。前端写配置阶段不可取消,进入浏览器授权阶段才开放取消;UI 超时先终止后端再读取真实 token,解决超时/回调同边界的乱序状态。
- **测试**:`forkguard_cancellable_oauth_drops_in_flight_flow_before_returning`;`forkguard_marketplace_oauth_{replacement_waits_for_previous_flow,cancel_waits_until_flow_finishes,remembers_cancel_before_register}`;工具商店浏览器旅程覆盖安装阶段不可取消、请求 ID 对齐与取消后待授权态
- 上游 PR:🟡 [#4379](https://github.com/Hmbown/CodeWhale/pull/4379) **OPEN**(2026-07-16):仅提底座可取消 API + future drop 回归；宿主按 `tool_id + request_id` 的替换/显式取消编排和 UI 留 pinvou3。关联上游问题 [#4380](https://github.com/Hmbown/CodeWhale/issues/4380)。

### AUTO-lite `automation` host executor 最小契约(2026-07-14,替代原 durable scheduler)
- **文件**:`crates/tui/src/automation_manager.rs`、`crates/tui/src/task_manager.rs`、`crates/tui/src/tools/automation.rs`、`crates/tui/src/core/engine/turn_loop.rs`、`crates/tui/src/core/engine/tests.rs`
- **改动**:① `ExecutionTask` 增加只读 getters(id/conversation_key/prompt/model/workspace/mode_label/allow_shell/trust_mode/auto_approve),host executor 无需 pub 字段即可消费;② `TaskExecutionEvent` 增加 `ThreadCreated { thread_id }` + 处理分支——会话身份先于 `ThreadLinked` 上报,发送失败/中断的 run 也能链上会话(**无 ACK**:事件进 channel 即返回,落盘发生在 manager 消费事件时,不保证先于 engine send);③ `reconcile_run_statuses` 运行中仅链接变化也触发 `save_run`——否则任务运行中历史打不开,要等状态变化才连带保存;④ automation 增加可选 `model`(record/create/update/tool schema 全链路)并传播到标准 task;⑤ registry 工具路径接入基线已有的 `approval_force_prompt` 机制(新增 `registered_tool_approval_force_prompt`):hook ask 与 `rlm_eval` 等不可旁路审批不能被 auto-approve 绕过;⑥ `TaskRecord` 保留 `idempotency_key: Option<String>` 惰性字段,并新增可选的稳定 `conversation_key`;automation 入队时以 `automation.id` 赋值,让每次 execution task id 独立，同时由 app 用 automation id 派生任务工作间。schema 升至 v4,缺少新字段的 v3 task 继续可读;⑦ 为宿主提供轻量终态保留接口：按 automation 选出超过预算的终态 Run，并删除对应终态 Task；queued/running 永不进入候选，不引入归档、索引或 journal;⑧ `HOURLY` 可选 `BYHOUR/BYMINUTE` 作为稳定时间锚点，首次运行及后续推进始终相对任务创建时间的本地日期连续计算；不带锚点的旧规则保持原行为。
- **明确不承诺"不丢、不重"(轻量取舍)**:scheduler_tick 先 `enqueue_run_task`(task 已持久化)后 `save_run`,两步之间崩溃 → 下个 tick 该时槽会再入队,可能**重复执行一次**;`ThreadCreated`/事件链路无持久化 ACK。不做 enqueue journal/执行去重幂等键/reporter ACK——那正是撤回的 durable scheduler 机制。办公场景(晨报/汇总类)重复跑一次可接受;若未来要硬保证,按 `codex/scheduled-tasks` 分支拣回对应主题。
- **审批产品决策(2026-07-14,恒 YOLO)**:定时任务与交互对话一致,**无人值守默认自动批准**(开箱即用、前端不暴露任何任务级权限设置)。落点在 **app 层**:每次执行都重新读取普通聊天的全局 Shell 配置(默认开启),并固定 `trust_mode=true`、`auto_approve=true`;create 写入同一组值,update 不接受任务级权限字段。基座 `DEFAULT_AUTOMATION_AUTO_APPROVE` 保持基线 `true` 不动。安全兜底 = force_prompt(⑤,rlm_eval/hook ask 无人值守自动**拒绝**而非挂起)+ C4 Dangerous 命令 YOLO 也 BLOCK。
- **理由**:PINVOU 定时任务复用底座 `AutomationManager`/`TaskManager`/`spawn_scheduler`,fork 只补 host 消费接口与会话身份耐久点。原 durable scheduler(sidecar index/retention guard/prune/journal,+13744/−5646)对办公场景(小时级起步、单机少量任务)过度设计,已整体撤回——run/task 走底座原生持久化。
- **测试**:`automation_enqueue_uses_default_and_explicit_task_settings`(model + conversation key 透传)、`worker_receives_persisted_conversation_key`、v3 task 兼容、`hourly_rrule_uses_clock_time_as_a_stable_anchor` / `hourly_rrule_accepts_minute_only_anchor`、`forkguard_running_run_link_persists_before_terminal_state`(运行中链接落盘,**负向验证过**:还原 bug 即 fail)、`forkguard_retention_keeps_latest_terminal_runs_and_all_active_runs`、`forkguard_deletes_only_terminal_task_records_and_artifacts`、`create_schema_exposes_rrule`(schema 含 model)、`rlm_eval_required_approval_ignores_generic_auto_approve` / `generic_required_tools_keep_auto_approve_behavior`(force_prompt 只对不可旁路工具生效);app 侧覆盖同一 automation 多次运行使用独立对话并共享任务工作间、缺模型失败仍保留预创建会话、全局 Shell 运行时刷新与恒 YOLO;UI 冒烟覆盖小时起点编辑与错误提示关闭
- **上游 PR**:🟢 `approval_force_prompt` / `rlm_eval` 不可旁路审批已在 upstream v0.8.68 主线等价覆盖，**不重复提**；getters/model 透传/`ThreadCreated` 仍与 pinvou3 宿主会话契约耦合，暂不随附。小时锚点已拆为独立 [#4381](https://github.com/Hmbown/CodeWhale/pull/4381) **OPEN**，避免混入 host executor 改动。

### C3 `tools` append_file + 大产物保护
- **文件**:`tools/file.rs`、`core/engine/dispatch.rs`、`client/chat.rs`、`tools/registry.rs`(`with_file_tools`)、`tui/approval.rs`、`tui/widgets/tool_card.rs`、`tools/approval_cache.rs`
- **改动**:`append_file` 工具(上游没有)+ content **64KB 硬上限** + `truncated_args_hint`(流截断缺字段→引导分块)+ SSE idle-timeout 遥测 + undo 快照纳入
- **理由**:本地慢 vLLM 大产物(PPT/长文档)>240s idle timeout 流截断;`write_file` 写 skeleton(≤8KB)→ `append_file` 追加 chunk(≤16KB)
- **测试**:`truncated_args_hint_*`、`test_{write,append}_file_rejects_oversized_content`
- 上游 PR:❌ 与专属 `append_file` 深耦合,去耦后无落点

### C4 `safety` careful 安全 hook
- **文件**:`tools/shell.rs`、`command_safety.rs`
- **改动**:Dangerous 命令(`rm -rf /`·`~`·`$HOME`·`/*`、fork bomb)在 **YOLO 也 BLOCKED**(上游 YOLO 跳过)。"YOLO 只是免审批弹窗,不等于允许毁灭性命令"
- **为何留**(2026-06-15 评估):pinvou3 默认 YOLO + 弱模型 + workspace=$HOME,这是唯一拦 `rm -rf ~` 的网(deny hook 只覆盖敏感路径/sudo,upstream careful 在 YOLO 跳过)。可移 app deny hook 但会削弱(裸字符串匹配 vs `analyze_command` 解包裹 + 丢 `safety_level` 红卡 metadata),不值得
- **测试**:`forkguard` careful shell YOLO-block 指纹 + `command_safety` Dangerous 测试(含 `bash -lc 'rm -rf /'` 包裹)
- 上游 PR:❌ 安全模型专用 ·(C4-a 多行逐行已被上游 `split_command_segments` harvest)

### C5 `prompt` GUI prompt / context / skills
- **文件**:`project_context.rs`、`project_context_cache.rs`、`skills/mod.rs`、`commands/groups/skills/skills.rs`、`tools/skill.rs`、`prompts.rs`(与 C7 共此文件)
- **project_context**:`PROJECT_CONTEXT_FILES`/`GLOBAL_PATHS` **砍空**(workspace=$HOME GUI 助手,不读其他 AI 工具配置);`load_repo_constitution_block` **短路**;`generate_ephemeral_context` **砍空返 None**(防 $HOME 树扫成 overview 注入 prompt,仅采上游函数名让调用点编译)
- **skills**:扫描路径**只** `~/.pinvou3/bundle/skills`(私有:技能市场装的技能 + bundle 内置;= `EngineConfig.skills_dir`)(原 10 路径 #41 收窄;union 接线已被上游 harvest)。**bundle/skills 必须进 `skills_directories`**:`load_skill` 工具用 `discover_in_workspace`(只走 `skills_directories`、不 union `EngineConfig.skills_dir`),不放进去就会"prompt catalogue 列了、load_skill 却扫不到→报 not found"(技能市场真机实测)。**2026-06-29 决策**:`~/.agents/skills` 也砍掉——pinvou3 技能统一走技能市场落 bundle/skills,不再扫全局 .agents/skills(连带 `agents_global_skills_dir()` 仅留作 helper)
- **skill 停用开关(技能市场 toggle,2026-06-25)**:`skills/mod.rs` 进程级 `DISABLED_SKILLS` + `set_disabled_skills()`/`is_skill_disabled()`,`render_skills_block` 跳过停用 skill(从 `## Skills` catalogue 隐藏)、`tools/skill.rs` `load_skill` 对停用 skill 当 not-found。镜像 connector `disabled_connectors`「全局持久」语义,但 skill 是 render 收口自由函数→进程级集即可(无需 Op/EngineConfig 字段)。消费方=app `bridge/skill_marketplace.rs`(`disabled_skills.json` + `model_skill_names` id→落盘名)+ `commands::set/get_disabled_skills` + 启动 `refresh_disabled_skills`。不上游(依赖 pinvou3 市场)
- **测试**:`forkguard_skills_dir_unions_*`、`forkguard_disabled_skill_hidden_from_catalogue`;project_context_cache / skills 多路径上游测试 `#[ignore]`
- 上游 PR:skills union → [#2737](https://github.com/Hmbown/CodeWhale/pull/2737) CLOSED(上游已 harvest);constitution 短路 ❌ 专用

### C6 `chore` 零碎适配
- **文件**:`llm_client/mod.rs`、`core/engine/lsp_hooks.rs`、`lsp/mod.rs`、`hooks.rs`(append_file 入 file_write 类)、`core/turn.rs`、`tui/app.rs`、`.gitignore`
- **改动**:编译 / 接线层零碎适配(各 1-5 行)

### C7 `prompt` static composer hook(密封静态层)
- **文件**:`prompts.rs`、`core/engine/turn_loop.rs`
- **机制**:`set_static_prompt_composer_override(Box<dyn Fn(&StaticPromptCtx)->String>)`——embedder 一个 hook **全量接管编译期静态文案**。`StaticPromptCtx` 是 pinvou3 **宽版**(mode/approval_mode/model_id/allow_shell/default_layers)
- **密封范围**:装了 composer 则后置 append 全 gate 掉——**ContextMgmt + COMPACT_TEMPLATE + Runtime Policy Reference**(`static_prompt_composer().is_none()`)+ **per-turn `<runtime_prompt>` tag**(`static_prompt_composer_installed()`)
- **理由**:逐块 `set_*_override` 防不住"上游新增块漏进 prompt";composer 把静态层密封,上游升级新 doctrine 进不了 pinvou3 prompt
- **⚠️ 同名 API 语义分叉**:上游独立实现**窄版** composer(`StaticPromptCtx{model_id, personality, default_layers}`)。决议:删上游窄版、保 pinvou3 宽 ctx;采上游 mode-independent 管线但在 `apply_static_prompt_composer` 内以常量 Yolo/Auto 构造宽 ctx。(v0.8.60 merge:调用点 `effective_static_prompt_composer()` 对齐 pinvou3 访问器 `static_prompt_composer()`)
- **测试**:submodule `forkguard_static_prompt_composer_*`;app `forkguard_static_composer_*`
- 上游 PR:[#2786](https://github.com/Hmbown/CodeWhale/pull/2786) CLOSED(上游窄版,语义不同);pinvou3 宽版保 fork

### ~~P `prefix-cache` pwd/workspace 移出静态 system~~ → **v0.8.65 已被上游 harvest**(撤指纹,见 §2.2)
> pwd/workspace 从静态 `## Environment` 移出 → per-turn `<turn_meta>`,让 static system 跨 session 字节静态、命中 vLLM prefix-cache。**上游 v0.8.65 已自带同优化**(render_environment_block 不再输出 pwd;turn_meta 带 workspace),不再 fork-distinct。2026-07-07 复核后,Q 自动 warmup 也已撤除;历史首轮漂移不再归因为单纯冷 prefill。

### ~~Q `prefix-cache` session 启动自动 cache warmup(2026-06-18)~~ → **已撤除(2026-07-07)**
- **原文件**:`core/session.rs`(`Session.cache_warmup_done` 运行时标志)、`core/engine/turn_loop.rs`(首请求前置 warmup)
- **原改动**:本 session第一次发请求前,clone 完整本请求前缀发一次 `max_tokens=1`/`tool_choice=none`/响应丢弃的预热请求,试图给 vLLM prefix-cache 预热。
- **撤除理由**:GUI 复核中关闭自动 warmup 后仍无法复现历史工具协议漂移;证据不再支持“自动 warmup 是根因修复”。继续 always-on 会给每个新 session 多发一次完整 prefill,放大首轮延迟、GPU 压力和排查噪声,且可能掩盖 schema / MCP adapter / tool_search 注入等真实问题。
- **当前状态**:删除 `Session.cache_warmup_done` 和 turn_loop 首请求预热路径;恢复此前因额外请求计数而 ignore 的 wiremock 测试。手动 `/cache warmup` 仍保留为上游 TUI 调试命令。撤除提交经 fork PR [#9](https://github.com/h3c-hexin/DeepSeek-TUI/pull/9) 合入 `pinvou3-clean`,merge commit `8073aa9b`。

### C8 `ops` 会话工具开关(SetDisallowedTools)
- **文件**:`core/ops.rs`(新增 `Op::SetDisallowedTools { tools: Vec<String> }`)、`core/engine.rs`(handler 写入 `config.disallowed_tools`)
- **改动**:运行时把"被禁用工具全名(模型可见,小写)"广播给在跑引擎 → 写 `config.disallowed_tools`,下一轮 `filter_tool_catalog_for_gates` 即对模型隐藏。空 = 不禁用
- **理由**:pinvou3「会话工具开关」需要把用户在 GUI 关掉的 connector 即时同步给引擎(中途生效);消费方在 pinvou3-app `engine_pool::set_disallowed_all` + `commands::set_disabled_connectors`
- **来源**:fork PR h3c-hexin/DeepSeek-TUI#4(已 ff 进 `pinvou3-clean`,commit `a0efea0b`)
- **测试**:`forkguard` `SetDisallowedTools op 定义` + `SetDisallowedTools 写 disallowed` 指纹(L1);行为 L2 待补
- 上游 PR:❌ pinvou3 专用(留 fork)

### C9 `engine` disallowed_tools 前缀通配匹配(2026-06-30)
- **文件**:`core/engine/turn_loop.rs`(`command_denies_tool` 支持 `*` 后缀通配)
- **改动**:`disallowed_tools` 规则以 `*` 结尾时按前缀匹配(`mcp_qcc-company_*` 命中该 server 名下**所有动态发现**的工具),否则精确匹配。向后兼容(无 `*` 行为不变)
- **理由**:远程 MCP 连接器(qcc)工具名连上后动态发现、manifest `mcp_tools` 为空,精确名禁用失效;只有按静态可知的 server 名生成 `mcp_{server}_*` 前缀规则、且匹配层支持通配,才能在工具发现**之前**就把禁用规则写好(规则与发现解耦)。与 C8 同 disallowed_tools 主题(C8=广播 op / C9=匹配逻辑)。消费方=pinvou3-app `marketplace::model_tool_names` 对 manifest 每个 `servers[]` 生成前缀规则
- **来源**:fork PR h3c-hexin/DeepSeek-TUI#5(已进 `pinvou3-clean`,commit `8dcd29c2`)
- **测试**:`forkguard` `command_denies_tool 前缀通配` 指纹(L1) + `disallowed_tools_gate_blocks_prefix_wildcard` 回归(L2)
- 上游 PR:🟢 [#3824](https://github.com/Hmbown/CodeWhale/pull/3824) **MERGED**(2026-06-30,merge `4150b4835ca6`;测试 fixture 已泛化 `mcp_acme_*`,去 qcc/gongwen)。**下次 sync harvest 后撤 C9 指纹**

### C10 `runtime` MCP env placeholder + Windows child console suppression(2026-06-30)
- **文件**:`mcp.rs`、`mcp/tests.rs`、`dependencies.rs`、`hooks.rs`、`tools/plugin.rs`、`tools/shell.rs`、`utils.rs`
- **改动**:
  - MCP server 配置支持 `${NAME}` 占位符,在启动 MCP client 前从进程环境解析,用于 pinvou3-app 注入 keyring 取出的 API key/secret,避免把密钥落入 manifest 明文。
  - Windows 上启动 MCP / hook / plugin / shell 子进程时复用 `CommandExt::creation_flags(CREATE_NO_WINDOW)`,抑制后台控制台闪窗;非 Windows 平台无行为变化。
  - 与 C9 同步保留 `disallowed_tools` 前缀通配逻辑,让动态 MCP 工具可在发现前按 server 前缀禁用。
- **理由**:pinvou3-app 已把 MCP 密钥迁移到 OS keyring,manifest 只保留 env placeholder;若 DeepSeek-TUI 不解析 placeholder,天气/问财/企查查等 MCP 启动后拿不到密钥。Windows GUI 场景下后台子进程弹控制台会打断用户输入,需要在底座统一抑制;该实现只加 Windows cfg,跨平台构建不受影响。
- **来源**:fork PR h3c-hexin/DeepSeek-TUI#6(**已合并入 pinvou3-clean**)/ main PR pinvou3#72,submodule commit `01b51974`(= pinvou3-clean head;含 `3d2be320` env placeholder + `01b51974` windows console。原 PR head `af124353` 经 rebase 合并重写为此 hash,内容一致)
- **测试**:`mcp/tests.rs` 覆盖 env placeholder 解析;`forkguard` C9 指纹覆盖前缀通配;pinvou3-app `cargo check` 通过。Windows console suppression 为平台行为,本次按 `#[cfg(windows)]` 编译路径与手动运行观察验证。
- 上游 PR(2026-06-30 提交 + 当日合入,**拆两个独立 PR**;均从 origin/main 手动应用——上游 mcp.rs 大改、stdio 外移到 `mcp/stdio.rs`,cherry-pick 会冲突):
  - **env placeholder** → 🟢 [#3825](https://github.com/Hmbown/CodeWhale/pull/3825) **MERGED**(merge `f4f4555cc968`)。**只提 stdio 子进程 env 展开**(`StdioTransport::spawn` 内 `expand_env_placeholders_map(&config.env)`);测试 fixture 泛化 `MCP_TEST_SECRET_TOKEN`(去 PINVOU3_*/QCC/AMAP)。补底座真缺口:MCP stdio 子进程 env 走 allowlist 过滤(`sanitized_mcp_env`),secret env var 继承不到→config.env 必须显式带值,又不能明文落 mcp.json。(首推曾因 rustfmt 单行超宽 Lint fail,fmt 后重推即过)
  - **Windows child console** → 🟢 [#3823](https://github.com/Hmbown/CodeWhale/pull/3823) **MERGED**(merge `d87dabcd0cba`)。`CREATE_NO_WINDOW` 抹子进程闪窗,`#[cfg(windows)]` no-op off-Windows;上游已把 stdio spawn 重构进 `mcp/stdio.rs`,suppress 落新 spawn 点。
  - **header 展开未提**(冗余):底座原生 `bearer_token_env_var`/`env_headers`(`McpHttpAuth::resolved_headers`)已覆盖 header secret,故上游 PR 剥掉 header 展开只留 stdio env;app 层 qcc 宜改用底座原生(follow-up,仍 fork)。
  - **下次 sync harvest**:上游已合 stdio env 展开 + Windows 抑窗两块,sync 后撤对应指纹;**C10-env 的 header 展开部分仍 fork 保留**(pinvou3 在用,上游未收)。

### C11 `runtime` Windows killed background shell reader 收尾(2026-07-07)
- **文件**:`tools/shell.rs`
- **改动**:Windows 后台 shell 已被 kill 时不再同步 `join` stdout/stderr reader thread,而是释放 join handle;非 Windows 和非 killed 状态保持原 join 行为。
- **理由**:Windows 子进程 kill 后 reader 线程可能仍阻塞在管道读取,同步 join 会让取消流程卡死;job object 已先关闭,此处应优先保证取消立即返回。
- **来源**:fork PR [#7](https://github.com/h3c-hexin/DeepSeek-TUI/pull/7),commit `cf2b231f`;其中 warmup cancellation 部分已随 Q 撤除,仅保留本条 shell 修复。
- **守护**:`fork-guard.sh` C11 指纹钉住 `ShellStatus::Killed` 分支;目标 warmup 回归测试 3 条通过。Windows 行为仍需 Windows CI/真机覆盖。
- 上游 PR:暂未提;需先补 Windows 稳定复现与平台回归测试。

### C12 `working-set` 内部提醒不参与路径提取(2026-07-14)
- **文件**:`crates/tui/src/working_set.rs`
- **改动**:Working Set 实时观察用户消息及从历史重建时,仅在分析投影中剥离开头完整闭合的 `<system-reminder>...</system-reminder>`;原消息、持久化历史和发给模型的内容保持逐字不变。
- **理由**:pinvou3 为严格 chat template 把每轮权限提醒放在 user content 前缀;Working Set 若扫描整段会把提醒中的 `sudo/apt/systemctl/pkexec` 误判为 repo path。内部策略不是用户给出的路径信号,不应污染 Active paths。
- **测试**:`forkguard_working_set_ignores_leading_system_reminder_paths`、`forkguard_working_set_rebuild_ignores_leading_system_reminder_paths`,同时断言真实提示词路径仍被识别且重建不修改历史。
- 上游 PR:暂不提;`<system-reminder>` 是 pinvou3 的嵌入约定,先留 fork。

### R `agentic-rag` EngineConfig.extra_tools 应用层工具注入口(2026-06-24)
- **文件**:`core/engine.rs`(`ExtraTools` newtype + `EngineConfig.extra_tools` 字段 + Default)、`core/engine/tool_setup.rs`(`build_turn_tool_registry_builder` 末尾 `with_tool` 循环注册);连带补 3 处 TUI 路径 EngineConfig literal(`runtime_threads.rs`/`tui/ui.rs`/`main.rs`,`extra_tools: Default::default()`)
- **改动**:给 `EngineConfig` 加 `pub extra_tools: ExtraTools`(newtype 包 `Vec<Arc<dyn ToolSpec>>`,手写 Debug 输出工具名——`dyn ToolSpec` 非 Debug,否则破 `#[derive(Debug)]`),每 turn build registry 时 append 到 builder。让**嵌入应用**(pinvou3-app)无需 fork 工具表即可注册自定义 `ToolSpec`
- **理由/用途**:Agentic RAG——app 层 `KbSearchTool`(`knowledge/kb_tool.rs`,持 `session_id`,execute 查该会话挂载知识集 → `L1Store::retrieve_for_chat`)经此注入,让本地 LLM 自主调 `kb_search` 检索本地知识(替代旧注入式)。`spawn_for_session` 按 session push,工具持 session_id 解决 `ToolContext` 无 session_id 的问题
- **测试**:`forkguard` `RAG1 extra_tools 字段` + `RAG2 tool_setup 注册` 指纹;app lib `blocklist_contract`(kb_search 可见)+ `kb_tool::tests`;真机测自发调用率/幻觉率
- 上游 PR:❌ **暂不提**(2026-06-30 复核纠正原"拟提"):上游 codewhale-tui 是纯 binary(**无 lib target**,C1 facade 是 fork 专属),且 `app-server` crate 不依赖 tui / 不构造 EngineConfig——`EngineConfig.extra_tools` 在上游**无任何 in-tree 消费者**,提了大概率被以「加无消费者的 public API」关。留 fork,等上游真出现嵌入/SDK 需求或能附 in-tree 消费者再提

### W `workflow` 三省六部工作流底座层
- **文件**:`tools/subagent/{mod,tests}.rs`、`core/ops.rs`、`core/events.rs`、`core/engine.rs`、`core/engine/{tests,approval,handle}.rs`、`tools/user_input.rs`、`runtime_threads.rs`、`tui/{sidebar,command_palette,ui,views/mod}.rs`、`main.rs`(EngineConfig 字段)
- **子 patch**:

  | | 内容 |
  |---|---|
  | W1 | `Op::SpawnSubAgent` +role_id/allowed_tools/max_steps/output_schema/expects_file_output;engine 按角色白名单+步数派 Custom SubAgent;空白名单 fail-fast |
  | W2/W3/W11 | StructuredOutput:`submit_output` 工具 + schema 校验 + x-output-file 落盘;催交重试上限(`MAX_STRUCTURED_OUTPUT_RETRIES`),耗尽置 failed;**结构化产出落盘成功即 break**(否则 temp=0 永动) |
  | W4 | `request_user_input` 答案总线路由给 SubAgent(`user_input_tx`,不吃 TOOL_TIMEOUT) |
  | W5 | `AgentComplete` +role(SDAN)+failed(宿主走失败路径,不被陈旧产物洗成 PASS) |
  | W6 | SubAgent Mailbox(TokenUsage 等信封直达宿主)+ AgentSpawned 关联 agent_id→role_id |
  | W7 | 贪心解码:SubAgent 每步 `temperature=0`(根治 NVFP4 下工具调用 XML 被采歪→空转) |
  | W8 | SubAgent surface 注册 web/custom 工具 |
  | W9 | ~~read_pdf catch_unwind 防 panic~~ **v0.8.60 被上游 `guard_pdf_extract` harvest**(见 §2.2) |
  | W10 | `EngineConfig.reasoning_effort` 会话建时初始化(不依赖首条 SendMessage);`"off"` 由 app bridge 按 `provider==vllm` 注入 |
  | W12 | `SubAgentSpawnOptions.max_steps` per-spawn 覆盖(`options.max_steps.unwrap_or(self.max_steps)`),registry 的 15/20/30 真生效 |
  | W13 | `Op::CancelSubAgents` + `SubAgentManager::cancel_all_running`：宿主可在用户停止整个工作流时显式 abort 全部后台 SubAgent；普通 `CancelRequest` 仍只取消前台 turn |
- **tool_whitelist**(与 C2 blocklist **互补两层,不冲突**):`EngineConfig.tool_whitelist` 通用白名单机制(submodule 字段 + turn_loop `retain`)。blocklist 全局减法(建 catalog 时);tool_whitelist per-session `retain`(turn_loop 最后)。whitelist 在 blocklist 过滤后的集上 retain → **无法重新暴露黑名单工具**。⚠️ **app 层监工用法已删(2026-06-15,对话型监工废弃)**:`supervisor_tool_whitelist()` + `spawn_for_session` 施加 + 死代码 `build_engine_config_for_workflow` 均移除,**机制本身(submodule)保留待用,字段恒 None**;submodule `engine.rs:263` doc 仍有一处指向已删函数的悬空引用,待下次 sync 顺带清。
- **验证**:L1 subagent scenarios 真 vLLM 跑通(`subagent_compare_3_libs` 并行 3 agent / 487s);W1–W13 forkguard 指纹;行为层含 W10 `engine_config_locks_critical_fields`、W13 `forkguard_cancel_all_running_aborts_every_live_agent`
- 上游 PR:❌ pinvou3 专用(可复用上游 WhaleFlow 基础 crate,暂未迁)

### app 层 fork(不在 submodule —— override hook / bridge 注入,fork-guard 也守)
- **prompt 内容(单一来源,main #14 重构 2026-06-15)**:`resources/bundle/instructions.md` 是**唯一 pinvou3 prompt 来源**——宪法/裁决/`AUTHORITY_RECAP` 全折叠进 §底线 + 动态注入 `{{PINVOU3_MODEL}}`/`{{PINVOU3_DATE}}`(治"编时间");`bridge/bundle.rs` 只剩 Mode 块 + `LOCALE_PREAMBLE/CLOSER` zh+ja 短版(`AUTHORITY_RECAP=""`、base.md 留空 stub、`compose_static_layers` 丢 base 只剩 Mode,**Plan 模式仍按 mode 切**)。经 `set_*_override` + `set_static_prompt_composer_override` 注入。**submodule 内 prompt 文案 drift=0**。依据=ablation 实测(user memory `prompt-ablation-methodology`):base.md 对 Qwen3.6 可测价值仅 Voice;整 prompt 22590→16612B,剩余大头=Skills~52%(`~/.agents/skills` 全局 lark 技能,待重设计)
- **bridge config**(`bridge/mod.rs`):`subagent_api_timeout=300`、`max_subagents`(prefs 默认)、`network_policy` fake-ip CIDR(`198.18.0.0/15`)、`compaction.token_threshold` = **`derive_compaction_threshold()` 动态推导**(探测 `max_model_len` → **调底座 `context_input_budget_for_route` 拿 emergency budget E** → `T=(E−S)/1.5−FIXED`,clamp;写死 190K 已废、被证在健康 256K 机也倒置)、`InstructionSource::Inline`。v0.8.58-60 新字段(verbosity/interactive_launch_limit/goal_*/disallowed_tools)全透传 default
- **auto-compact 触发不倒置(根治,2026-07-03)**:pinvou3 曾**镜像**底座 `INTERNAL_BUDGET_LARGE_WINDOW_THRESHOLD=500K`/`TURN_MAX_OUTPUT_TOKENS=262144`/output 预留分档/`E=W−O−1024` 公式来自算 E;上游 sync 改这些 → **编译不报错却静默倒置**(与 tool_search 折叠单名同类:依赖上游不变的假设,间接检查抓不到)。**修**:底座 `pub` 出 `context_input_budget_for_route`(`core/engine.rs` re-export,可上游),pinvou3 `derive_compaction_threshold` **直接调底座拿 E**、删所有镜像常数 → 上游改 output 预留/公式 pinvou3 自动跟随、永不倒置。守护:`forkguard_compaction_threshold_below_emergency_all_windows` **对拍底座 E**(跨仓一致,非镜像)+ `compaction_128k_scenarios`(钉 env=24576 模拟生产 wire)+ fork-guard 3 指纹(depub/derive/测试);负向验证过(FIXED=0 → 对拍测试报倒置 fail)。详见 `docs/context-compaction-设计.md`
- **敏感目录 deny hook**:`resources/bundle/deny_sensitive_paths.sh`——ToolCallBefore 拦敏感路径 + 关闭态 sudo。**hard-deny 必须 `exit 2`**(v0.8.60 Hooks v2 `fold_tool_call_before_results` 只认 exit_code==2,旧 exit 1 被当 passthrough)
- **dump 工具**:`bin/dump_system_prompt.rs`(随 `PromptSessionContext` 字段 / prompt 函数签名维护)

---

## 2. 移除 / harvest 清单

### 2.1 clean re-fork 永久丢弃(2026-06-04,不再带入)
- subagent 本地约束全套(MAX_STEPS/ELAPSED/resolve_agent_ref/tool_agent_route)——`agent_*`/`delegate` 全在 blocklist,生产不可达
- phase/demo workflow(跨仓全删)——已由 W 三省六部重做
- qwen-128K 死码(models.rs)——真实模型走上游 `_Nk` hint

### 2.2 已被上游 harvest(指纹撤除,非 fork-distinct)
- **v0.8.53 及以前**:bing decode、network_policy fake-ip API、InstructionSource enum、base override hook、EngineConfig.instructions、256K auto-compact 基础设施、MAX_OUTPUT env、file_search/grep_files timeout
- **v0.8.57**:skills union 接线、C4-a 多行逐行(`split_command_segments`)、本地 Bocha(#2946)
- **v0.8.60**:**W9 read_pdf catch_unwind** → 上游 `guard_pdf_extract`(`file.rs`,同语义 catch_unwind+错误映射,带自测;char-boundary 部分也已是上游自带)。代价仅罕见 font/CMap panic 的中文提示
- **v0.8.65**:**P pwd/workspace 移出静态 system**(`render_environment_block` 不再输出 pwd + turn_meta 带 `Current workspace`)——上游 v0.8.65 已自带同优化(hexin 本人 PR `f981134d` harvest),撤 fork-guard P 2 条指纹(`env block 移出 volatile pwd` / `turn_meta 注入 workspace`)

---

## 3. fork-guard 守护 + sync 后验证

```bash
./scripts/fork-guard.sh          # 全量:指纹层 + 编译跑回归测试
./scripts/fork-guard.sh --fast   # 仅指纹层,秒级(merge 后第一道快筛)
```

两层:**指纹层** grep 每个 fork 标记是否还在(抓「merge 静默丢整段 patch」);**行为层** `cargo test` 跑回归测试(抓「值/逻辑被改回上游」)。完整清单见 `fork-guard.sh` `fingerprints=` 数组——新增 fork patch 必同步加指纹(见 fork-policy §3)。

### ⚠️ sync 后必做验证 checklist(fork-guard **不够**,每条都踩过坑)
1. **全量 lib 测试** `cargo test -p codewhale-tui --lib`——抓非 `forkguard_` 前缀的上游测试因 fork fail(v0.8.51 append_file 静默丢失靠此抓)
2. **dump_system_prompt 前后 diff**(不在 fork-guard 构建里)——非 0 就逐块查谁漏进静态 prompt(v0.8.57 Runtime Policy 141 行泄漏靠此抓)
3. **扫 per-turn message 构造路径** `grep -rn "runtime_prompt\|messages.push" turn_loop.rs engine.rs`——上游可能新增每请求注入的 transient 消息,dump 抓不到
4. **工具集合 + 激活机制盘点**:① 对比两版 `ToolSpec::name()` 集合,新工具漏入要补黑名单;② **更要查上游有没有新增能激活 deferred 工具的机制**(`tool_search`/`ensure_advanced_tooling` 类)——blocklist 是 defer 非删除,任何激活 deferred 的新路径都击穿门控
5. **hook 决策协议**:上游可能改 hook 退出码/JSON 契约(v0.8.60 Hooks v2 把 hard-deny 从「非零」改成「exit 2」)——dump/编译都抓不到,必须读 `fold_tool_call_before_results` 确认 deny 脚本退出码契约
6. **app 端单线程测试** `cargo test --manifest-path pinvou3-app/.../Cargo.toml --lib -- --test-threads=1`——bridge env 测试并行会 flake(非回归)

---

## 4. Sync 历史

### Scheduled-lite 重做:撤回 durable scheduler(2026-07-13,分支 `codex/scheduled-lite`)
- **动机**:fork PR #8 的 durable scheduler 使 drift 达 +13744/−5646(59 文件),远超 1500 行软上限;复盘认定 sidecar index / retention guard / crash-safe prune / journal 对办公场景(小时级起步、单机少量任务)过度设计。
- **做法**:submodule 回 `8073aa9b`(fork PR #9 tip)建 `codex/scheduled-lite`,重打 AUTO-lite 最小补丁(+336/−10,5 文件,见 §1 AUTO-lite);父项目 executor 回旧版 `mpsc::UnboundedSender<TaskExecutionEvent>` 事件接口,`scheduled_tasks.rs` 撤二级索引/retention/pruning 适配;前端撤分钟级创建入口(只留小时/每天/工作日/每周),定时任务侧边栏入口恢复。**简化决策:定时任务恒 YOLO**——前端不暴露任务级权限设置,AI 草稿 schema 去权限字段,运行时复用普通聊天 Yolo 权限推导(见 §1 AUTO-lite 审批产品决策)。
- **保留物**:durable scheduler 重实现留 `codex/scheduled-tasks` 分支备查,后续如需分钟级高频调度可按主题拣回。
- **验证**:底座覆盖 model/conversation key 透传、schema v4/v3 兼容、强制审批与运行链接持久化;app 覆盖恒 YOLO、独立运行对话、任务级共享工作间与 session retention;另跑父项目 `cargo check --locked`、前端单测+生产构建、UI 冒烟与 fork-guard。AUTO-lite 指纹由 8 条增至 13 条。
- **2026-07-14 会话模型修正(一次运行一对话)**:底座持久 `conversation_key=automation.id`,每次运行生成独立 execution task id;app 每次运行创建新的 `sched-*` 会话，模型和提示词等 profile 固定在该次运行。同一 automation 的会话彼此独立，继续追问只进入选中的运行会话。UI 时/分采用 iOS 风双滚轮(`ScheduledTimeWheel`)。守护:`scheduled_runs_get_independent_conversations_and_share_the_task_workspace`。
- **2026-07-14 存储与工作区语义**:定时任务不再有“选目录”;工作间固定为 `~/.pinvou3/scheduled/<automation_id>/workspace/`。同一任务的所有运行对话共享该目录，不同任务互相隔离；定时会话 JSON 与普通会话统一存入 `~/.pinvou3/sessions/`，产物归属仍按每次运行会话的 artifact 记录隔离。软件未发布，不迁移旧 scheduled store/profile/owner/产物；profile schema 和 workspace 必须严格匹配当前规则。普通对话继续沿用底座最新 50 条清理；定时任务按 automation 保留最新 50 条终态记录，超额时配套删除 Session/Run/Task，queued/running 永不删除；无 cwds 任务一等公民、**点模板即激活**。
- **2026-07-14 列表/详情响应式层级**:未选任务时展示“已安排的任务”标题、产品说明、搜索、筛选和建议模板;选中任务后介绍区自动消失,恢复紧凑双栏(左侧筛选/搜索/任务列表,右侧当前配置)。详情页彻底移除 Shell/信任模式权限行,只显示 `Yolo · 自动执行`;后端每次运行按普通聊天规则读取全局 Shell,并固定信任与自动批准。UI 单测、Vite production build、真实浏览器 smoke 均通过。
- **2026-07-13 二次复审修复**:① 编辑并重发放开——`commands.rs` 的 `edit_last_turn` 撤 `ensure_chat_session` 门(EnginePool 内部本就按 scheduled_profile 做 turn gate),与继续追问同路;会话管理类命令(删除/改名/归档/save_session_messages)仍拒绝 scheduled 会话;② 运行中链接落盘(AUTO-lite ③);③ `.gitattributes` 强制 `*.sh eol=lf`(autocrlf=true 的 checkout 曾把 fork-guard.sh 转成 CRLF,严格 bash 直接失败);④ fork-policy §0 基线同步 8073-lite。
- **⚠️ Windows 本机跑 app lib 测试**:`rfd`(tauri-plugin-dialog)静态导入 `TaskDialogIndirect`,cargo test 的裸测试 exe 无 manifest → 解析到 System32 comctl32 v5 → `STATUS_ENTRYPOINT_NOT_FOUND(0xc0000139)` 启动即挂。绕法:在 `target/debug/deps/pinvou3_lib-<hash>.exe.manifest` 放 Common-Controls v6 外置 manifest(声明 `Microsoft.Windows.Common-Controls 6.0.0.0`)后正常运行;根治需在 build script 给 test 目标嵌 manifest(待议)。
- **2026-07-16 小时调度起点**:`HOURLY` 接受可选 `BYHOUR/BYMINUTE` 时间锚点；`next_run_at` 以任务创建时间对应的本地日期为固定参考连续推进，避免恢复、tick 或跨天后相位漂移。父仓创建/编辑表单显示“起始时间”，聊天引导使用同一格式；无锚点旧规则继续按创建/恢复时刻推算。通用 RRULE 语义已拆为 [#4381](https://github.com/Hmbown/CodeWhale/pull/4381) **OPEN**，仅保留 pinvou3 的 UI/会话/工作间契约在 fork。

### ~~Durable scheduled runtime~~(2026-07-13,fork PR #8,HEAD `5f5a58db`;**同日被 Scheduled-lite 撤回**,见上)
- **规模触发**:AUTO 使 fork drift 从 `+7070/−4930,54 文件` 增至 `+13744/−5646,59 文件`,显著超过 1500 行软上限,已执行撤回评估。
- **撤回评估结论**:当前保留。PINVOU 定时任务必须复用底座 `AutomationManager` / `TaskManager` / scheduler / session contract;把 parser、队列、崩溃恢复和持久化在 app 层重写会形成第二套底座,违反项目架构边界。AUTO 的通用部分后续按较小主题拆分上游提案,上游接受等价能力后逐块撤回。
- **历史清理**:fork PR #8 从 `8073aa9b` 直接重建为单一 feature commit,去掉 gitlink 异常遗留的重复 `c1edbd26` 和旧 merge;最终 rebase merge 为 `5f5a58db`。
- **验证**:`automation_manager` 53/53、`task_manager` 44/44、YOLO 强制审批 hook、下游 executor contract 通过;`manual_run_recovers_journaled_enqueue` 用 `save_run` failpoint 真正覆盖“任务已持久入队、run 尚未落盘”的恢复窗口。

### v0.8.65 + 第 3 次 clean re-fork(2026-06-29,HEAD `6445fc4c`)
**史上最大 sync**:merge v0.8.60→v0.8.65,**659 commit / 561 文件 / 52 冲突块(16 文件)**;命中 fork-policy §5 撤回评估三触发(drift 2443>1500 / conflict 52>10 / 上游新 API 与 W 重叠)。
- **W 层撤回评估结论(关键修正)**:原评估"撤 W5/6/8/10/12 用上游"**高估了撤回面**——W 工作流层(`Op::SpawnSubAgent` 多字段 + `AgentComplete{role,failed}` + `AgentSpawned` role 关联 + structured output 落盘)是 **pinvou3-app harness(三省六部)的底座接口**,撤掉直接断工作流。**定案=fork 基底**:`subagent/{mod,tests,mailbox}` 整套取 fork(W + 对话工具 agent_open/eval/close 全保,**Qwen3.6 零风险,不换上游单 agent**),只适配上游编译变化。真正撤回只剩 **P**(pwd-move,上游已 harvest)。
- **冲突解法**:25 块手解(engine #5 match 用 ops.rs canonical enum 整段重建 / prompts 8 块 C7 保宽版 / project_context 砍空 / skills #41 在上游 SkillDiscoveryMode API 重表达 / app 取上游 `mod tests;`)。
- **编译涟漪(大头)**:fork v0.8.60-era subagent 移植 v0.8.65 适配 104→0 error。关键:C1 facade lib.rs 删 3 stale + 加 11 上游新 mod;`SubAgentResult` 加 4 上游字段 + `SubAgentStatus::BudgetExhausted`;从上游移植 `terminal_results_excluding`/`update_runtime_limits`(token_budget 参数 `_` 忽略)/`subagent_completion_from_result` 等进 fork manager;manager 构造器 4→6 arg;`AgentWorkerSpec` 加 `runtime_profile`;恢复 fork C7 三件套(`runtime_prompt_message`/`runtime_prompt_text`/`approval_mode_for`)。
- **决策③(runaway 护栏,用户拍板)**:token-budget scope-gate 遥测**不港**(fork 用步数上限 max_steps;上游 receipts 测试 `#[cfg(any())]` 排除);`MAX_SPAWN_DEPTH_CEILING` **用上游 8**(均为 merged 现状,零改动)。
- **验证**:lib **5148 pass**(+55 ignored fork 基底行为 +1 verifier 并行 flake)+ **三省六部 mock-LLM e2e ×2**(wiremock 绕过 #402:SpawnSubAgent→submit_output→AgentComplete happy + fail-closed);pinvou3-app harness 适配新 API(build_engine_config 删 capacity/改名 launch_concurrency/加 8 字段、Op::SendMessage +provider/provenance/dynamic_tools、AgentSpawned/Progress +`..`、dump +skills_scan_codewhale_only)后 GUI 手动走查通过。
- **clean re-fork**:`git reset --soft v0.8.65` 保树 → 8 主题 commit(C1/C2/C3/C4/C5+C7/C6/W/test),**最终树与 merge `f6558746` 字节等价 diff=0**。备份 `backup/v0.8.65-merge-result` + `backup/pre-reclean-trial-tip`。

### Clean re-fork(2026-06-15,HEAD `1161bc78`)
第 2 次 clean re-fork(首次 2026-06-04 ← v0.8.53)。动机:v0.8.60 merge 后历史乱(26 commit / ~10 merge / fork 散落三次 sync)。
- **做法**:`git reset --soft v0.8.60` 保留全部 fork 树 → 按 file→theme 重组成 8 个线性主题 commit,**最终树与 merge `fa412ca1` 字节等价**(fork-guard 41 指纹全过)。备份 `backup/pre-reclean-v0.8.60`
- **逐 patch 评估**:全部 C1-C7+W 都在用(L1 21/21 + L2 166 + forkguard 验证),无活代码可删;C4 评估为留(YOLO 防灾难网);tool_whitelist↔blocklist 不冲突(互补两层);Plan 模式属 app 层独立清理,本次不动
- **清理**:shell.rs 删已推翻的 `嘴替设计.md` 引用;engine.rs 去过时品悟引用 + 加 tool_whitelist 两层模型 doc

### v0.8.60(2026-06-15,merge v0.8.57→v0.8.60,279 commit / 248 文件)
**大版本 sync**。上游主线:Native Anthropic provider、**Hooks v2(JSON allow/deny/ask 决策契约)**、Agent Fleet 真跑、/goal 目标管理、concise verbosity、interactive fanout 闸、多 provider/model、命令重构成 `commands/groups/`、constitution prompt 改 YAML+renderer。
- **冲突面小**:248 文件仅 **7 文件 / 14 冲突块**(其余自动合并)。详:prompts.rs(C7 测试 + 访问器名对齐)/ turn_loop.rs(C7 gate)/ project_context.rs(C5 砍空保 None)/ subagent/mod.rs(W union)/ subagent/tests.rs(W union)/ file.rs(W9 harvest)/ main.rs(EngineConfig 字段)
- **🔴 抓到真安全回归**:`deny_sensitive_paths.sh` 靠 **exit 1** 拒绝,但 Hooks v2 改成只认 `exit_code==2`(exit 1 当 ALLOW)→ 硬墙静默失效。修:全改 exit 2 + fork-guard 加指纹
- **app 适配**:EngineConfig/Op/dump 补 5+3+2 个新字段(GUI 全透传 default);lib.rs 加 `pub mod fleet/context_report/model_inventory`、删孤儿 `prompt_persist`
- **验证**:dump 字节稳定、blocklist 无需改(无新 model 工具)、fork-guard 全过、lib 4539/app 166 pass、L1 21/21
- **教训**:① 上游同语义 API 名差异(`effective_static_prompt_composer`)merge 取上游名,编译能抓;② **hook 决策协议变更是 dump/编译都抓不到的隐形安全回归——必须读 fold 逻辑**;③ 大版本号差≠小 diff,commit/文件数才是真规模

### v0.8.57(2026-06-11,merge v0.8.53→v0.8.57,342 commit)
DeepSeek→CodeWhale rebrand + **system prompt 改 mode-independent**(mode/approval 移出静态前缀走 per-turn `<runtime_prompt>` tag)。关键判断:C7 composer 同名 API 语义分叉(保宽 ctx)、Runtime Policy + runtime_prompt tag 两道新 gate(#42)、**tool_search 击穿 blocklist**(上游新注入路径激活 deferred agent 工具 → 前端裸 JSON;靠 `spawn_headless` probe 真实链路定位,修法见 C2)。

### 旧版教训速查(v0.8.47–53,per-conflict 细节已废弃)
| 版本 | 可复用教训 |
|---|---|
| v0.8.53 | dump bin 不在 fork-guard 构建里,**sync 后单跑**(`PromptSessionContext` 漏字段靠它抓) |
| v0.8.51 | **sync 后必跑全量 lib 测试**(merge 取上游 `Implementer.allowed_tools` 静默丢 append_file) |
| v0.8.49 | **整文件 `--theirs` 危险**(冲掉不在冲突区的 fork patch)→ fork-distinct 文件逐 hunk 解 |
| v0.8.47 | 上游把工具 deferral 翻成 allowlist(`request_user_input` 被 defer 气泡消失)→ C2 的由来 |

### app 层 prompt 瘦身(2026-06-05,20.2K→8.9K,迭代 prompt 前必读)
反事实审计(「没它哪条生产路径会变」)删:Personality(并入 base.md §Voice)/ Session Longevity(与 blocklist 矛盾)/ Approval Policy(单 Yolo-Auto)/ prompt-cache 教学 / Compaction Relay 模板(无生产者无消费者)/ Article VII 九层→三行裁决 / Sub-agents(工具不可见)。操作性原则归 instructions.md 单一来源,base.md 只留红线+裁决+语气。
