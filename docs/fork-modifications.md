# DeepSeek-TUI Fork 修改清单

> 本文件集中记录 pinvou3 对 `DeepSeek-TUI` 底座的所有 fork 修改。
> 
> 目的:
> 1.  upstream PR 时快速定位改动点
> 2.  团队交接 / 新人 onboarding
> 3.  上游 sync 后检查 merge 是否静默丢失
>
> 格式: `[优先级]` 文件路径 + 行号范围 + 修改摘要 + 理由 + 是否适合提上游 PR。

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
- ⚠️ 上游 **#2132 默认搜索后端 Bing→DuckDuckGo**;pinvou3 bing decode 修复仍在(Bing 仍是可选后端),PR #2245 紧迫性下降但仍有效。

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
| 3 | ~5028 | `GENERAL_AGENT_INTRO` 增加 **stop-on-failure** + **bounded effort** | 弱模型反复重试同一失败工具,浪费步数 | ✅ 通用,可提 |
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
| 13 | `validate_dns_resolved_ip` 放行落在 **fake-ip CIDR** 内的解析 IP(`is_trusted_fakeip_addr`) | clash/TUN fake-ip 下域名全解析到 198.18.x 占位段被 SSRF 误杀。**按 IP 段信任**(替代早期 `proxy=["*"]`,后者会放行任意域名→内网 SSRF);真实私网/loopback/元数据仍拦 | 🔵 可提(碰 SSRF 信任边界,需先开 issue) |

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

### 组 3 — BASE_PROMPT 事实层剪裁 ❌ (原 #29 + #30 + #31 + #32 + #33 + #34 + #35)

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

#### 4a — 文本字串替换 (原 #36 + #37 + #38)

| 项 | 内容 |
|---|---|
| 文件 | `prompts/base.md` + `prompts.rs::LOCALE_PREAMBLE_ZH_HANS` + `prompts.rs::AUTHORITY_RECAP` |
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
| #28 Tier 5 cover EngineConfig.instructions | `forkguard_local_law_tier_covers_engine_config_instructions` | codewhale-tui |
| #29 RLM 段被删 | `forkguard_rlm_section_removed_by_pinvou3` | codewhale-tui |
| #31 Toolbox / agent_eval 字串被清 | `forkguard_pinvou3_omitted_upstream_specific_tool_names_from_base_prompt` | codewhale-tui |
| #31 fork_context: true / DeepSeek prefix-cache 被清 | `forkguard_no_deepseek_specific_fork_context_prose_in_base_prompt` | codewhale-tui |
| #31 Tool Selection Guide 改 embedder-aware | `forkguard_tool_selection_guide_is_embedder_aware` | codewhale-tui |
| #36 Constitution 改 PINVOU3 + 删 Brother Whale | `forkguard_constitutional_preamble_uses_pinvou3_branding` | codewhale-tui |
| #39 environment 删 lang / codewhale_version | `render_environment_block_lists_supplied_locale_and_workspace`(反向断言) | codewhale-tui |

---

最后更新: 2026-05-27
