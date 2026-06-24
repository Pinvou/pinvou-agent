# DeepSeek-TUI Fork 修改清单

> pinvou3 对 `DeepSeek-TUI`(已 rebrand `CodeWhale`)底座所有 fork 修改的**单一现状清单**。
> 用途:① sync 后查 patch 存活 ② 交接 / onboarding ③ 上游 PR 定位改动点。
> 配套:`scripts/fork-guard.sh`(指纹 + 回归测试守卫)、`docs/fork-policy.md`(维护策略 + sync 流程 + PR 状态)。
>
> **当前基线**:submodule 分支 `pinvou3-clean` ← upstream **v0.8.60**;HEAD `1161bc78` = v0.8.60 + 8 主题 commit(2026-06-15 clean re-fork,线性历史)。

---

## 0. 当前状态速览(2026-06-15)

| 项 | 值 |
|---|---|
| submodule 分支 | **`pinvou3-clean`**(`.gitmodules` 追踪);HEAD `1161bc78`;备份 `backup/pre-reclean-v0.8.60` |
| fork drift | **+2335 / −360 行,43 文件**(`git -C DeepSeek-TUI diff v0.8.60..HEAD --shortstat`)。超 1500 软上限,主体是工作流层 W——属"接受重 fork"(fork-policy §0);app 层 prompt 走 override 注入,不计入 |
| 历史 | v0.8.60 + 8 commit:C1 lib · C2 blocklist · C3 append_file · C4 safety · C5+C7 prompt-composer · C6 chore · W 工作流层 · docs。后续叠加:C8 会话工具开关 op(#4,`a0efea0b`,2026-06-23) · R extra_tools 注入口(`6b3059da`,2026-06-24) |
| LLM 暴露 native 工具 | **23 个**(全量注册 − 81 黑名单;**tool_search 已禁用**,模型无法激活 deferred 工具)。MCP `mcp_pinvou_present_artifact` 另接,共 24 入口 |
| fork-guard | **49 指纹 + 回归测试**(`scripts/fork-guard.sh`;+C8 会话工具开关 2 条 +RAG1/RAG2 守 extra_tools 注入口);底座 lib 4539 pass(+1 已知 flake:verifier 后台 shell 并行误报)/ app lib 190 pass(单线程;另 5 个 `bridge::tests` legacy model_preset 测试 9e296c4 模型列表化后过时失败,main 上即如此,非合并回归) |
| system prompt | dump 逐字节稳定(210 行,diff=0);per-turn `<runtime_prompt>` tag + goal continuation 均已 gate |

---

## 1. fork 结构(C1–C8 + R + W 逻辑主题)

> 逻辑分组,对应主题 commit。看某文件 fork-distinct 改动:`git -C DeepSeek-TUI diff v0.8.60..HEAD -- <file>`。
> 冲突易出血优先级(sync review 顺序):**prompts.rs(C5+C7) > turn_loop.rs(C7) > subagent/mod.rs(W) > tool_catalog.rs(C2) > project_context.rs(C5)**。

### C1 `lib` library facade
- **文件**:`crates/tui/src/lib.rs`(整文件——上游只有 `main.rs`,无 lib target)
- **改动**:`pub mod` 暴露内部模块 + `#[cfg(test)] pub mod test_support`,让 pinvou3-app 以 `deepseek_tui::*` as-library 调用 + `cargo test --lib` 能跑
- **⚠️ 维护**:上游每加/删模块要**手动同步 `pub mod`**(上游无 lib.rs,3-way 不会自动改)。孤儿 `pub mod` 会编译错(v0.8.51 `cycle_manager` / v0.8.60 `prompt_persist` 删除即此坑);`acp_server` 依赖 bin 专属符号不能进 lib
- 上游 PR:❌ pinvou3 专用

### C2 `tools` blocklist 工具门控
- **文件**:`tools/pinvou3_blocklist.rs`(新建,**81 条黑名单**)、`core/engine/tool_catalog.rs`、`tools/registry.rs`、`tools/mod.rs`
- **哲学**:上游(v0.8.47 起)是 **allowlist**;pinvou3 相反——**显示全部、只隐藏黑名单**,给 Qwen3.6 精简到 **23 工具**
- **关键**:`pinvou3_should_defer_native_tool(name, mode, always_load)` **mode-aware**:Yolo 只 defer 黑名单。`request_user_input` 跨所有 mode 硬保留(否则 GUI 不出选择气泡);`image_analyze` 放出(需 bridge 开 `VisionModel` feature);`checklist_*` 有意可见。`PINVOU3_BLOCKLIST_OVERRIDE` env 供 L1 harness 解锁
- **⚠️ tool_search 防御**:blocklist 是「defer 不删除」,工具仍在 catalog。上游 `tool_search`(`ensure_advanced_tooling` 注入)能让模型**搜索激活被 blocklist 的 deferred 工具**→ 击穿门控。修法:`tool_search_*` 进 blocklist + **注入处 gate**(`is_pinvou3_hidden(TOOL_SEARCH_*)` 为真不注入)→ catalog 根本不含
- **测试**:`pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default`、`forkguard_tool_search_not_injected_*`
- 上游 PR:❌ 哲学相反

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
- **skills**:扫描路径只留 `~/.agents/skills`(原 10 路径,#41;union 接线已被上游 harvest,只剩路径收窄)
- **测试**:`forkguard_skills_dir_unions_*`;project_context_cache / skills 多路径上游测试 `#[ignore]`
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

### P `prefix-cache` pwd/workspace 移出静态 system(2026-06-17)
- **文件**:`prompts.rs`(render_environment_block 删 `- pwd` 行,与 C5/C7 共此文件)、`core/engine.rs`(turn_metadata_block 加 `Current workspace` 行)。app 层同步:`instructions.md` 删 `{{PINVOU3_WORKSPACE}}`/`{{PINVOU3_DATE}}`(改静态文案 + 相对路径引导)、`bridge/mod.rs` 删对应 replace
- **改动**:每 session 变的 workspace 路径(pwd)从静态 `## Environment` **移出** → per-turn `<turn_meta>` 的 `Current workspace`;date 同理(turn_meta 本就有);产出引导改"用相对路径写,工具自动落 workspace"(实测 4/4,PathEscape 兜底)
- **理由**:vLLM(开 `--enable-prefix-caching` + 投机解码 mtp)在 prefix-cache **部分命中**(system 前半命中上个 session、到 workspace 处分叉接续 prefill)时,分叉点 KV 不自洽 + mtp 对 KV 敏感 → 工具调用退化成裸 XML(实测 L1 single subagent 25%;**所有工具受影响**,read_file/exec_shell 部分命中下全 0~1/4)。移 pwd/workspace 让 static system 跨 session 字节静态 → 完整命中,25%→~100%。⚠️ **生产 GUI 有 cache warmup 自我预热完整 prefix、基本不犯**(B 5/6);此 fork 主要修 **L1 headless(无 warmup)测试准确性** + 防御(warmup 失效兜底)
- **测试**:`forkguard_environment_block_omits_volatile_pwd`(指纹同名);L1 `subagent_single_simple`(25%→稳态100%,13/13)+ `relpath_write_file`(相对路径落 workspace,4/4)
- 上游 PR:✅ **拟提**——prefix-cache 优化通用,且符合上游 environment-volatile 方向(§8 #2314 已 merged);PR 拟为 "move volatile pwd from static system prefix to per-turn turn_meta"
> ⚠️ **2026-06-18 订正**:本节"生产 GUI 有 cache warmup 自我预热"是**错的**——`build_cache_warmup_request` 底座只有手动 `/cache warmup` TUI 命令触发,pinvou3 Tauri GUI **从不自动调**。真相是新 session **首请求**仍冷启动 × mtp → 首轮采歪(见 Q 节)。pwd-move 本身**与漂移无关**(实测放/不放都不是因),保留即可。

### Q `prefix-cache` session 启动自动 cache warmup(2026-06-18)
- **文件**:`core/session.rs`(`Session.cache_warmup_done` 运行时标志)、`core/engine/turn_loop.rs`(首请求前置 warmup)
- **改动**:本 session **第一次发请求前**,用**完整本请求前缀(system+tools+当前轮 user 消息及其 `<turn_meta>`)** clone 一个 `max_tokens=1`/`tool_choice=none`/`stream=none`/响应丢弃的预热请求 `await` 发出,把整段冷前缀喂进 vLLM prefix-cache;一次性(flag)、不进 context、30s 超时兜底
- **根因/理由**:vLLM(NVFP4)+ mtp 投机解码在新 session **首请求冷 prefill** 上把生成采歪——首个 `tool_call`/`<turn_meta>` 标签/系统指令被吐成裸文本(实测两 session:首轮漂、用户**问一句即自愈**——本质就是手动 warmup)。⚠️ **必须预热到 turn_meta**:模型恰在 `<turn_meta>` 处复读采歪(msg1 实锤 `...qwen36_35b_35b_256k...` 重复),v1 用 `build_cache_warmup_request`(剥掉当前轮 user 消息)漏热 turn_meta → 仍漂;v2 热完整首请求才根治。**漂移与工具表/subagent 放通无关**(兜大圈验证后定论:是首轮冷启动,非 schema)
- **测试**:`forkguard` `session warmup flag` + `首请求 warmup 注入` 指纹;行为待补 L1(新 session 首轮 tool_call 不漂)
- 上游 PR:✅ **拟提**——本地 vLLM+mtp 的通用 first-turn 防漂,自动 warmup 比手动 `/cache warmup` 更稳

### C8 `ops` 会话工具开关(SetDisallowedTools)
- **文件**:`core/ops.rs`(新增 `Op::SetDisallowedTools { tools: Vec<String> }`)、`core/engine.rs`(handler 写入 `config.disallowed_tools`)
- **改动**:运行时把"被禁用工具全名(模型可见,小写)"广播给在跑引擎 → 写 `config.disallowed_tools`,下一轮 `filter_tool_catalog_for_gates` 即对模型隐藏。空 = 不禁用
- **理由**:pinvou3「会话工具开关」需要把用户在 GUI 关掉的 connector 即时同步给引擎(中途生效);消费方在 pinvou3-app `engine_pool::set_disallowed_all` + `commands::set_disabled_connectors`
- **来源**:fork PR h3c-hexin/DeepSeek-TUI#4(已 ff 进 `pinvou3-clean`,commit `a0efea0b`)
- **测试**:`forkguard` `SetDisallowedTools op 定义` + `SetDisallowedTools 写 disallowed` 指纹(L1);行为 L2 待补
- 上游 PR:❌ pinvou3 专用(留 fork)

### R `agentic-rag` EngineConfig.extra_tools 应用层工具注入口(2026-06-24)
- **文件**:`core/engine.rs`(`ExtraTools` newtype + `EngineConfig.extra_tools` 字段 + Default)、`core/engine/tool_setup.rs`(`build_turn_tool_registry_builder` 末尾 `with_tool` 循环注册);连带补 3 处 TUI 路径 EngineConfig literal(`runtime_threads.rs`/`tui/ui.rs`/`main.rs`,`extra_tools: Default::default()`)
- **改动**:给 `EngineConfig` 加 `pub extra_tools: ExtraTools`(newtype 包 `Vec<Arc<dyn ToolSpec>>`,手写 Debug 输出工具名——`dyn ToolSpec` 非 Debug,否则破 `#[derive(Debug)]`),每 turn build registry 时 append 到 builder。让**嵌入应用**(pinvou3-app)无需 fork 工具表即可注册自定义 `ToolSpec`
- **理由/用途**:Agentic RAG——app 层 `KbSearchTool`(`knowledge/kb_tool.rs`,持 `session_id`,execute 查该会话挂载知识集 → `L1Store::retrieve_for_chat`)经此注入,让本地 LLM 自主调 `kb_search` 检索本地知识(替代旧注入式)。`spawn_for_session` 按 session push,工具持 session_id 解决 `ToolContext` 无 session_id 的问题
- **测试**:`forkguard` `RAG1 extra_tools 字段` + `RAG2 tool_setup 注册` 指纹;app lib `blocklist_contract`(kb_search 可见)+ `kb_tool::tests`;真机测自发调用率/幻觉率
- 上游 PR:✅ **拟提**——`extra_tools` 是通用扩展点(任何嵌入方可注册工具),与具体 kb_search 解耦

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
- **tool_whitelist**(与 C2 blocklist **互补两层,不冲突**):`EngineConfig.tool_whitelist` 通用白名单机制(submodule 字段 + turn_loop `retain`)。blocklist 全局减法(建 catalog 时);tool_whitelist per-session `retain`(turn_loop 最后)。whitelist 在 blocklist 过滤后的集上 retain → **无法重新暴露黑名单工具**。⚠️ **app 层监工用法已删(2026-06-15,对话型监工废弃)**:`supervisor_tool_whitelist()` + `spawn_for_session` 施加 + 死代码 `build_engine_config_for_workflow` 均移除,**机制本身(submodule)保留待用,字段恒 None**;submodule `engine.rs:263` doc 仍有一处指向已删函数的悬空引用,待下次 sync 顺带清。
- **验证**:L1 subagent scenarios 真 vLLM 跑通(`subagent_compare_3_libs` 并行 3 agent / 487s);W1–W12 forkguard 指纹;行为层仅 W10 `engine_config_locks_critical_fields`
- 上游 PR:❌ pinvou3 专用(可复用上游 WhaleFlow 基础 crate,暂未迁)

### app 层 fork(不在 submodule —— override hook / bridge 注入,fork-guard 也守)
- **prompt 内容(单一来源,main #14 重构 2026-06-15)**:`resources/bundle/instructions.md` 是**唯一 pinvou3 prompt 来源**——宪法/裁决/`AUTHORITY_RECAP` 全折叠进 §底线 + 动态注入 `{{PINVOU3_MODEL}}`/`{{PINVOU3_DATE}}`(治"编时间");`bridge/bundle.rs` 只剩 Mode 块 + `LOCALE_PREAMBLE/CLOSER` zh+ja 短版(`AUTHORITY_RECAP=""`、base.md 留空 stub、`compose_static_layers` 丢 base 只剩 Mode,**Plan 模式仍按 mode 切**)。经 `set_*_override` + `set_static_prompt_composer_override` 注入。**submodule 内 prompt 文案 drift=0**。依据=ablation 实测(user memory `prompt-ablation-methodology`):base.md 对 Qwen3.6 可测价值仅 Voice;整 prompt 22590→16612B,剩余大头=Skills~52%(`~/.agents/skills` 全局 lark 技能,待重设计)
- **bridge config**(`bridge/mod.rs`):`subagent_api_timeout=300`、`max_subagents`(prefs 默认)、`network_policy` fake-ip CIDR(`198.18.0.0/15`)、`compaction.token_threshold=190_000`(256K×74%)、`InstructionSource::Inline`。v0.8.58-60 新字段(verbosity/interactive_launch_limit/goal_*/disallowed_tools)全透传 default
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

---

## 3. fork-guard 守护 + sync 后验证

```bash
./scripts/fork-guard.sh          # 全量:指纹层 + 编译跑回归测试
./scripts/fork-guard.sh --fast   # 仅指纹层,秒级(merge 后第一道快筛)
```

两层:**指纹层** grep 每个 fork 标记是否还在(抓「merge 静默丢整段 patch」);**行为层** `cargo test` 跑回归测试(抓「值/逻辑被改回上游」)。**43 指纹**(submodule C1-C7+W+P / app),完整清单见 `fork-guard.sh` `fingerprints=` 数组——新增 fork patch 必同步加指纹(见 fork-policy §3)。

### ⚠️ sync 后必做验证 checklist(fork-guard **不够**,每条都踩过坑)
1. **全量 lib 测试** `cargo test -p codewhale-tui --lib`——抓非 `forkguard_` 前缀的上游测试因 fork fail(v0.8.51 append_file 静默丢失靠此抓)
2. **dump_system_prompt 前后 diff**(不在 fork-guard 构建里)——非 0 就逐块查谁漏进静态 prompt(v0.8.57 Runtime Policy 141 行泄漏靠此抓)
3. **扫 per-turn message 构造路径** `grep -rn "runtime_prompt\|messages.push" turn_loop.rs engine.rs`——上游可能新增每请求注入的 transient 消息,dump 抓不到
4. **工具集合 + 激活机制盘点**:① 对比两版 `ToolSpec::name()` 集合,新工具漏入要补黑名单;② **更要查上游有没有新增能激活 deferred 工具的机制**(`tool_search`/`ensure_advanced_tooling` 类)——blocklist 是 defer 非删除,任何激活 deferred 的新路径都击穿门控
5. **hook 决策协议**:上游可能改 hook 退出码/JSON 契约(v0.8.60 Hooks v2 把 hard-deny 从「非零」改成「exit 2」)——dump/编译都抓不到,必须读 `fold_tool_call_before_results` 确认 deny 脚本退出码契约
6. **app 端单线程测试** `cargo test --manifest-path pinvou3-app/.../Cargo.toml --lib -- --test-threads=1`——bridge env 测试并行会 flake(非回归)

---

## 4. Sync 历史

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
