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

---

最后更新: 2026-05-27
