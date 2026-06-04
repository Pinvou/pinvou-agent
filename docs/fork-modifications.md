# DeepSeek-TUI Fork 修改清单

> pinvou3 对 `DeepSeek-TUI` 底座所有 fork 修改的单一清单。
> 用途:① 上游 PR 定位改动点 ② 团队交接 / onboarding ③ sync 后检查 patch 存活。
>
> **当前基线**:submodule 分支 `pinvou3-clean` ← upstream **v0.8.53**
> (2026-06-04 clean re-fork,从 v0.8.53 干净起点重建为 6 个主题 commit)。
> 配套:`scripts/fork-guard.sh`(指纹 + 回归测试守卫)、`docs/fork-policy.md`(维护策略 + 上游 PR 状态单一真相源)。

---

## 0. 当前状态速览

- submodule 分支 **`pinvou3-clean`**(`.gitmodules` 追踪此分支;旧 `pinvou3-patches` 留 fork 上当备份)
- fork drift **+1258 / -315 行,34 文件**(submodule;app 层 prompt 内容走 override 注入,不计入此数)
- LLM 实际暴露 native 工具 **23 个**(= 全量注册 − blocklist;`mcp_pinvou_present_artifact` / `js_execution` 等走 MCP 另接)
- fork-guard:**22 指纹 + 回归测试**;底座 lib **3850 pass** / app 后端 lib **98 pass** / system prompt 与上游 sync 前逐字节一致

---

## 1. fork 结构(6 主题 commit ← v0.8.53)

> `git -C DeepSeek-TUI log --oneline v0.8.53..pinvou3-clean`。每个 commit 一个主题,只含仍 fork-distinct 的 patch。

### C1 `feat(lib)` library facade
| | |
|---|---|
| 文件 | `crates/tui/src/lib.rs`(整文件 —— **上游只有 `main.rs`,无 lib target**) |
| 改动 | 暴露内部模块为 library(`pub mod ...`)+ `#[cfg(test)] pub mod test_support`,让 pinvou3-app 以 `deepseek_tui::*` as-library 调用 + `cargo test --lib` 能跑 |
| ⚠️ 维护 | 上游每加/删模块需手动同步 `pub mod`(上游无 lib.rs,3-way 不会自动改它;v0.8.51 cycle removal 时残留孤儿 `pub mod cycle_manager` 即此坑)。`acp_server` 依赖 bin 专属符号不能进 lib |
| 上游 PR | ❌ pinvou3 专用 |

### C2 `feat(tools)` pinvou3 blocklist 工具门控
| | |
|---|---|
| 文件 | `tools/pinvou3_blocklist.rs`(新建)、`core/engine/tool_catalog.rs`、`tools/registry.rs`(to_api_tools hook)、`core/engine.rs`(re-export)、`tools/skill.rs` / `tui/command_palette.rs`(`#[ignore]`) |
| 哲学 | 上游 v0.8.47 起工具门控是 **allowlist**(`DEFAULT_ACTIVE_NATIVE_TOOLS`);pinvou3 相反 —— **显示全部、只隐藏黑名单**,给 Qwen3.6 精简到 ~23 工具 |
| 关键 | `pinvou3_should_defer_native_tool`:Yolo 只 defer 黑名单、其余全显示;非 Yolo 才叠加上游 allowlist。`request_user_input` 跨所有 mode 硬保留(否则 GUI 不出选择气泡);`image_analyze` 放出(Qwen3.6 有视觉)。`PINVOU3_BLOCKLIST_OVERRIDE` env 供 L1 harness 临时解锁 |
| blocklist 含 | v0.8.53 补漏:`speech`/`tts`/`github_close_pr`/`rlm_session_objects`/`run_verifiers`/`slop_ledger_*`(`checklist_*` 有意保留可见) |
| 测试 | `pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default` |
| 上游 PR | ❌ 哲学相反,pinvou3 专用 |

### C3 `feat(tools)` append_file + 大产物保护
| | |
|---|---|
| 文件 | `tools/file.rs`、`core/engine/dispatch.rs`、`core/engine/turn_loop.rs`、`client/chat.rs`、`tools/registry.rs`(`with_file_tools`)、`tui/approval.rs`、`tui/widgets/tool_card.rs`、`core/engine/tests.rs`、`tools/approval_cache.rs` |
| 改动 | `append_file` 工具(**上游完全没有**)+ content **64KB 硬上限** + `truncated_args_hint`(流截断缺字段→引导分块)+ SSE idle-timeout 遥测 + undo 快照纳入 append_file |
| 理由 | 本地慢 vLLM 大产物(PPT/长文档)>240s idle timeout 流截断;`write_file` 写 skeleton(≤8KB)→ `append_file` 追加 chunk(≤16KB)工作流 |
| 测试 | `truncated_args_hint_fires/skips_*`、`test_{write,append}_file_rejects_oversized_content` |
| 上游 PR | ❌ **评估后不提**:64KB cap / truncated_args_hint 与 pinvou3 专属 `append_file` 深度耦合(cap 同管 append_file、hint 引导 "build up with append_file");上游无 append_file,去耦后引导无落点。留 fork |

### C4 `feat(safety)` careful 安全 hook
| | |
|---|---|
| 文件 | `command_safety.rs`、`tools/shell.rs` |
| 改动 | (a) 多行命令**逐行分析取最严级别**(上游一刀切 Dangerous,误伤批量 `cp`/`mkdir`);`SafetyLevel` 加 `PartialOrd, Ord`。(b) Dangerous 命令在 **YOLO 模式也 BLOCKED**(上游 YOLO 跳过)—— 配合超级权限关闭态硬拦 sudo |
| 上游 PR | ❌ pinvou3 安全模型专用 |

### C5 `feat(prompt)` GUI prompt / context / skills
| | |
|---|---|
| 文件 | `project_context.rs`、`skills/mod.rs`、`prompts.rs`、`prompts/{modes/agent.md, compact.md, approvals/suggest.md, subagent_output_format.md, base.txt}`、`commands/skills.rs` |
| project_context | `PROJECT_CONTEXT_FILES`/`GLOBAL_PATHS` 砍空(workspace=$HOME GUI 助手,不读其他 AI 工具配置,context 走 inline 注入);`load_repo_constitution_block` 短路(v0.8.53 上游 `.codewhale/constitution.json` authority 层,与 §5 禁读 `~/.codewhale` 冲突) |
| skills | 扫描路径只留 `~/.agents/skills`(原 10 路径,#41);union 接线 `render_available_skills_context_for_workspace_and_dir`(上游 `or_else` 短路 bug 致 workspace=$HOME 时 bundle skills 不可见) |
| prompts/*.md | embedder-agnostic 措辞(去 `/compact`/`Ctrl+L`/`checklist_write` 硬编码,改 "whichever planning tool the runtime exposes");`compact.md` 加 "这是模板非真 handoff"(防 Qwen3.6 误读空模板) |
| 测试 | `forkguard_skills_dir_unions_with_home_rooted_workspace_skills`、project_context 多路径测试 `#[ignore]` |
| 上游 PR | skills union 接线 → **已提 [PR #2737](https://github.com/Hmbown/CodeWhale/pull/2737)**;其余 ❌ pinvou3 场景专用 |

### C6 `chore` 零碎 fork 适配
| | |
|---|---|
| 文件 | `llm_client/mod.rs`、`core/engine/lsp_hooks.rs`、`lsp/mod.rs`、`hooks.rs`、`core/turn.rs`、`tui/app.rs`、`.gitignore` |
| 改动 | 编译 / 接线层零碎适配(各 1-5 行) |

### app 层 fork(不在 submodule —— 通过 override hook / bridge 注入)

> 这些是 pinvou3-app 内的改动,不计入 submodule drift,但同属 fork 工程,fork-guard 也守。

- **prompt 内容**:`pinvou3-app/src-tauri/resources/bundle/base.md` + `bridge/bundle.rs`(Constitution `PINVOU3` 品牌 / `LOCALE_PREAMBLE` / `AUTHORITY_RECAP`),经上游 `set_*_override` hook 注入(hook 本身已上游化)。submodule 内 prompt 文案 drift = 0。
- **bridge config**(`bridge/mod.rs`):`subagent_api_timeout=300`、`max_subagents=1`、`network_policy` fake-ip CIDR 信任(`with_trusted_fakeip_cidrs(["198.18.0.0/15"])`)、`compaction.token_threshold=190_000`(256K×74%,见 `docs/auto-compact-256K-tuning.md`)、`InstructionSource::Inline` 注入(instructions 不落 disk)。
- **dump 工具**:`bin/dump_system_prompt.rs`(随上游 `PromptSessionContext` 字段变化维护)。

---

## 2. clean re-fork 移除清单(2026-06-04)

**丢弃(不再带入 fork)**:

| patch | 原因 |
|---|---|
| subagent 本地约束全套(MAX_STEPS=20 / ELAPSED=300 / resolve_agent_ref / tool_agent_route / Implementer-append_file) | `agent_*`/`delegate` 全在 blocklist,**subagent 路径生产不可达**;重做 subagent 时再恢复。`tool_agent_route` 硬编码 `deepseek-v4-flash` → **已提 [PR #2736](https://github.com/Hmbown/CodeWhale/pull/2736)**(继承父 model,通用 bug) |
| phase/demo workflow(跨仓全删) | submodule(PhaseDef/DemoInfo/strip_marker/PhaseChanged)+ app 后端(commands 四件套/ActiveSkillBinding/engine handler)+ app 前端(WorkflowView/PhaseChips/state.workflow/监听)。workflow 后续重做。**专家卡牌(persona)/plan_phase/auto-compact 独立,未碰** |
| qwen-128K(models.rs) | 死码:真实模型 `qwen36_35b_256k` 走上游 `_Nk` hint 解析返 256K;通用 `qwen→128K` 永不触发且语义错 |
| fetch_url 残留测试(33 行) | 测的全是已上游化 API(fake-ip CIDR),上游已有等价测试 |

**已被上游 v0.8.53 harvest(指纹撤除,非 fork-distinct)**:bing decode、network_policy fake-ip API(`with_trusted_fakeip_cidrs`/`is_trusted_fakeip_addr`)、InstructionSource enum、base override hook(`set_*_override`)、EngineConfig.instructions、256K auto-compact 核心基础设施(窗口识别 + `_Nk` hint + `should_auto_compact`)、MAX_OUTPUT env、file_search/grep_files timeout(上游 #2035)。

---

## 3. fork-guard 守护

```bash
./scripts/fork-guard.sh          # 全量:指纹层 + 编译跑回归测试
./scripts/fork-guard.sh --fast   # 仅指纹层,秒级(merge 后第一道快筛)
```

两层:**指纹层** grep 每个 fork 标记是否还在(抓「merge 静默丢整段 patch」);**行为层** `cargo test` 跑回归测试(抓「值/逻辑被改回上游」)。

**22 指纹**(`fingerprints=` 数组):
- submodule(12):file.rs 64KB · truncated_args_hint(dispatch) · tool_catalog blocklist · pinvou3_blocklist · careful 多行逐行 · careful shell YOLO-block · skills union(#25)· prompts skills union(#26)· skills 路径#41 · PROJECT_CONTEXT 砍空 · GLOBAL_PATHS 砍空 · constitution.json 短路
- app(10):bridge fake-ip(#18b)· bridge timeout(#16)· Tier5(#28)· Output Formatting(#33)· Sub-Agent Strategy(#32)· Constitution PINVOU3(#36)· Brother Whale 删(#36)· LOCALE(#37)· AUTHORITY(#38)· Inline 注入

**回归测试**:
| crate | 测试 |
|---|---|
| codewhale-tui | `forkguard_*` + `pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default` + `truncated_args_hint_fires/skips_*` + `test_{write,append}_file_rejects_oversized_content` |
| pinvou3-tauri | `forkguard_*` + `engine_config_locks_critical_fields`(max_subagents=1/timeout=300)+ `default_model_window_recognized_by_engine`(256K 识别)+ `search_prefs_*` |

⚠️ **sync 后除 fork-guard 还必须**:① `cargo test -p codewhale-tui --lib` **全量**(抓非 `forkguard_` 前缀的上游测试因 fork 行为 fail —— v0.8.51 append_file 静默丢失即靠全量抓出)② 跑一次 `dump_system_prompt`(它不在 fork-guard 构建里,v0.8.53 `PromptSessionContext.allow_shell` 漏字段即靠它抓出)。

---

## 4. Sync 历史(附录,倒序)

> clean re-fork(2026-06-04)后,以下章节为历史 lessons-learned;其描述的 per-patch 多已被 re-fork 重组/丢弃。保留以备上游回潮排查 + 维护经验。

### v0.8.53(2026-06-04,merge → 旧 pinvou3-patches,后被 clean re-fork 取代)
唯一真实冲突 `project_context.rs`:上游引入 `.codewhale/constitution.json` authority 层 + WHALE.md 弃用,撞组4 砍空 → 保空数组 + 新增 `load_repo_constitution_block` 短路。新字段:`EngineConfig.subagent_heartbeat_timeout`(默认 300=api_timeout,透传安全)、`PromptSessionContext.allow_shell`(dump bin 补字段)。教训:**dump bin 不在 fork-guard 构建里,sync 后要单跑**。

### v0.8.51(2026-06-03,118 commit)
4 冲突(prompts/agent.md embedder-agnostic 取 HEAD、registry import 两留、tool_card 合并)。**上游 cycle removal**:删 `cycle_manager` + `EngineConfig.cycle` → lib.rs 孤儿 `pub mod` + bridge cycle 关闭逻辑同删。**`CompactionConfig.auto_floor_tokens` 删除** → bridge 配套删,`token_threshold=190_000` 保留。新字段 `speech_output_dir`/`hook_executor`(EngineConfig)、`hook_executor`(Op::SendMessage)透传 default。**🩹 append_file 静默丢失**:merge 取上游 Implementer.allowed_tools 丢了 append_file,靠全量 lib 测试抓出 → 教训:**sync 后必跑全量 lib 测试,别只信 fork-guard**。

### v0.8.49(2026-06-02,367 commit)
6 个 PR 被上游 harvest(#2245 bing/#2311 InstructionSource/#2313 Tier5/#2314 environment-volatile/#2354 stop-on-failure/#2355 fetch_url)+ override hook 上游自实现扩展。**教训:整文件 `--theirs` 危险**(冲掉 4 个不在冲突区的 fork patch,靠指纹层抓回)—— fork-distinct 文件必须逐 hunk 解。lib.rs 上游新模块需手动补 `pub mod`(固定维护成本)。

### v0.8.47(2026-05-27,123 commit)
上游把工具 deferral 从 blocklist 翻成 **allowlist**(philosophy 相反)→ `request_user_input`/`append_file` 被 defer GUI 不出气泡 → 新增 `pinvou3_should_defer_native_tool`(C2 的由来)。上游把 write/append/edit 合进 `tools/file.rs`。`context_input_budget` 按窗口分级 / `_Nk` hint 全 vendor 被上游采纳。

---

最后更新:2026-06-04(clean re-fork → `pinvou3-clean` ← v0.8.53,6 主题 commit;正文重写为 commit 结构 + 当前 fork-guard;旧 per-patch 编号表已废弃,详见 §1)
