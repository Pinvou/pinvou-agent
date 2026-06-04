# DeepSeek-TUI Fork 修改清单

> 本文件集中记录 pinvou3 对 `DeepSeek-TUI` 底座的所有 fork 修改。
> 
> 目的:
> 1.  upstream PR 时快速定位改动点
> 2.  团队交接 / 新人 onboarding
> 3.  上游 sync 后检查 merge 是否静默丢失
>
> 格式: `[优先级]` 文件路径 + 行号范围 + 修改摘要 + 理由 + 是否适合提上游 PR。

> **2026-05-29 阶段2 prompt override 重构**:原 #28/#32/#33/#36/#37/#38(改 base.md / prompts.rs 品牌 + 事实层)已从"直接改 submodule"迁到 **override hook** —— submodule `base.md` 回退上游原文(0 diff),pinvou3 prompt 内容移到 `pinvou3-app/src-tauri/resources/bundle/base.md` + `bridge/bundle.rs` 常量,启动经底座 `set_*_override` 注入(新 patch #42)。详见 `docs/base-prompt-override-阶段2.md`。submodule 这部分 prompt fork drift = 0;下方组 3/组 4 描述的"改 base.md"已不在 submodule,仅内容落点变了。
>
> **2026-05-29 上游 PR 一批 → 05-31 大批合入**:override hook 机制(#42 配套)抽通用版提 **#2356**;另把 doc #3(subagent stop-on-failure)提 **#2354**、doc #13(fetch_url fake-ip 信任)提 **#2355**。三者均从 `upstream/main` 切净分支、零 pinvou3 字样泄漏。**05-31 owner 大批 merge**:#2354/#2355 连同早前 OPEN 的 #2245/#2311/#2313/#2314 全部合入,**OPEN 仅剩 #2356**。已合入的 fork patch 下次 sync 会被上游 harvest(漂移归零),按文件级 diff 确认。**PR 全量实时状态见 `docs/fork-policy.md §8`(单一真相源,本文件不再重复维护 PR 号)**。
>
> ⚠️ **下次 sync 待办**:`tools/search.rs`(file_search)的 spawn_blocking + 30s timeout fork patch 已被上游 **#2035**(merge `d22da53e`)用自家实现覆盖,fork 版与上游重叠 —— sync 时**撤回 fork 版留上游版**(配套 PR #2044/#1790 已 CLOSED)。

---

## 🧹 Clean re-fork (2026-06-04, 新分支 `pinvou3-clean` ← v0.8.53)

把积累的 36 个交错 fork 提交 + 160 commit 乱历史,**从 v0.8.53 干净起点重建为 6 个主题 commit**,只留仍 fork-distinct 的 patch。drift **+1844→+1243 / 34 文件**(submodule)。

**新历史(6 主题 commit)**:
1. `feat(lib)` lib facade(暴露内部模块供 app as-library)
2. `feat(tools)` pinvou3 blocklist 工具门控
3. `feat(tools)` append_file + 大产物保护(64KB 上限 / truncated_args_hint / SSE idle 遥测)
4. `feat(safety)` careful hook(多行逐行 + YOLO 也拦 Dangerous)
5. `feat(prompt)` GUI prompt/context/skills(project_context 砍空 + constitution 短路 + skills union/路径砍 + prompts embedder-agnostic)
6. `chore` 零碎适配(llm_client/lsp/hooks/gitignore)

**丢弃的 patch**:
- **subagent 本地约束全套**(MAX_STEPS/ELAPSED/resolve_agent_ref/Implementer-append_file)—— 实证 `agent_*`/`delegate` 全在 blocklist,**subagent 路径生产不可达**,等重做 subagent 再说。`tool_agent_route` 硬编码 deepseek-v4-flash 改为**提上游 PR**(通用 bug)。
- **phase/demo workflow 全删(跨仓)**—— submodule(PhaseDef/DemoInfo/strip_marker/PhaseChanged)+ app 后端(commands 四件套/ActiveSkillBinding/engine handler)+ app 前端(WorkflowView/PhaseChips/state.workflow/监听)。**专家卡牌(persona)+ plan_phase 独立,未碰**。workflow 后面重做。
- **qwen-128K**(models.rs 死码,真实模型 `qwen36_35b_256k` 走上游 hint 返 256K)。
- **fetch_url 残留测试**(33 行,测的全是已上游化 API)。

**已 harvest 指纹撤除**(v0.8.53 上游自带,非 fork-distinct):bing decode / network_policy fake-ip API / InstructionSource enum / base override hook / EngineConfig.instructions / 256K auto-compact / MAX_OUTPUT env。fork-guard 指纹从 ~32 → **22**(只剩真 fork patch)。

**验证**:fork-guard 全过(22 指纹 + codewhale-tui 6 + pinvou3-tauri 7 测试);底座 lib **3850 pass**;app 后端 lib **98 pass**;system prompt 与 re-fork 前逐字节一致。⚠️ app 前端(index.html)是 JS 未跑,需 run-dev 冒烟确认 UI。

**待办**:① `pinvou3-clean` 切为主分支 + push fork + 更新父仓 submodule 指针;② `pinvou3-clean-wip` 快照分支可删;③ `tool_agent_route` deepseek-v4-flash PR。

---

## 🔄 v0.8.53 同步后整理 (2026-06-04, submodule merge v0.8.53 → pinvou3-patches)

上游 v0.8.51→**v0.8.53** 同步 **40 commit**(14 fix / 9 feat / 6 docs)。**仅 1 个真实冲突**(`project_context.rs`,其余 14 个重叠文件如 engine.rs/turn_loop.rs/subagent 全自动合)。drift +1811→**+1844 / 41 文件**。0.8.53 是当前 Latest release(0.8.52 仅 tag 未 release,跳过直接对 53)。

**✅ 唯一冲突 `project_context.rs`(4 块)解法 —— 撞上组4-4c**:
上游 v0.8.53 引入整套 **`.codewhale/constitution.json` 仓库 authority 层 + WHALE.md 弃用**(新增 9 个常量 + `load_repo_constitution_block` 加载函数 + 6 个测试),与我们组4-4c(`PROJECT_CONTEXT_FILES`/`GLOBAL_PATHS` 砍空、不读其他 AI 工具配置)正面冲突。
- 块1/2(doc+常量)— **保我们空数组**;上游新 constitution 常量**保留**(标 `#[allow(dead_code)]`)让自动合进来的函数体编译通过。
- 块3(`load_global_agents_context`)— **保 HEAD** early-return(GLOBAL_PATHS 空)。
- 块4(测试)— 上游 2 个新全局多路径测试 + 原 1 个**全标 `#[ignore]`**(扫描路径砍空后不可达)。
- **🆕 新 fork patch:`load_repo_constitution_block` 短路 early-return**。该函数读 `<workspace>/.codewhale/constitution.json`,pinvou3 workspace=$HOME 场景会读 `~/.codewhale/constitution.json` —— **与 §5 禁令(~/.codewhale 禁读)直接冲突**,且 pinvou3 走 inline 注入不依赖 disk 项目配置。短路返回 `(None, [])`,保留函数体防回退。配套 fork-guard 指纹(锚点「v0.8.53 上游引入 `.codewhale/constitution.json`」)+ 上游 4 个 constitution/WHALE 测试标 `#[ignore]`。

**⚠️ 本次踩的坑**:
- **`EngineConfig` 新字段 `subagent_heartbeat_timeout: Duration`**(配 subagent lifecycle hooks feat):bridge 解构/init 透传 default。✅ **已评估安全,不需 override**:默认 `DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS=300`,**正好 = 我们 `subagent_api_timeout` override 值**;且配置层 `subagent_heartbeat_timeout_defaults_clamps_and_respects_api_timeout` 测试证实心跳被 clamp 到 ≥ api_timeout;进度在步骤边界/工具完成上报会重置心跳(`record_agent_progress`→`touch`),单步 LLM 调用受 api_timeout=300 封顶。唯一边缘风险:子 agent 内跑 >5min 的工具(全量 build)期间无进度上报会被掐,但 pinvou3 单子 agent + 长命令走 background,属边缘。
- **`PromptSessionContext` 新字段 `allow_shell: bool`**(v0.8.53,gate shell 工具是否进 prompt):`bin/dump_system_prompt.rs` 构造缺它 → **编译失败**。⚠️ **fork-guard 盲区**:dump bin 不在 fork-guard 的 lib-test 构建里,只有真跑 dump 才暴露。已补 `allow_shell: cfg.allow_shell`(复刻生产)。**sync 后除 fork-guard 还要跑一次 `dump_system_prompt` 确认 prompt 工具链没断。**
- **✅ prompt 零漂移**:v0.8.53 虽改 prompts.rs/engine,dump 出的 pinvou3 system prompt 与 v0.8.51 **逐字节一致(324 行/21122 字节)**。
- **flaky 测试**:`mcp::tests::legacy_sse_closed_stream_reconnects_and_retries_tool_call` 全量并行跑偶发 FAILED,单独跑 ok —— 时序/资源争用环境噪音,非 fork/merge 问题(同 v0.8.47 记的 locale 测试性质)。

**🧪 本次 feat 三处单测验证(无 vLLM 端到端,跑单元层)**:subagent lifecycle hooks(5)+ hooks 模块含 careful hook fork(41)+ subagent 模块全量含所有 subagent fork patch(122)+ models 模块含上游模型族分类与 qwen-128K patch 共存(12)+ mode-change runtime message(3)+ bridge 窗口识别(1)—— **全过**。端到端冒烟(子 agent 行为/GUI)仍需起 app + vLLM(L1 harness)。

**✅ 验证**:fork-guard 全过(30 指纹含新增 constitution 短路 + 12 + 7 测试);底座全量 lib **3862 pass / 0 fail / 35 ignored**(31→35,+4 constitution/WHALE)。

**🔭 与本次 feat 相关、需冒烟关注**:subagent lifecycle hooks(hooks.rs + hook_executor)、classify model families(models.rs ↔ qwen-128K patch)、mode-change runtime message(engine.rs)。

---

## 🔄 v0.8.51 同步后整理 (2026-06-03, submodule merge v0.8.51 → pinvou3-patches)

上游 v0.8.49→**v0.8.51** 同步 **118 commit**(61 fix 主导 / 13 feat / 14 test / 14 docs)。⚠️ 注意 0.8.50/51 是**独立 release tag,不在 origin/main 上**(main 仍停 v0.8.49+CI),merge 直接对 `v0.8.51` tag。**仅 4 个内容冲突**(其余大改文件如 engine.rs +455 / shell.rs +206 / turn_loop.rs +209 / subagent +97 / llm_client +271 全靠 3-way 自动合)。drift +1796→**+1811 / 41 文件**(基本持平)。

**✅ 4 个冲突解法**:
- `prompts.rs` + `prompts/modes/agent.md`(Context Management)— **取 HEAD**:上游重新写回 `/compact`/`Ctrl+L`/auto_compact 终端细节,与组3 embedder-agnostic 主旨冲突,保我们的通用措辞。
- `tools/registry.rs`(import)— **两行都留**:HEAD `use pinvou3_blocklist`(组1)与上游新 `use schema_canonicalize`(byte-level schema 规范化 feature)互不排斥,缺一编译失败。
- `tui/widgets/tool_card.rs`(tool_family map)— **合并双方**:保 fork 的 `append_file`→Patch,纳入上游新增 `exec_shell_cancel`/`task_shell_start`/`task_shell_wait`→Run。

**⚠️ 本次暴露/踩的坑**:
- **上游 cycle removal**(release 主题之一):`cycle_manager` 模块 + `EngineConfig.cycle` 字段整体删除。① submodule `lib.rs` 残留孤儿 `pub mod cycle_manager;`(lib.rs 是 fork 文件、上游只有 main.rs,3-way 没自动删)→ 编译失败,**删行**。② bridge `EngineConfig { cycle: CycleConfig{enabled:false,..} }` 显式关 cycle 的逻辑失效 → **删除**(原目标"关 cycle 防小窗口误触发"已由上游删子系统达成,自然作废)。
- **`CompactionConfig.auto_floor_tokens` 删除**(floor 概念随 cycle removal 一并去掉,新结构仅 `enabled/token_threshold/model/cache_summary`):bridge init 的 `auto_floor_tokens: 60_000` + 回归测试 `floor < threshold` 断言**双删**;`token_threshold: 190_000`(256K×74%)patch 仍活。
- **`EngineConfig` 新字段 `speech_output_dir`/`hook_executor`** + **`Op::SendMessage` 新字段 `hook_executor`**:bridge 解构/init/SendMessage 三处透传 default(None)。这是固定的「上游新字段维护成本」。
- **🩹 append_file 静默丢失找回(组5)**:merge 取上游 `SubAgentType::Implementer.allowed_tools()` 丢了 fork 的 `append_file` 条目,但 fork 加的测试断言 `test_implementer_allowed_tools_include_writes` 存活把它抓出。**已恢复 append_file 进 Implementer 列表 + 加 fork-guard 指纹(锚点「[pinvou3-fork 組5] append_file」)**。⚠️ **盲区教训**:该测试非 `forkguard_` 前缀 → fork-guard 不跑它,只有跑底座**全量 lib 测试**才暴露;实际 v0.8.49 起 allowed_tools 就已无 append_file(此前未跑全量未察觉)。**sync 后必跑 `cargo test -p codewhale-tui --lib` 全量,别只信 fork-guard。**
- **新上游模块**:`tools/{schema_canonicalize,speech,verifier}.rs` 已在 `tools/mod.rs` 声明(自动合 OK);`acp_server` + 9 个 `*_tests` 模块是 bin-only(main.rs 专属),lib.rs 正确不列。

**✅ 验证**:fork-guard 全过(29 指纹含新增 append_file + codewhale-tui 12 + pinvou3-tauri 7 测试);底座全量 lib **3799 pass / 0 fail / 29 ignored**。

**🗑️ 本次自然作废的 fork patch**(上游 harvest / 删子系统):cycle 关闭逻辑、auto_floor_tokens 下限 —— 两者对应的上游字段/子系统已被删,fork 代码同步删除,drift 不增。

---

## 🔄 v0.8.49 同步后整理 (2026-06-02, merge `31adba22`,submodule HEAD `2267f53c`)

上游 v0.8.47→v0.8.49 同步 **367 commit**。冲突 6 文件 16 标记,无困难冲突。drift 从 v0.8.47 基线 +2127 降到 **+1796 / 40 文件**(6 个 PR 被 harvest)。

**✅ 上游已 harvest(fork 版作废,merge 时取上游侧)**:
- 我们提的 6 个 PR 全部合入 v0.8.49:#2245 bing decode、#2311 InstructionSource、#2313 Tier5、#2314 environment-volatile、#2354 subagent stop-on-failure、#2355 fetch_url fake-ip。
- **prompt override hook**:上游自己实现并扩展(我们 PR #2356,上游扩成 10 个 `set_*_override`,签名变 `Result<(), String>`)。fork submodule 侧取上游版,**app 层 install_prompt_overrides 适配新签名**(`let _ =` 忽略幂等重复 set 的 Err)。
- #40 environment block 移 volatile:上游 harvest,fork-guard 指纹撤除。

**⚠️ 本次 merge 暴露/踩的坑**:
- **`--theirs` 误覆盖**:解 subagent/mod.rs 单点冲突时对整文件取了上游版,冲掉 4 个不在冲突区的 fork-distinct patch(#1 MAX_STEPS=20 / #2 ELAPSED=300 / #4 resolve_agent_ref 截断 / #7 tool_agent_route 继承父 model)。靠 fork-guard 指纹层抓出,逐个重施。**教训:整文件 `--theirs` 危险,fork-distinct 文件必须逐 hunk 解。**
- **lib.rs export 维护成本**:上游新增 7 模块需手动补 `pub mod`(purge/slop_ledger/workspace_discovery/shell_dispatcher/prompt_zones/session_failure_classifier + 其一);`acp_server` 依赖 bin 专属 `resolve_cli_auto_route` 不能进 lib。
- **EngineConfig/Op 新字段**:v0.8.49 加 `allowed_tools`/`tools`(EngineConfig)、`allowed_tools`(Op::SendMessage)、`tool_catalog`/`base_url`(Event::TurnComplete)。bridge 透传 default,engine.rs 事件解构用 `..` 忽略。
- **Skill{phases,demo} 撞车**:v0.8.49 在 runtime_api.rs 测试新增 Skill{} 构造,缺 fork 的 phase/demo 字段 → lib test 编译失败,补默认值。
- **app override 曾被发版误删**:`e702a02`(同事发版)为过 build 删了整套 app override,本次 sync 一并恢复。详见父仓 commit `7c813b8`。

**✅ fork-distinct patch 全部验证存活**:fork-guard 全过(指纹层 0 缺失 + codewhale-tui 12 测试 + pinvou3-tauri 7 测试)。子 agent 预警的语义风险点(tool_catalog blocklist / skills_dir union / file.rs 64KB / tool_agent_route)均由对应 forkguard_ 测试守住通过。

### fork commit 死/活快照(截至 v0.8.49,共 35 个非 merge commit)

> 快照随每次 sync 变动。"死"= 功能已被上游 harvest,merge 取上游版,commit 留历史但净 drift=0,**无需再守护**;"活"= 仍 fork-distinct。**净 drift +1796/40 文件,全在"活"集。**

**💀 已 harvest(死,11 个)** —— 下次评估可从守护清单移除:
`93e94741` MAX_OUTPUT_TOKENS env · `9af2ee97` InstructionSource enum · `5f847284` override hook(上游自带并扩展)· `9ef4a1d6` fetch_url fake-ip · `8d6d461d` bing decode · `15244e66` stop-on-failure · `363dd35a` role system→user · `7e5288e3` 256K auto-compact · `aaa19202` grep_files timeout · `079a3bb6` file_search timeout · `7514ec8e` brand 回退(已被 af64e9f7 回退)

**🟢 仍活(22 个)** —— 6 大主题,这才是真维护负担:
1. **工具表 blocklist**:`b9b40ce7`/`36526ce1`/`1ba8e418`/`44372248`/`032973b5`/`b776189e`/`d264542b`(pinvou3_blocklist.rs + tool_catalog defer)
2. **phase/demo workflow**:`f5c20678`/`357a2ace`/`2267f53c`(skills phases+demo + turn_loop strip_marker + events)
3. **subagent 本地约束**:`aab9cab8` + MAX_STEPS=20/ELAPSED=300(继承父 model / resolve_agent_ref 截断)
4. **careful 安全 hook**:`a25352a1`(多行逐行)/`b2f6ef56`(YOLO 全拦)
5. **file 大产物**:`0526dc57`(append_file)/`ebe58b8d`/`ade944db`(64KB+遥测)/`63e17d77`(truncated_args_hint)
6. **library 暴露 + project_context**:`6ac5b976`/`47e6abcd`/`dd879db8`(lib pub mod)/`9ae23c70`(PATHS 砍空)

**🟗 半死半活(2 个)**:
- `af64e9f7`:override 注入逻辑死(上游自带 hook),但 base.txt 的 append_file 列表项 + modes/compact 等 prompt md 精简编辑仍活。
- `bf048a7c`:守的 fetch_url fake-ip 已 harvest,但 forkguard 测试本体还在 —— **下次可删 `forkguard_validate_dns_resolved_ip_allows_fakeip` 测试**(特性已上游化)。

---

## 🔄 v0.8.47 同步后整理 (2026-05-27, merge `44844fa6`)

上游 `origin/main` 同步 123 commit (v0.8.45→v0.8.47)。逐 patch 验证存活,结论:

**✅ 上游已 harvest(fork 版消失=零漂移,下列条目作废,无需再维护)**:
- **subagent role system→user** (原 PR #2057) — 上游已合并,`turn_loop.rs` 用上游注释,代码一致。
- **`context_input_budget` 按窗口分级** (阶段 H B2) — 上游采纳。
- **`_Nk` hint 全 vendor** (阶段 H B1) — 上游采纳并 rename `deepseek_context_window_hint`→`explicit_context_window_hint`。
- **`DEEPSEEK_MAX_OUTPUT_TOKENS` env override** — 上游已具备。
- (早先) `file_search`/`grep_files` timeout、OpenAI streaming batch tool_calls — 历史已收敛。

**✅ fork-distinct patch 全部验证存活**(merge 未静默丢失):subagent(MAX_STEPS=20 / ELAPSED=300 / resolve_agent_ref / tool_agent_route / stop-on-failure)、web_search bing decode、fetch_url+network_policy fake-ip IP 段信任、file.rs 64KB+append_file+truncated_args_hint、chat.rs SSE 诊断、bridge 全部。

**⚠️ 本次 merge 暴露的 fork 维护点**:
- 文件路径变更:上游把 write/append/edit 合进 **`tools/file.rs`**(原 `write_file.rs`/`append_file.rs` 不再独立)。
- `tool_catalog.rs`:上游把工具 deferral 从 pinvou3 的 **blocklist 模型**(显示全部、隐藏黑名单)改成 **allowlist**(`DEFAULT_ACTIVE_NATIVE_TOOLS` 白名单、其余 defer)。philosophy 相反 → 静默踩坑:`request_user_input`/`append_file` 不在上游白名单被 defer,**GUI 里 request_user_input 不出气泡**。修复:新增 `pinvou3_should_defer_native_tool`(Yolo 只 defer 黑名单、其余全显示;非 Yolo 才叠加上游 allowlist),`should_default_defer_tool` 保持上游纯逻辑。回归测试 `pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default` 锁死,防下次 sync 再踩。
- `lib.rs`:上游新模块需手动加 `pub mod`(本次补了 `tool_output_receipts`)。这是 fork lib-export patch 的固定维护成本。
- bridge:上游 `EngineConfig` 新增 `show_thinking`/`goal_state`/`tools_always_load`/`prefer_bwrap`,已透传 default。
- ⚠️ 上游 **#2132 默认搜索后端 Bing→DuckDuckGo**;pinvou3 bing decode 修复仍在(Bing 仍是可选后端),PR #2245 紧迫性下降但仍有效。pinvou3-app 通过 bridge prefs 显式覆盖 search_provider(默认 Bing,GUI 可切 Metaso/Bocha),所以底座默认是 DDG 或 Bing 对 pinvou3 用户不影响 —— 这是编排层职责,不污染底座。

**注**:1 个测试 `system_prompt_skips_locale_preamble_for_english` 在本机失败 = 全局中文 skills(lark-*/h3c-ppt)注入 prompt 触发,**环境噪音非代码问题**,CI 干净环境通过。

---

## 目录

- [Subagent 模块](#subagent-模块)
- [联网工具链](#联网工具链)
- [文件工具](#文件工具)
- [Bridge / 配置](#bridge--配置)
- [SSE / 流处理](#sse--流处理)
- [上下文管理](#上下文管理)
- [其他 GUI 适配](#其他-gui-适配)

---

## Subagent 模块

### `crates/tui/src/tools/subagent/mod.rs`

| # | 行号 | 修改 | 理由 | 上游 PR? |
|---|---|---|---|---|
| 1 | ~67 | `DEFAULT_MAX_STEPS` 100→**20** | 防弱模型死磕 17min;single-task 够用 | ⚠️ 环境相关 |
| 2 | ~70 | `DEFAULT_SUBAGENT_ELAPSED_MAX` 硬上限 **300s** | 防 subagent 内部死磕反复重试 | ⚠️ 环境相关 |
| 3 | ~5028 | `GENERAL_AGENT_INTRO` 增加 **stop-on-failure** + **bounded effort** | 弱模型反复重试同一失败工具,浪费步数 | ✅ **#2354 已 MERGED**(05-31,下次 sync 归零) |
| 4 | ~1419 | `resolve_agent_ref()` 截断容错: LLM 截断 `"agent_"` 前缀时自动补回 | Qwen3.6 偶发截断 agent_id;单 subagent 也必需 | ✅ 通用,已验证 |
| 7 | ~4505 | `tool_agent_route()` 继承父 session `runtime.model.clone()` | 原硬编码 `"deepseek-v4-flash"` 本地 vLLM 无此模型→404;单 subagent 必需 | ✅ 通用 bugfix |

> ⚠️ **2026-05-27 弃用**(原 #5/#6 + max_steps→12 + elapsed→600 + harness 1260):多 subagent 并行 fan-out 废弃后,为 fan-out 调的 budget prompt(`build_assignment_prompt` step budget、`agent_open` "Keep it SIMPLE"、`GENERAL_AGENT_INTRO` 8-call 硬预算)全部回退 committed——它们会掐死单 subagent 长任务。根因/详情见 `process.md` Lesson learned 2026-05-27。

### `pinvou3-app/src-tauri/tests/l1_dialog_harness.rs`

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 8 | `spawn_for_scenario` 跟随 bridge 默认 `max_subagents` | 避免测试与生产行为不一致 | ✅ 测试修复 |

---

## 联网工具链

### `crates/tui/src/tools/web_search.rs` (底座内部)

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 10 | `normalize_bing_url` 先 `decode_html_entities` 再提取 `u=` 参数 | bing /ck/a 重定向 href 用 `&amp;` 实体编码,直接 regex 取 `u=` 为空→默认后端恒返 0 | ✅ **#2245 已 APPROVE+CI 全绿,待 owner merge**(05-28) |

> ⚠️ **2026-05-27 弃用**(原 #11/#12 网络重试 retry + timeout 15s→30s):env-specific 且两次 L1 重跑未被触发,回退 committed。

### `crates/tui/src/tools/fetch_url.rs` (底座内部)

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 13 | `validate_dns_resolved_ip` 放行落在 **fake-ip CIDR** 内的解析 IP(`is_trusted_fakeip_addr`) | clash/TUN fake-ip 下域名全解析到 198.18.x 占位段被 SSRF 误杀。**按 IP 段信任**(替代早期 `proxy=["*"]`,后者会放行任意域名→内网 SSRF);真实私网/loopback/元数据仍拦 | ✅ **#2355 已 MERGED**(05-31;PR 版做成 opt-in 可配置 CIDR,默认空=无行为变化,下次 sync 归零) |

---

## 文件工具

### `crates/tui/src/tools/file.rs` (底座内部;v0.8.47 起 write/append/edit 合并于此)

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 14 | content **64KB 硬上限**(超限返 `InvalidInput` + 引导骨架+分块) | 大产物生成静默 >240s → SSE timeout → 流截断 | ✅ 通用保护 |
| 15 | `truncated_args_hint`: 流截断缺字段时回"参数被截断请分块" | 替代干巴巴 missing_field,掐断 loop_guard 原样重试 | ✅ 通用体验改善 |

---

## Bridge / 配置

### `pinvou3-app/src-tauri/src/bridge/mod.rs`

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 16 | `subagent_api_timeout` 120s→**300s** | 本地 vLLM 慢推理,复杂 prompt 生成常 >120s;单 subagent 必需 | ⚠️ 环境相关 |
| 17 | `max_subagents` 默认 **1**(= 上游默认) | 2026-05-27 定论:多/并行 fan-out 弱模型不可用(编排认知,非工具/后端),只留单+串行。曾试 4 已回退 | ✅ 与上游一致 |
| 18 | `network_policy` 从 `None` 改 `Some(decider)`:`default=Allow` + **`with_trusted_fakeip_cidrs(["198.18.0.0/15"])`** | 按 IP 段信任 fake-ip 占位段(配合 #13);替代早期 `proxy=["*"]`(SSRF) | 🔵 配 #13 |
| 27 | `boot()` 末新增 `write_pinvou3_workspace_context_if_needed()`,往 `~/.codewhale/instructions.md` 写 12 行 pinvou3 精简版(覆盖底座 auto-gen / 不动用户自定义) | P0-2: `workspace=$HOME` + 底座 `auto_generate_context` 会自动 dump 500 行 $HOME 目录树到 prompt(暴露 `~/.ssh/id_ed25519` 等敏感路径名,与 instructions §8 禁令直接冲突;1145→662 行 -30%) | ❌ pinvou3 专用(workspace=$HOME 是 pinvou3 GUI 假设,上游典型场景 workspace 是 git repo,不会触发) |

> 配套:`crates/tui/src/network_policy.rs` 新增 `with_trusted_fakeip_cidrs` / `is_trusted_fakeip_addr` + IPv4 CIDR helper(无新依赖)。

### `pinvou3-app/src-tauri/src/bridge/prefs.rs`

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 19 | `AdvancedPrefs` 增加 `max_subagents: Option<usize>` + `max_steps: Option<u32>` | 让用户可调,当前 GUI 未暴露 | ✅ 通用功能 |

---

## SSE / 流处理

### `crates/tui/src/client/chat.rs` (底座内部)

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 20 | SSE idle timeout 错误带 `bytes_received`/在途 tool_use buffer 诊断 | 区分 prefill 静默 vs 参数中途断 | ✅ 通用诊断增强 |
| 21 | `stream_open_timeout` 45s→**180s** | 本地 vLLM 首 token 慢 | ⚠️ 环境相关 |

### `pinvou3-app/src-tauri/src/bridge/mod.rs` (bridge 层)

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 22 | 思考指示器修复: `Event::Error` 按 `recoverable` 分流 | 瞬态错误(SSE idle timeout)被当 turn 结束 → 前端 busy=false 但引擎仍跑 | ✅ 通用 bugfix |

---

## 上下文管理

### `crates/tui/src/agent/session.rs` / `context.rs` (底座内部)

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 23 | `context_input_budget` 按窗口分级(256K 下不等于 1M 默认值) | 256K 窗口下底座 4 个子系统静默退化,会话涨到 max_model_len 撞墙 | ⚠️ 模型相关 |
| 24 | `_Nk` hint 全 vendor(不仅 DeepSeek) | 本地 vLLM 也需 compact 预算 hint | ✅ 通用功能 |

---

## Prompt 拼装 / Skills 注入

> 背景: 来自系统 prompt 质量评估(`docs/system-prompt-架构.md`),把 1144 行 prompt 压到 571 行(-50%)。原则:**只改事实层**(model/render target/工具列表/sub-agent cap/品牌字样/扫描路径),**不动哲学层**(Constitution 7 articles / Statutes / Personality / Hierarchy)。终态 dump:`/tmp/pinvou3_system_prompt_v10.txt`。
>
> 14 个 patch (#25-#41) 按 6 个逻辑单元归组。原 # 编号保留(便于上游 sync 时 cherry-pick),代码 / 测试 / fork-guard 指纹照旧。

### 组 1 — Skills 注入 union 入口 ✅ (原 #25 + #26)

| 项 | 内容 |
|---|---|
| 文件 | `crates/tui/src/skills/mod.rs` + `crates/tui/src/prompts.rs:757-769` |
| 改动 | `skills/mod.rs` 新增 pub `render_available_skills_context_for_workspace_and_dir(workspace, skills_dir)`;`prompts.rs` skills_block 拼装由 `for_workspace(...).or_else(skills_dir...)` 改成 union(`for_workspace_and_dir`) |
| 理由 | 上游 `or_else` fallback 永不触发——workspace=$HOME 时 `discover_in_workspace` 总返回 Some(home-rooted skills),pinvou3 注入的 `EngineConfig.skills_dir` 形同虚设,bundle 内 `pinvou-review-plan`/`pinvou-review-final` AI 全程看不见 |
| 上游 PR | ✅ 通用 bugfix(任何用 `skills_dir` 的 embedder 同样受影响) |
| 测试 | `forkguard_skills_dir_unions_with_home_rooted_workspace_skills` |

### 组 2 — Tier 5 覆盖 EngineConfig.instructions ✅ (原 #28)

| 项 | 内容 |
|---|---|
| 文件 | `crates/tui/src/prompts/base.md:55` (Article VII Tier 5) |
| 改动 | 加 "any file configured via `EngineConfig.instructions`" 显式条款 + 声明 EngineConfig.instructions imperatives 属 Local Law(非 Tier 7 Memory) |
| 理由 | pinvou3 instructions 路径 `~/.pinvou3/sessions/*/instructions.md` 不在原始 4-file 名单(AGENTS.md / CLAUDE.md / .codewhale/instructions.md / .deepseek/instructions.md),内容又是祈使句,会被底座 Article VII 默认贬到 Tier 7 Memory(用户一句话可 override) |
| 上游 PR | ✅ 通用 API 缺口(任何用 EngineConfig.instructions 的 embedder 都受益) |
| 测试 | `forkguard_local_law_tier_covers_engine_config_instructions` |

### 组 3 — BASE_PROMPT 事实层剪裁 ❌ (原 #29-#35) — **阶段2 起内容在 pinvou3-app/resources/bundle/base.md,submodule base.md 已回退上游**

| 项 | 内容 |
|---|---|
| 文件 | `prompts/base.md` 多段 + `prompts.rs:773-789` inline + `prompts/modes/agent.md` |
| 删除 | `## RLM — How to Use It` 段 (20 行) + Tool Selection Guide `### rlm_open / rlm_eval` 段 + `## Your V4 Characteristics` 段 (12 行) + `## Toolbox (fast reference)` 段 (16 行,列 30+ 上游工具) + Context Management 里 "DeepSeek V4 thinking tokens" 句 |
| 改写 | `## Sub-Agent Strategy` (删 "$0.14/M / cap 10 / hard ceiling 20" → "concurrent cap is embedder-configured");`## Output Formatting` 反转 ("terminal-only" → "Match the embedder's render target");`## Composition Pattern` 抽象 (硬编码 `checklist_write` / `update_plan` → "use whichever planning tool the runtime exposes");`## Context Management` + `agent.md` 删 `/compact` slash command / `cache hit %` 角标 / sidebar 等 terminal UI 引用;`### agent_open / agent_eval / agent_close / tool_agent` 改成通用 `### Sub-agent tools (if exposed)` (删 `fork_context` / `Fin fast lane` / `Flash V4` 等 DeepSeek-specific 引导) |
| 理由 | BASE_PROMPT 是给 codewhale-tui 终端 + DeepSeek V4 + 30+ 工具齐全场景写的;pinvou3 = Qwen3.6 + Tauri GUI + ~10 工具,三维度全错位。AI 看到引导会试调不可用工具失败 |
| 上游 PR | ❌ pinvou3 专用(其他用底座的人可能用 RLM / V4 / 完整 toolbox) |
| 测试改造 | 上游 4 个回归测试因这些删/改必然 fail(断言 RLM/agent_eval/fork_context/Brother Whale 关键字存在),反向重写为 forkguard:<br>• `forkguard_tool_selection_guide_is_embedder_aware`<br>• `forkguard_rlm_section_removed_by_pinvou3`<br>• `forkguard_pinvou3_omitted_upstream_specific_tool_names_from_base_prompt`<br>• `forkguard_no_deepseek_specific_fork_context_prose_in_base_prompt` |

### 组 4 — P-brand cleanup: 品牌字样 + 路径列表清理 ❌ (原 #36 + #37 + #38 + 2026-05-28 扩展)

> 这一组覆盖所有 "去 codewhale 品牌" 改动:文本字串替换 + 路径清单砍 + Tier 5 段精简 + 配套 pinvou3 bridge 路径迁移。最终 prompt 里 `codewhale` 只剩 3 处 `<codewhale:subagent.done>` 协议 sentinel(hook 契约不可改)。

#### 4a — 文本字串替换 (原 #36 + #37 + #38) — **阶段2 起经 override 注入,内容在 pinvou3-app**

| 项 | 内容 |
|---|---|
| 文件 | **(阶段2)** `pinvou3-app/.../bundle/base.md` + `bridge/bundle.rs::{LOCALE_PREAMBLE_ZH_HANS,AUTHORITY_RECAP}`;submodule 三处常量已回退 codewhale,经 `set_*_override` 注入 |
| 改动 | (a) Constitution 标题 `CODEWHALE` → `PINVOU3` + 删 `### Preamble` 整段(5 行 Brother Whale 品牌诗),改成一句 `You are {model_id}, running inside pinvou3. Honor the user's trust through truth, clarity, and working code.`<br>(b) `LOCALE_PREAMBLE_ZH_HANS` "你正在 codewhale 中运行" → "你正在 pinvou3 中运行"<br>(c) `AUTHORITY_RECAP` "Constitution of CodeWhale" → "Constitution of pinvou3" |
| 理由 | pinvou3 是独立产品;模型自我认知应标识 pinvou3 运行环境而非底座品牌 |
| 测试 | `forkguard_constitutional_preamble_uses_pinvou3_branding` |

#### 4b — Tier 5 段精简 (2026-05-28)

| 项 | 内容 |
|---|---|
| 文件 | `prompts/base.md:55` (Article VII Tier 5) |
| 改动 | 删裸暴露的品牌路径名 `AGENTS.md, CLAUDE.md`, `.codewhale/instructions.md`, `.deepseek/instructions.md`,改成 "files configured via `EngineConfig.instructions` plus any workspace-rooted instructions file the runtime discovers"。语义保留(EngineConfig.instructions 仍升 Tier 5 Local Law),只删品牌列举 |
| 理由 | pinvou3 用户不该在 prompt 里看到其他 AI 工具品牌路径 |
| 测试 | `forkguard_local_law_tier_covers_engine_config_instructions`(扩展为同时断言品牌路径不裸列) |

#### 4c — PROJECT_CONTEXT_FILES + GLOBAL_PATHS 砍到 1 (2026-05-28)

| 项 | 内容 |
|---|---|
| 文件 | `project_context.rs` (`PROJECT_CONTEXT_FILES` + `GLOBAL_PATHS` + `auto_generate_context`) |
| 改动 | (a) `PROJECT_CONTEXT_FILES` 6 路径(`WHALE.md`/`AGENTS.md`/`.claude/instructions.md`/`CLAUDE.md`/`.codewhale/instructions.md`/`.deepseek/instructions.md`)→ 只剩 `.pinvou3/workspace_context.md`<br>(b) `GLOBAL_PATHS` 4 路径(`~/.codewhale/AGENTS.md` 等)→ 空数组,`load_global_agents_context` early return None<br>(c) `auto_generate_context` 配套对齐:检查目标 + 写盘路径都改 `.pinvou3/workspace_context.md` |
| 理由 | pinvou3 不识别其他 AI 工具的全局/workspace 配置(`~/CLAUDE.md`/`~/AGENTS.md` 等);只用 pinvou3 自家路径 |
| 上游 PR | ❌ pinvou3 专用 |
| 测试 | 底座原 11 个相关 test 用 `#[ignore = "pinvou3 fork (P-brand cleanup): ..."]` 标记保留(便于上游 sync cherry-pick) |

#### 4d — pinvou3 bridge 路径迁移 + legacy 清理 (2026-05-28)

| 项 | 内容 |
|---|---|
| 文件 | `pinvou3-app/src-tauri/src/bridge/mod.rs::write_pinvou3_workspace_context_at` (扩展原 #27) |
| 改动 | (a) 写盘路径从 `~/.codewhale/instructions.md` → `~/.pinvou3/workspace_context.md`(配套 4c PROJECT_CONTEXT_FILES 唯一一条路径)<br>(b) 同时清理 legacy `~/.codewhale/instructions.md` + `~/.deepseek/instructions.md`(仅清 auto-gen 残留 / pinvou3 早期写的版本,用户自定义保留) |
| 理由 | 配套 4c;同时清干净用户 $HOME 上的早期路径残留 |
| 测试 | `forkguard_writes_pinvou3_workspace_context_to_codewhale_instructions`(扩展为 4 case:含 legacy 清理验证) |

### 组 5 — Environment block 精简 + 移到 volatile 区 ⚠️ (原 #39 + #40)

| 项 | 内容 |
|---|---|
| 文件 | `prompts.rs::render_environment_block` + 渲染位置(原 #2.25 → #6,紧邻 instructions block) |
| 改动 | (a) 删 `- lang:` 字段(跟 locale_preamble/closer/pinvou3 §1 三处冗余)<br>(b) 删 `- codewhale_version:` 字段(显示 codewhale-tui crate 版本而非 pinvou3-app 版本,误导)<br>(c) 渲染位置从 volatile boundary **上方**移到**下方** |
| 理由 | 上游注释假设"workspace fixed for the run"对 codewhale-tui 终端成立、对 pinvou3 多 session 不成立(pwd 含 session_id 跨 session 漂移)。移到 volatile 区让静态 prefix 跨 session byte-stable,prefix cache 命中率提升 |
| 上游 PR | ⚠️ #40(移位)通用价值适合 PR;#39(删字段)pinvou3 专用 |
| 测试 | `render_environment_block_lists_supplied_locale_and_workspace`(改造为反向断言确认 lang/codewhale_version 不存在) |

### 组 7 — InstructionSource enum (C 方案 P-no-disk) ⚠️

> 配套去 codewhale 品牌后的最后一步:消除 `~/.pinvou3/sessions/<sid>/instructions.md`
> disk 文件。pinvou3 instructions 之前必须先渲染写盘再传 `Vec<PathBuf>` 给底座 —— disk
> 是底座 `EngineConfig.instructions: Vec<PathBuf>` API 设计假设的副作用。改成 enum
> 后 pinvou3 用 `Inline` 直接传内存字符串,底座 `render_instructions_block` 区分两种
> variant(File 走 disk 兼容上游,Inline 走内存)。

| 项 | 内容 |
|---|---|
| 文件 | `prompts.rs` (enum 定义 + render 改造) + `core/engine.rs` (EngineConfig 字段) + 7 处 CLI/TUI/runtime call sites (`.into()` 升级) |
| 改动 | (a) `prompts.rs` 新增 `pub enum InstructionSource { File(PathBuf), Inline { name, content } }` + `From<PathBuf>` impl<br>(b) `EngineConfig.instructions: Vec<PathBuf>` → `Vec<InstructionSource>`<br>(c) `render_instructions_block` 区分两 variant: File `std::fs::read_to_string`, Inline 直接用 content,共享 truncate/skip-empty 逻辑<br>(d) 上游 CLI/TUI/runtime 路径 7 处 call sites 加 `.into_iter().map(Into::into).collect()` 升级 |
| 配套 pinvou3 | `bridge::session_instructions(sid)` 返回 `Vec<Inline>`(rendered INSTRUCTIONS_MD with placeholder substituted);`bridge::instructions()`(legacy headless 路径)同样改 Inline;**删** `write_session_instructions` / `session_instruction_paths`;**删** engine.rs spawn_for_session 里的写盘调用;sync_session 改 `system_prompt: None`(让底座从 EngineConfig.instructions 自动重拼,不再被 rehydrate disk 覆盖);boot 时一次性清 `~/.pinvou3/sessions/*/instructions.md` legacy 残留 |
| 收益 | (1) disk 上不再有 `<sid>/instructions.md` 用户能看到 / 修改无效的文件;(2) multi-engine 并发不再依赖 per-session disk file 避免 rehydrate race,内存对象天然隔离;(3) sudo permission 改动后用户重启会话生效(原 disk 热刷路径走掉,接受) |
| 上游 PR | ✅ 通用 API 改进(任何 embedder 想注入运行时计算的 instructions 都受益,不只 pinvou3) |
| 测试 | 底座 5 个 `render_instructions_block_*` 测试改用 `.into()` 走 File variant;新加 `forkguard_render_instructions_block_handles_inline_source`(覆盖 Inline empty / oversize / mixed File+Inline 顺序);pinvou3 `engine_config_for_session_paths_are_isolated` 改成断言 Inline name + content 含 session_id;`workspace_orientation_guidance_present` 因 Tier 5 精简 fail 用 `#[ignore]` 标记 |

### 组 6 — Skills 扫描路径精简 ❌ (原 #41)

| 项 | 内容 |
|---|---|
| 文件 | `skills/mod.rs::skills_directories_with_home` |
| 改动 | 砍 10 条路径(6 个 workspace `<ws>/.agents/skills` / `skills` / `.opencode/skills` / `.claude/skills` / `.cursor/skills` / `.codewhale/skills` + 3 个 home `~/.claude/skills` / `~/.codewhale/skills` / `~/.deepseek/skills` + 1 个 fallback `/tmp/codewhale/skills`),只保留 `~/.agents/skills`。pinvou3 自带 skill 通过 `EngineConfig.skills_dir` 走组 1 的 union 入口注入 |
| 理由 | pinvou3 GUI 单 embedder 场景:workspace=$HOME → 10 条 home/workspace 路径全重叠;`.opencode`/`.cursor`/`.claude`/`.codewhale`/`.deepseek` 等多工具约定对 pinvou3 无意义 |
| 上游 PR | ❌ pinvou3 专用决策 |
| 测试 | `forkguard_skills_dir_unions_with_home_rooted_workspace_skills`(同时验证 `.deepseek/skills` 等不进);底座 7 个原版路径测试用 `#[ignore = "pinvou3 fork patch #41: ..."]` 标记保留,便于上游 sync 时对照恢复 |

---

## 其他 GUI 适配

### `pinvou3-app/src-tauri/Cargo.toml`

| # | 修改 | 理由 | 上游 PR? |
|---|---|---|---|
| 25 | `package="codewhale-tui"` 重命名保留 `deepseek_tui::` 别名 | 上游 rebrand 后 crate 名变了,pinvou3 代码大量用旧名 | ✅ 兼容层 |

---

## 快速核对表 (上游 sync 后使用)

**一条命令自动校验**(替代旧的手工 grep):

```bash
./scripts/fork-guard.sh          # 全量:指纹层 + 编译跑 fork 回归测试
./scripts/fork-guard.sh --fast   # 仅指纹层,秒级,不编译(merge 后第一道快筛)
```

两层防护:
- **指纹层** — grep 每个 fork 标记是否还在,抓「merge 静默丢整段 patch」(对应旧手工 grep,升级为带退出码 + 逐项 ✓/✗)。
- **行为层** — `cargo test` 跑精选 fork 回归测试,抓「值/逻辑被改回上游」(指纹 grep 抓不住,例:`tool_agent_route` 被改回硬编码 `deepseek-v4-flash` 时旧测试因 stub model 巧合而假阳性 → 新增 `forkguard_..._inherits_parent_model` 用区别 model 名真守住)。

> L1 vLLM dialog harness 慢且需后端,**不在** fork-guard 内,按需单独跑。

**fork 回归测试清单**(脚本里 `forkguard_` 前缀 + 显式列名;新增 patch 时同步加测试 + 指纹 + 本文档):

| patch | 测试 | crate |
|---|---|---|
| #1/#2 步数/墙钟上限 | `forkguard_subagent_step_and_elapsed_caps_match_local_budget` | codewhale-tui |
| #4 resolve_agent_ref 截断 | `resolve_agent_ref_tolerates_truncated_agent_prefix` | codewhale-tui |
| #7 tool_agent_route 继承父 model | `forkguard_tool_agent_route_inherits_parent_model_not_hardcoded_flash` | codewhale-tui |
| #10 bing 实体解码 | `bing_ckurl_with_html_entities_decodes_real_url` | codewhale-tui |
| #13 fetch_url fake-ip 放行 | `forkguard_validate_dns_resolved_ip_allows_fakeip_blocks_real_private` | codewhale-tui |
| #14 64KB 上限 | `test_write_file_rejects_oversized_content` / `..._append_..._` | codewhale-tui |
| #15 truncated_args_hint | `truncated_args_hint_fires...` / `..._skips_other_tools...` | codewhale-tui |
| #18a fake-ip CIDR | `trusted_fakeip_cidr_allows_placeholder_but_not_real_private` | codewhale-tui |
| tool_catalog blocklist | `pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default` | codewhale-tui |
| #18b bridge fake-ip 信任段 | `forkguard_network_policy_trusts_fakeip_range_only` | pinvou3-tauri |
| #16/锁定字段 | `engine_config_locks_critical_fields`(含 max_subagents=1 / timeout=300) | pinvou3-tauri |
| #23/#24 窗口识别 | `default_model_window_recognized_by_engine` | pinvou3-tauri |
| #25/#26 skills_dir union 不被短路 | `forkguard_skills_dir_unions_with_home_rooted_workspace_skills` | codewhale-tui |
| #27 workspace=$HOME 不被 500 行 dump | `forkguard_writes_pinvou3_workspace_context_to_codewhale_instructions` | pinvou3-tauri |
| #42 base override hook 存活(submodule) | 指纹 grep `set_base_prompt_override`(无专属测试,hook 失效则下方 app 端到端测试 fail) | codewhale-tui |
| #28/#29/#31/#36 内容(阶段2 迁 app) | `forkguard_base_override_has_pinvou3_content`(锚点+删项) | pinvou3-tauri |
| #37/#38 locale/authority 品牌(阶段2 迁 app) | `forkguard_locale_authority_override_branding` | pinvou3-tauri |
| 阶段2 override 端到端生效 | `forkguard_install_overrides_makes_compose_emit_pinvou3` | pinvou3-tauri |
| #39 environment 删 lang / codewhale_version | `render_environment_block_lists_supplied_locale_and_workspace`(反向断言) | codewhale-tui |

> 注:原 #29/#31/#36 的 6 个 submodule 内容 forkguard 测试在阶段2 已删除(base.md 回退上游,内容移 app),职责由上面 3 个 pinvou3-tauri 测试承接。

---

最后更新: 2026-06-04(v0.8.53 sync 完成;1 冲突 project_context.rs(组4 撞 constitution.json layer)+ load_repo_constitution_block 短路 + subagent_heartbeat_timeout 新字段;详见顶部 v0.8.53 章节。v0.8.51 章节见其下)
