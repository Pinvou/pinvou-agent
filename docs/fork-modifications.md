# DeepSeek-TUI Fork 修改清单

> pinvou3 对 `DeepSeek-TUI` 底座所有 fork 修改的**单一现状清单**。
> 用途:① sync 后查 patch 存活 ② 团队交接 / onboarding ③ 上游 PR 定位改动点。
> 配套:`scripts/fork-guard.sh`(指纹 + 回归测试守卫)、`docs/fork-policy.md`(维护策略 + sync 流程 + 上游 PR 状态)。
>
> **当前基线**:submodule 分支 `pinvou3-clean` ← upstream **v0.8.57**(2026-06-11 sync,详见 §4)。

---

## 0. 当前状态速览(2026-06-11)

| 项 | 值 |
|---|---|
| submodule 分支 | **`pinvou3-clean`**(`.gitmodules` 追踪;旧 `pinvou3-patches` 留 fork 当备份) |
| fork drift | **+1364 / −307 行,30 文件**(`git -C DeepSeek-TUI diff v0.8.57..HEAD --stat`;app 层 prompt 走 override 注入,不计入此数) |
| LLM 暴露 native 工具 | **23 个**(blocklist 模型:全量注册 ~99 − 黑名单 81;**tool_search 已禁用**,模型无法激活 deferred 工具;实测见 §2)。MCP `mcp_pinvou_present_artifact` 另接,共 24 入口 |
| fork-guard | **28 指纹 + 回归测试**(submodule 17 + app 11);底座 lib **4218 pass** / app lib **105 pass**(单线程) |
| system prompt | dump 与 sync 前**逐字节一致**(172 行,diff=0);per-turn `<runtime_prompt>` tag 注入已 gate |

---

## 1. fork 结构(C1–C7 逻辑主题分组 ← v0.8.57)

> C1–C7 是 fork 的**逻辑分组**(非线性 git commit —— merge 后历史含上游 342 commit)。
> 看某文件的 fork-distinct 改动:`git -C DeepSeek-TUI diff v0.8.57..HEAD -- <file>`。

### C1 `lib` library facade
| | |
|---|---|
| 文件 | `crates/tui/src/lib.rs`(整文件 —— **上游只有 `main.rs`,无 lib target**) |
| 改动 | 暴露内部模块为 library(`pub mod ...`)+ `#[cfg(test)] pub mod test_support`,让 pinvou3-app 以 `deepseek_tui::*` as-library 调用 + `cargo test --lib` 能跑 |
| ⚠️ 维护 | **上游每加/删模块要手动同步 `pub mod`**(上游无 lib.rs,3-way 不会自动改它)。孤儿 `pub mod` 会编译错(v0.8.51 cycle removal 残留 `cycle_manager` 即此坑)。`acp_server` 依赖 bin 专属符号不能进 lib |
| 上游 PR | ❌ pinvou3 专用 |

### C2 `tools` blocklist 工具门控
| | |
|---|---|
| 文件 | `tools/pinvou3_blocklist.rs`(新建,**81 条黑名单**)、`core/engine/tool_catalog.rs`、`tools/registry.rs`、`core/engine.rs`(re-export) |
| 哲学 | 上游(v0.8.47 起)工具门控是 **allowlist**(`DEFAULT_ACTIVE_NATIVE_TOOLS`);pinvou3 相反 —— **显示全部、只隐藏黑名单**,给 Qwen3.6 精简到 **23 个**工具 |
| 关键 | `pinvou3_should_defer_native_tool(name, mode, always_load)` **mode-aware**:Yolo 只 defer 黑名单、其余全显示;非 Yolo 才叠加上游 allowlist。`request_user_input` 跨所有 mode 硬保留(否则 GUI 不出选择气泡);`image_analyze` 放出(Qwen3.6 有视觉,需 bridge 开 `VisionModel` feature)。`PINVOU3_BLOCKLIST_OVERRIDE` env 供 L1 harness 临时解锁 |
| ⚠️ tool_search 防御(v0.8.57) | **blocklist 是「defer 不删除」,工具仍在 catalog**,上游 v0.8.57 新增的 `tool_search` 工具(`ensure_advanced_tooling` 注入)能让模型**搜索并激活被 blocklist 的 deferred 工具**(agent/delegate),绕过门控 → 前端裸 JSON。修法:`tool_search_tool_regex`/`bm25` 加进 blocklist + **注入处 gate**(`is_pinvou3_hidden(TOOL_SEARCH_*)` 为真不注入)→ catalog 根本不含 tool_search。详见 sync §4 |
| blocklist 内容 | 见 `pinvou3_blocklist.rs` 源(subagent/RLM/automation/github/task/slop_ledger 全家桶 + todo legacy 别名 + **tool_search** 等)。**`checklist_*` 有意保留可见** |
| 测试 | `pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default`、`forkguard_tool_search_not_injected_blocks_deferred_activation` |
| 上游 PR | ❌ 哲学相反,pinvou3 专用 |

### C3 `tools` append_file + 大产物保护
| | |
|---|---|
| 文件 | `tools/file.rs`、`core/engine/dispatch.rs`、`core/engine/turn_loop.rs`、`client/chat.rs`、`tools/registry.rs`(`with_file_tools`)、`tui/approval.rs`、`tui/widgets/tool_card.rs`、`tools/approval_cache.rs` |
| 改动 | `append_file` 工具(**上游完全没有**)+ content **64KB 硬上限** + `truncated_args_hint`(流截断缺字段→引导分块)+ SSE idle-timeout 遥测 + undo 快照纳入 append_file |
| 理由 | 本地慢 vLLM 大产物(PPT/长文档)>240s idle timeout 流截断;`write_file` 写 skeleton(≤8KB)→ `append_file` 追加 chunk(≤16KB)工作流 |
| 测试 | `truncated_args_hint_fires/skips_*`、`test_{write,append}_file_rejects_oversized_content` |
| 上游 PR | ❌ 不提:64KB cap / hint 与 pinvou3 专属 `append_file` 深度耦合,上游无 append_file 去耦后引导无落点 |

### C4 `safety` careful 安全 hook(仅剩 C4-b)
| | |
|---|---|
| 文件 | `tools/shell.rs` |
| 改动 | **C4-b**:Dangerous 命令在 **YOLO 模式也 BLOCKED**(`shell.rs:2121` 注释 "BLOCKED in ALL modes including YOLO";上游 YOLO 跳过)—— 配合超级权限关闭态硬拦 sudo |
| ~~C4-a~~ | ~~多行命令逐行取最严级别~~ **v0.8.57 撤除**:上游 `extract neutral command support` 自带 `split_command_segments`(按 \n/&&/\|\|/;)+ `analyze_destructive_patterns` 逐段判 rm/find,已取代「批量 cp/mkdir 不误判 Dangerous」原意 |
| 上游 PR | ❌ pinvou3 安全模型专用 |

### C5 `prompt` GUI prompt / context / skills
| | |
|---|---|
| 文件 | `project_context.rs`、`skills/mod.rs`、`prompts.rs`、`commands/skills.rs` |
| project_context | `PROJECT_CONTEXT_FILES`/`GLOBAL_PATHS` **砍空**(workspace=$HOME GUI 助手,不读其他 AI 工具配置,context 走 inline 注入);`load_repo_constitution_block` **短路**(上游 `.codewhale/constitution.json` authority 层与 fork-policy §5 禁读 `~/.codewhale` 冲突) |
| skills | 扫描路径**只留 `~/.agents/skills`**(原 10 路径,#41;`skills_directories_with_home` 体,`_workspace` 未用)。⚠️ **union 接线已被上游 v0.8.57 harvest**(`render_available_skills_context_for_workspace_and_dir`)→ 取上游;**只有 #41 路径收窄仍 fork** |
| prompts/*.md | 已回退上游原文(drift=0):embedder-agnostic 措辞被 C7 composer 淘汰(composer 设置后这些常量只进 `ctx.default_layers`,pinvou3 不读) |
| 测试 | `forkguard_skills_dir_unions_with_home_rooted_workspace_skills`;project_context_cache / skills 多路径上游测试 `#[ignore]`(不读 AGENTS.md / 不扫 workspace skill 目录) |
| 上游 PR | skills union → [PR #2737](https://github.com/Hmbown/CodeWhale/pull/2737) **CLOSED**(上游已自行 harvest);constitution 短路 ❌ pinvou3 专用 |

### C6 `chore` 零碎 fork 适配
| | |
|---|---|
| 文件 | `llm_client/mod.rs`、`core/engine/lsp_hooks.rs`、`lsp/mod.rs`、`hooks.rs`、`core/turn.rs`、`tui/app.rs`、`.gitignore` |
| 改动 | 编译 / 接线层零碎适配(各 1-5 行) |

### C7 `prompt` static composer hook(密封静态层,#42)
| | |
|---|---|
| 文件 | `prompts.rs`、`core/engine/turn_loop.rs` |
| 机制 | `set_static_prompt_composer_override(Box<dyn Fn(&StaticPromptCtx)->String>)`:embedder 一个 hook **全量接管编译期静态文案**。`StaticPromptCtx` `#[non_exhaustive]`(**宽版** mode/approval_mode/model_id/allow_shell/default_layers) |
| 密封范围 | 装了 composer 则后置 append 全部 gate 掉:**ContextMgmt + COMPACT_TEMPLATE + Runtime Policy Reference**(`static_prompt_composer().is_none()`)+ **per-turn `<runtime_prompt>` tag 注入**(`static_prompt_composer_installed()`,turn_loop `messages_with_turn_metadata`)。后两者是 v0.8.57 上游新增,均会绕过 base/personality composer 漏入 |
| 理由 | 逐块 `set_*_override` 防不住"上游新增块漏进 prompt";composer 把静态层密封——上游升级新增 doctrine 只进 default 合成,进不了 pinvou3 prompt。借此 system prompt **20.2K→8.9K** |
| ⚠️ v0.8.57 reconcile | 上游 **独立实现了窄版** composer(`StaticPromptCtx{model_id, personality, default_layers}`,只换 base/personality)+ compose 管线改 **mode-independent**(2 参,mode 移出静态前缀)。决议:**删上游窄版重复定义、保 pinvou3 宽 ctx**(app `compose_static_layers` 依赖 `ctx.mode`/`ctx.approval_mode`);采上游 2 参管线但在 `apply_static_prompt_composer` 内**以常量 Yolo/Auto 构造宽 ctx**(生产单 Yolo-Auto,行为与 sync 前字节一致)。详见 §4 |
| 测试 | submodule `forkguard_static_prompt_composer_{replaces,unset}_*`;app `forkguard_static_composer_{takes_over,suppresses_context_mgmt,gates_runtime_prompt_tag}` |
| 上游 PR | [PR #2786](https://github.com/Hmbown/CodeWhale/pull/2786) **CLOSED**(上游自行实现窄版,语义不同);pinvou3 宽版保 fork |

#### app 层 composer 文案三轮瘦身记录(2026-06-05,20.2K→8.9K)——迭代 prompt 前必读
> 全部代码级证据,删的每一块都是「没它哪条生产路径会变」反事实审计的结果。

- **第一轮(机制+初删 →13.6K)**:删 Personality(语气并入 base.md §Voice)/ Session Longevity(推 sub-agents,与 blocklist 矛盾)/ Efficient Approvals + Approval Policy(生产单 Yolo-Auto,Plan 入口已下线)/ prompt-cache 教学 / taxonomy(instructions 工具表覆盖)。
- **第二轮(逐块反事实 →9.9K)**:Compaction Relay 模板全删(自动压缩走 `canonical_prompt()` 纯代码、手动走 `create_summary()` 独立 LLM,`handoff.md` 无写入通路 →模板无生产者无消费者);Article VII 九层→三行裁决(Tier 7/8/9 引用幽灵实体,长句与 Qwen3.6 有效文风相反;保留:Core duties→用户当前消息→instructions 项目法→工具输出 beats 记忆);Thinking budget 删(`reasoning_effort=off`)/ Sub-agents 删(工具不可见)/ 语言 bookend 压 1-2 句 / Authority Recap→Final Reminder 短版。
- **第三轮(base↔instructions 去重 →8.9K)**:操作性原则归 instructions.md 单一来源,base.md 只留红线+裁决+语气。消三处真冲突:① "never end a turn with a promise" 挪进 MODE_EXECUTE_MD(执行态专属);② "写后必读回" 与 §3 及时交付张力 → 统一为"最相关检查或明说没验";③ Language 句幽灵引用删。

### app 层 fork(不在 submodule —— 通过 override hook / bridge 注入,fork-guard 也守)
- **prompt 内容**:`resources/bundle/base.md` + `bridge/bundle.rs`(Constitution + 三行裁决 / Mode 块 / `LOCALE_PREAMBLE/CLOSER` zh+ja 短版 / `AUTHORITY_RECAP`→Final Reminder),经 `set_*_override` + `set_static_prompt_composer_override` 注入。**submodule 内 prompt 文案 drift=0**。
- **bridge config**(`bridge/mod.rs`):`subagent_api_timeout=300`、`max_subagents=1`、`network_policy` fake-ip CIDR 信任(`198.18.0.0/15`)、`compaction.token_threshold=190_000`(256K×74%,见 `docs/auto-compact-256K-tuning.md`)、`InstructionSource::Inline`(instructions 不落 disk)。⚠️ **v0.8.57 新字段 `stream_chunk_timeout` 现透传 default,本地慢 vLLM 或需调大,待实跑验证**。
- **dump 工具**:`bin/dump_system_prompt.rs`(随上游 `PromptSessionContext` 字段 / prompt 函数签名变化维护)。

---

## 2. 移除 / harvest 清单

### 2.1 clean re-fork 永久丢弃(2026-06-04,不再带入 fork)
| patch | 原因 |
|---|---|
| subagent 本地约束全套(MAX_STEPS / ELAPSED / resolve_agent_ref / tool_agent_route) | `agent_*`/`delegate` 全在 blocklist,**subagent 路径生产不可达**;重做 subagent 时再恢复 |
| phase/demo workflow(跨仓全删) | submodule + app 后端 + app 前端整套。workflow 后续重做(可复用上游 v0.8.57 新增的 WhaleFlow 基础 crate) |
| qwen-128K 死码(models.rs) | 真实模型走上游 `_Nk` hint 返 256K;通用 `qwen→128K` 永不触发且语义错 |

### 2.2 已被上游 harvest(指纹撤除,非 fork-distinct)
- **v0.8.53 及以前**:bing decode、network_policy fake-ip API、InstructionSource enum、base override hook(`set_*_override`)、EngineConfig.instructions、256K auto-compact 核心基础设施、MAX_OUTPUT env、file_search/grep_files timeout。
- **v0.8.57 新增 harvest**:① **skills union 接线**(上游 `skills_directories` 10 目录 union)→ 删 pinvou3 重复定义;② **C4-a 多行逐行**(上游 `split_command_segments` 取代)→ 删 fork 块 + 撤指纹;③ **本地 Bocha commit**(= 自己上游化的 #2946 已 merge)→ 整文件取上游。

> 工具集合实测命令(sync 后盘点新工具是否漏入,见 §3 checklist):对比两版本 `ToolSpec::name()` 字面量集合差集。

---

## 3. fork-guard 守护 + sync 后验证 checklist

```bash
./scripts/fork-guard.sh          # 全量:指纹层 + 编译跑回归测试
./scripts/fork-guard.sh --fast   # 仅指纹层,秒级(merge 后第一道快筛)
```

两层:**指纹层** grep 每个 fork 标记是否还在(抓「merge 静默丢整段 patch」);**行为层** `cargo test` 跑回归测试(抓「值/逻辑被改回上游」)。

**26 指纹**(`fingerprints=` 数组):
- **submodule(17)**:file.rs 64KB · truncated_args_hint(dispatch) · tool_catalog blocklist · pinvou3_blocklist · **tool_search 注入受 blocklist gate** · **tool_search 进 blocklist** · careful shell YOLO-block · skills union(#25)· prompts skills union(#26)· skills 路径#41 · PROJECT_CONTEXT 砍空 · GLOBAL_PATHS 砍空 · constitution.json 短路 · static composer hook(#42)· ContextMgmt/COMPACT gate(#42)· **Runtime Policy Ref gate(#42)** · **per-turn runtime_prompt tag gate(#42)**
- **app(11)**:bridge fake-ip(#18b)· bridge timeout(#16)· Constitution PINVOU3(#36)· Brother Whale 删(#36)· 冲突裁决三行(#43)· LOCALE zh 短版(#37)· AUTHORITY Final Reminder(#38)· Inline 注入 · composer 安装(#42)· compose_static_layers(#42)· LOCALE ja 短版(#42)

**回归测试**:
| crate | 测试 |
|---|---|
| codewhale-tui | `forkguard_*` + `pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default` + `truncated_args_hint_*` + `test_{write,append}_file_rejects_oversized_content` |
| pinvou3-tauri | `forkguard_*` + `engine_config_locks_critical_fields`(max_subagents=1/timeout=300)+ `default_model_window_recognized_by_engine`(256K)+ `search_prefs_*` |

### ⚠️ sync 后必做验证 checklist(fork-guard **不够**,以下每条都踩过坑)
1. **全量 lib 测试**:`cargo test -p codewhale-tui --lib`——抓非 `forkguard_` 前缀的上游测试因 fork fail(v0.8.51 append_file 静默丢失靠此抓出)。
2. **dump_system_prompt 前后 diff**——它不在 fork-guard 构建里。**非 0 diff 就逐块查谁漏进静态 prompt**(v0.8.57 Runtime Policy Reference 141 行泄漏靠此抓出;v0.8.53 `PromptSessionContext.allow_shell` 漏字段也靠它)。
3. **扫 per-turn message 构造路径**(v0.8.57 新增):`grep -rn "runtime_prompt\|messages.push" turn_loop.rs engine.rs`——上游可能新增**每请求注入的 transient 消息**(如 `<runtime_prompt>` tag),dump **抓不到**(只 dump system prompt)。
4. **工具集合盘点 + 激活机制盘点**:① 对比 v0.8.XX 两版 `ToolSpec::name()` 集合,新工具漏入要补黑名单;② **更要查上游有没有新增能激活/暴露 deferred 工具的机制**(`tool_search` / preflight / `ensure_advanced_tooling` 类动态注入)——blocklist 是 **defer 非删除**,被 blocklist 的工具仍在 catalog,任何能激活 deferred 的新路径都会击穿门控(v0.8.57 tool_search 即此,见 §4)。验证:`forkguard_tool_search_not_injected_*` 断言 pinvou3 catalog 不含激活工具。
5. **app 端单线程测试**:`cargo test --manifest-path pinvou3-app/.../Cargo.toml --lib -- --test-threads=1`——bridge provider/path 测试设全局 env,并行会 flake(非回归)。

---

## 4. Sync 历史

### v0.8.57(2026-06-11,merge v0.8.53→v0.8.57,342 commit / 291 文件)

**大版本 sync**。上游主线:DeepSeek→CodeWhale rebrand 全面铺开 + **system prompt 改 mode-independent**(mode/approval/allow_shell 移出静态前缀走 per-turn `<runtime_prompt>` tag,为 prefix cache 字节稳定)。

**引入的利好新特性**:睡眠唤醒不丢 turn(#2990,本地长生成救命)、跨会话磁盘 prompt 缓存(`prompt_persist.rs`)、慢 shell 自动转后台、PDF 读取挂死修复、9 个关键 bug 修复(#2880)、`turn_end` 观察者 hook、Qwen3.6 Plus 模型解析(迁入 `model_routing.rs`)、审批/状态选择器本地化(zh/ja)、WhaleFlow 基础 crate(无 runtime tool,workspace 重做 workflow 时可复用)。

**5 处冲突 + 关键判断**:
- **C7 composer 语义冲突**(最大判断):上游独立实现窄版同名 API。删上游窄版、保 pinvou3 宽 ctx;采上游 mode-independent 管线但以常量 Yolo/Auto 构造。`compose_prompt_with_approval_model_and_shell` 等 5 参 mode-aware 函数随上游删除。
- **Runtime Policy Reference gate**(新 #42):上游 #2951 新增 141 行全模式块进静态前缀 → 加 `static_prompt_composer().is_none()` gate(否则 dump 172→313 行)。
- **per-turn `<runtime_prompt>` tag gate**(新 #42,**sync 后审阅才发现**):上游每请求注入 transient `<runtime_prompt visibility="internal" .../>`。pinvou3 单 Yolo 下零信息 + 解释文档已 gate → Qwen3.6 看到无解释 internal tag 会复述。加 `static_prompt_composer_installed()` gate。**dump 抓不到,靠 grep 上游新机制发现**。
- **C5 project_context**:上游坐实 constitution.json + 全局 WHALE 路径,pinvou3 砍空+短路理念不变对更大面重打;5 个 cache 测试 `#[ignore]`。
- **C2 tool_catalog**:上游 `should_default_defer_tool` 改 2 参;pinvou3 透传 `build_model_tool_catalog` 持有的 mode,保 **mode-aware** 原设计。

**harvest 撤回**:C4-a 多行逐行 / skills union 接线 / 本地 Bocha commit(详见 §2.2)。

**C1 lib.rs**:补 7 `pub mod`(`config_persistence`/`llm_response_cache`/`model_routing`/`oauth`/`project_context_cache`/`prompt_persist`/`tls`);`resolve_auto_route_with_flash` 迁 `commands`→`model_routing`。

**app 层适配**:`EngineConfig` 新增 `search_base_url`(None)/ `stream_chunk_timeout`(default,**待验调大**);`PromptSessionContext` 移除 `allow_shell`(#2949);dump bin + 2 个 forkguard 测试随 mode-independent 签名(去 AppMode 参 / `system_prompt_for_mode` 删除)更新。

**⚠️ tool_search 击穿 blocklist(sync 后用户报障,深挖才定位)**:GUI 里「带儿子去欧洲旅游」类对话,工具调用渲染成裸 JSON(`prompt`/`allowed_tools`/`fork_context` 字段)。逐层排除(vLLM 正常 / 后端解析正常 / prompt 字节一致 / 我的 gate 不影响)后,靠**真实链路 probe**(`spawn_headless` + 真 vLLM,打印底座 Event 序列)+ 用户「未更新底座的工作区是好的」锁定:**v0.8.57 上游新增 `tool_search` 工具注入**(`ensure_advanced_tooling`,`git show v0.8.53` 确认这段不存在)→ `initial_active_tools` 强制激活它 → 模型用 tool_search **搜索并激活被 blocklist 的 deferred `delegate_to_agent`/`agent_*`**(blocklist 是 defer 不删除)→ 前端不认识 agent 工具 → 裸 JSON。修法见 C2「tool_search 防御」。**这是 blocklist 防御被上游新机制击穿的范例**:`pinvou3_should_defer_native_tool` 没问题,问题在另一条注入路径(`ensure_advanced_tooling` `defer_loading:false` 硬编码)+ tool_search 的激活能力。

**教训**(已并入 §3 checklist):① 上游「独立实现同名 API 但语义不同」既非冲突也非干净 harvest,逐字段比对决定保 fork / 取上游;② 新增静态 prompt 块绕过 composer 漏入 →**dump diff 是唯一可靠抓手**;③ 新增 per-turn 注入 dump 抓不到 →**还要扫 turn_loop/engine 消息构造路径**;④ **blocklist 防御可被上游新机制击穿**(tool_search 激活 deferred):工具集合盘点要盘的不只「工具名增减」,还有「上游有没有新增能激活/暴露 deferred 工具的机制(tool_search/preflight/动态注入)」——blocklist 是 defer 非删除,任何能激活 deferred 的新路径都是漏洞;**bug 排查时优先用真实链路 probe(spawn_headless)看底座 Event 序列,别被随机采样误导**(本次 probe 凑巧只采到正常工具,差点误判为「模型不稳定」)。

### 旧版教训速查(v0.8.47–53,clean re-fork 前;per-conflict 细节已废弃,只留可复用教训)
| 版本 | 可复用教训 |
|---|---|
| v0.8.53 | dump bin 不在 fork-guard 构建里,**sync 后单跑**(`PromptSessionContext.allow_shell` 漏字段靠它抓) |
| v0.8.51 | **sync 后必跑全量 lib 测试**,别只信 fork-guard(merge 取上游 `Implementer.allowed_tools` 静默丢 append_file,靠全量测试抓) |
| v0.8.49 | **整文件 `--theirs` 危险**(冲掉 4 个不在冲突区的 fork patch,靠指纹层抓回)→ fork-distinct 文件必须逐 hunk 解 |
| v0.8.47 | 上游把工具 deferral 从 blocklist 翻成 allowlist(`request_user_input`/`append_file` 被 defer 气泡消失)→ C2 的由来 |

---

最后更新:2026-06-11(v0.8.57 sync;基线 v0.8.53→v0.8.57,正文校准 drift/指纹/工具数,旧 sync 历史压成教训速查)

## 工作流底座层(feat/sansheng-workflow PR 随附,submodule 分支 pinvou3-workflow-v0857)

随三省六部工作流引入的底座 fork 层,从 pinvou3-clean 旧线移植到 v0.8.57:

| 项 | 内容 |
|---|---|
| Op::SpawnSubAgent 扩展 | +role_id/allowed_tools/max_steps/output_schema/expects_file_output 五字段;engine 按角色白名单+步数派 Custom SubAgent;空白名单 fail-fast |
| StructuredOutput | submit_output 工具+schema 校验+x-output-file 落盘;stop 拦截催交(MAX_STRUCTURED_OUTPUT_RETRIES);耗尽置 failed |
| request_user_input 路由 | SubAgent 可弹 GUI 卡片阻塞等答案(user_input_tx),不吃 TOOL_TIMEOUT |
| AgentComplete 信封 | +role(SDAN Result.from)+failed(宿主走失败路径,不再被陈旧产物洗成 PASS) |
| mailbox + AgentSpawned | Op 路径 SubAgent 挂 Mailbox(TokenUsage 等信封直达宿主)+二发 AgentSpawned 关联 agent_id→role_id(edict-obs) |
| 贪心解码 | SubAgent 每步 temperature=0(根治 NVFP4 下工具调用 XML 被采歪→空转) |
| C8/C9 | SubAgent surface 注册 web/custom 工具;read_pdf catch_unwind+中文字符边界防 panic |
