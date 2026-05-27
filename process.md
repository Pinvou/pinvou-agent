# pinvou3 进度记录

跨阶段待办、决策、follow-up 集中地。决策细节走 git commit + `docs/`,这里只放**需要单独排期**的事。

最后更新: 2026-05-27

---

## 当前状态

- **main HEAD**: `d99af05` (阶段 J + J-后续 — 已 push origin `Pinvou/pinvou3` + backup `h3c-hexin/pinvou3`)
- **fork HEAD**: `63e17d77` (`pinvou3-patches`,已同步上游 **v0.8.45 + rebrand CodeWhale**,已 push `h3c-hexin/DeepSeek-TUI`)。另有分支 `fix/grep-files-timeout`(`f2153f10`)= **PR #2146**,从 `upstream/main` 拉,不并入 patches
- **进行中**: 无重大未并项;阶段 J 全部并入 main 并推送
- **worktrees**: `workflow-discussion` (本会话工作,已并入 main)、`workflow-desing-01`、`pinvou-review-v2-DO-NOT-DELETE`

---

## 已完成阶段

> 决策细节走 git log,这里只留**定位 + 数据点 + 文档兜底**。

### 阶段 J — 大文件 SSE timeout Phase 1 + 上游 v0.8.45 同步/rebrand + 思考指示器修复 (2026-05-25)

本会话四块,全部并入 main `e729d06` + 推 origin/backup:

1. **P7 大文件 SSE timeout Phase 1 完整版**(fork `ade944d`,代办 #0 治本): 
   - `write_file`/`append_file` 加 **64KB 单次 content 硬上限**(超限返 `InvalidInput` + 引导骨架+分块)。阈值 64KB 而非 32KB 是为不撞上游 `WRITE_FILE_INLINE_DIFF_LIMIT_BYTES`(32KB diff-omit 仍合法)。
   - **截断感知错误**(`truncated_args_hint`): write/append 因流截断缺 required 字段时,错误改成"参数被截断、content 太大、先骨架后 append ≤16KB 分块",替代干巴巴 missing_field,掐断 loop_guard 原样重试内耗。
   - SSE idle timeout 错误带 `bytes_received`/在途 tool_use buffer 诊断,区分 prefill 静默 vs 参数中途断。**实测根因**: 大 tool-arg 生成静默 > 240s → 流被切 → arg_repair 修出缺字段调用。

2. **上游 v0.8.45 同步**(fork `2564193c` merge): 上游 148 commit + rebrand(crate `deepseek-tui`→`codewhale-tui`,仓库 `Hmbown/CodeWhale`)。6 冲突解净(file_search/grep 取上游已 harvest 版,append_file/phase-marker/subagent-cap/预算全保)。pinvou3-app rebrand 适配仅 2 处: Cargo.toml `package="codewhale-tui"` 重命名保留 `deepseek_tui::` 别名 + bridge 透传新增 `EngineConfig.subagent_api_timeout`。

3. **思考指示器修复**(main `e729d06`): ① 去掉冗余 busy-indicator banner(和消息流气泡重复);② **根因修复**: bridge 之前无条件 `Event::Error`→`chat:done`,而 SSE idle timeout 是 `recoverable=true` 中途错误(turn 不结束),却被当结束 → 前端 `setBusy(false)` 把"思考中"掐掉、引擎还在跑却显示空白。现按 `recoverable` 分流: 瞬态→`chat:transient_error`(只飘 ⚠️ 不动 busy),仅致命→`chat:done`。

4. **上游 PR**: #2057(subagent role)、#2060(self-hosted 窗口 compaction 预算)已合;#2146(grep timeout)、#2147(web_search retry)待审。CLAUDE.md 加"fork 改动按通用/专用决定是否提 PR"规则。

### 阶段 J-后续 — 子agent 底座修复系列 (2026-05-26)

`v0.8.45-subagent-reeval` 报告发现后端并发 OK、但子agent 仍大面积超时失败。当日密集底座修复 11 项 + prompt 工程 3 项,全部落 `pinvou3-patches` + main:

**超时/默认值调参**:
- `subagent_api_timeout` 120s→300s、`DEFAULT_SUBAGENT_ELAPSED_MAX` 300s→600s、harness heavy-scenario timeout 660s→1260s
- `DEFAULT_MAX_STEPS` 20→12 (20 步在 600s 内跑不完,12 步强制收敛)
- ⚠️ `max_subagents`: 当时把 app 默认从 1 改到 4。**2026-05-27 验证结论建议回退到 1**(多/并行 fan-out 不可用,见下方 lesson learned)。⚠️ subagent 行为调优由 subagent owner 负责,此处仅记录验证侧建议,不擅自改 subagent 代码

**联网工具链补强**(⚠️ 当日快照,2026-05-27 已部分推翻——见下方 lesson learned + subagent 边界的最新口径):
- `web_search` Bing parser: `normalize_bing_url` 先 `decode_html_entities`,修复 bing 恒返回 0(**保留,已提 PR #2245**)
- `fetch_url` fake-ip 信任: ~~`proxy=["*"]`~~ **已废弃改成按 IP 段信任**(`with_trusted_fakeip_cidrs`),`proxy=["*"]` 会放行任意域名→内网 SSRF
- ~~网络重试 retry + timeout 15s→30s~~ **已弃**(env-specific 且未验证)

**子agent 交互修复**:
- `tool_agent` model 路由: 硬编码 `"deepseek-v4-flash"` → 继承父 session `runtime.model.clone()`,避免子agent 走远程 API
- `resolve_agent_ref` 截断容错: LLM 截断 `"agent_"` 前缀时自动补回,修复 `agent_eval` 连续 "not found"
- harness `max_subagents` 硬编码去除: `spawn_for_scenario` 不再覆写 bridge 默认值

**任务范围膨胀修复 (Scope Bloat)**:
- 根因: 主agent 生成过于详细的 prompt("搜 A/B/C/D 然后分析写报告"),子agent 淹没系统 prompt 的步数约束,进入 8-15 步搜索循环撞 cap
- 底座修复三处: ① `agent_open` schema description 引导主agent 给简单任务; ② `build_assignment_prompt` 强制注入 `**step budget: 12 max**` + 半程提醒; ③ `DEFAULT_MAX_STEPS` 20→12 硬性上限
- 预期: 失败模式从"超时白跑"转为"提前返半成品",主agent 负责综合

### 阶段 I — 工作流 phase 可视化 MVP1 + P7 大文件 SSE timeout 治本 (2026-05-22)

**MVP1 phase 可视化** (`54825bd` / `133ee3f` main 主分支 + `efa7811` worktree 平行方案): SKILL.md `phases:` 字段解析 + `<phase id="../>` marker + `Event::PhaseChanged` + workflow 视图 chip 横排。主分支版 chips 在 workflow 视图内 + MVP3a 工具调用启发式 phase 推断;worktree 平行版 chips 在 chatroom 顶部 + 显式启用 + USER_NUDGE 兜底。两套并存暂未合并,看 h3c-ppt 实战哪个更顺。

**P7 大文件 SSE timeout 治本** (`095fda2` + worktree `c54b149` / `863e3e7`): 实测 h3c-ppt P7 阶段 LLM 生成 12K+ tokens HTML decode 10 分钟撞 240s SSE timeout + JSON 字段顺序乱(content 在 path 前)→ missing_field 'path' → loop_guard 3 次锁死。完整三连失败链路一次性治本:

| 层 | 修法 | 提供者 |
|---|---|---|
| 工具能力 | `append_file` 新工具 (创建/追加,返摘要不返 full diff) + `write_file` >32KB 跳过 inline diff 改返摘要 | submodule `0526dc5` |
| `(Plan, Planning)` reminder | "产物 >300 行 plan 必须拆: 先写小骨架(≤200 行, 不 inline CSS/JS) → 分块 append_file → read_file 验证" | main `133ee3f` 后 + worktree 骨架约束 `863e3e7` |
| `(Yolo, Executing)` reminder | 执行阶段 6 条规则含同样约束 | 同上 |
| `(Yolo, None)` reminder | 纯 Yolo 路径(不进 Plan 模式)同步覆盖,关键漏洞 | worktree `c54b149` |
| Pinvou GATE 硬阈值 | 单次 write_file >300 行/20KB → CRITICAL; 20+页 HTML PPT 无分块 → CRITICAL | main `133ee3f` 后 |
| max_output_tokens | 16384 → 65536 配合 append_file 提供 budget | main `133ee3f` 后 |
| instructions.md §7 | 大产物先 write_file 骨架, 再 append_file 分块追加 (1 行兜底) | 同上 |

15 页 PPT 实测端到端跑通: write_file 5304 bytes 骨架 + 多次 append_file 累积 14KB+,无任何 SSE timeout / missing_field / loop_guard block。详 git log。

### 阶段 H — auto-compact 接通本地 256K vLLM (2026-05-19, `3325f9d` + `5f53848`)

底座为 V4 1M 设计,默认参数在 256K 窗口下 4 个子系统静默退化或反向触发,会话一路涨到 vLLM `max_model_len` 撞墙 400。一次性收口:**B1+B2 fork**(`_Nk` hint 全 vendor + `context_input_budget` 按窗口分级)+ **bridge** 关 cycle 子系统 + wire `DEEPSEEK_MAX_OUTPUT_TOKENS` + **careful hook** YOLO 下也 BLOCK Dangerous + 隐藏 token bar。回归测试锁默认模型窗口识别。详 `docs/auto-compact-256K-tuning.md`。

### 阶段 G — 品悟 v2 review 系统 (2026-05-19, `e6c5ea8` 一次性 merge 20 commits)

把 pinvou2 "常驻并发嘴替"压成"3 节点触发"——基于 pinvou2 实测(raise_concern 工具化率 25%)+ gstack production 验证 + 同步前台流约束。配套 rebrand "嘴替"→"品悟"(toggle 跟产品名同名)。

| 节点 | 触发 | 严苛度 | 状态 |
|---|---|---|---|
| **A. Plan 出炉** | `ExitPlanMode` 前 | **L2 blocking** | ✅ `/pinvou-review-plan` |
| **D. 任务收口** | TurnComplete | L1 advisory | ✅ `/pinvou-review-final` |
| **E. Stuck 兜底** | auto-continue 3 次失败 | L1 advisory | ⏸️ v1.5 选做未做 |

详 `docs/Pinvou-品悟设计.md`。

### 阶段 F — subagent 完整支持 + L1 testing 系统成熟 (2026-05-18~19, `5cf9afc`)

single subagent context isolation 完全可用 + 9 fork patches 落定。L2 backend 49 tests CI gate;L1 真 vLLM dialog harness 18 scenarios;Judge rubric r2(6 维 × 1-5 分);5 次 baseline 演进 4.75→4.66。`max_subagents` 4→1 工程硬锁;INSTRUCTIONS_MD v4(164→112 行)。上游 PR #1790 + Issue #1791 已提。详 `docs/l1-baselines/v0.8.37-vllm3_final-r2-18scn/judge-report.md`。

### 阶段 E — 工具表精简 + GUI 体验优化 (`e30b64c` / fork `8e9be9c7`)

LLM 可见工具 85→16+4,schema 119KB→14KB,翻译 19s→8s。OpenAI streaming batch tool_calls 修复 → 上游 PR [#1686](https://github.com/Hmbown/DeepSeek-TUI/pull/1686) 已合。Plan 模式 `trust_mode=true` + 路径校验放宽。详 `docs/工具表精简方案.md`。

---

## 代办项

### 🟡 中等

#### 0. ✅ P7 大文件 Phase 1 — 已落地 (阶段 J `ade944d`, 2026-05-25)

预案里的"硬上限拒绝 + actionable recovery"已做:
- ✅ `write_file`/`append_file` content **64KB 硬上限**(非 32KB,避开上游 32KB diff-omit 合法区) → 超限返 `InvalidInput` + 骨架/分块引导。
- ✅ actionable recovery: `truncated_args_hint` —— 流截断缺字段时回"参数被截断请分块",而非干巴巴 missing_field,引导模型换 tool 不原样重试(缓解 loop_guard 锁死)。
- ✅ 诊断日志: SSE idle timeout 带 bytes/buffer,**实测确认根因** = 大 tool-arg 生成静默 > 240s 被切。
- ⚠️ **未做**(暂不需要): `dont_count_for_loop_guard` 标记 / sticky `tool_choice=named` 强制下轮 append_file。当前 hint 已能引导模型自纠,实测够用,YAGNI。

#### A. WorkFlow 视图编排 (差异化最强)
- 用户明说"准备做编排,要细聊"
- 交互模型待讨论: todo checklist / 可视化节点流 / 专家协作
- 跟现有 `plan_card` 可能融合
- 设计落定再 2-3 天实现

#### B. 附件预处理 pipeline
bridge 拦截 docx/pdf/image 上传 → pandoc/tesseract 转 md → 嵌 user message。省 5-10k token/turn + UX 直拿数据 + AI 无需 `pandoc_convert/image_ocr` 工具决策。1-2 天,详 `docs/工具表精简方案.md` §6.1。

#### D. 模型预设切换 GUI
bridge 已有 `ModelPreset` 占位,缺 GUI。支持远程 DeepSeek API / OpenRouter。

#### GUI / Subagent 体验
- **GUI subagent 卡片方案 B/C** (扩展版): 主 agent 串行视图 / subagent 内部 timeline。当前 A 已让用户能识别"嵌套 LLM 调用"
- **Settings toggle 启用 subagent**: 当前 blocklist 默认屏蔽,用户测试需 env override。加 `UserPrefs.advanced.enable_subagent` GUI 勾选

### 🟢 低优先
- **Multi-subagent 大研究任务**: 2026-05-27 验证结论——并行 fan-out 不可用,真根因是主 agent 编排/结果提取认知失败(非工具/后端,见 lesson learned 2026-05-27)。建议替代:主 agent 直接 curl/web_search 或串行单任务 subagent。**最终 subagent 策略由 owner 决策**
- **真 sanity 跨 session 重评**: 当前 sanity 是同 session retake,真要看 Claude judge 一致性需开新 session 重评
- **GB10 self-hosted GitHub Actions runner**: L1 nightly 进 CI,等团队 ≥2 人或发版频率上升 (详 `docs/自动化测试方案.md` §8)

### ⏸️ 已决策不做
- **视觉模型补足** (Qwen-VL / InternVL2 + bridge 自动 caption) — 当前场景不需要,详 `docs/工具表精简方案.md` §6.2 备查
- **多 vLLM 实例给 subagent** — 不解决根本(subagent 慢主因是模型行为,非 vLLM 限制),且破坏 single-vLLM 简洁架构
- **prompt 工程让 LLM 不死磕** — reminder 改造 ROI 负(lesson learned 2026-05-18 验证)
- **reminder vs XML 标签去重观察** — 同上,被 lesson learned 推翻
- **L3 GUI Playwright** — UI 改动少 ROI 低
- **Scenario 提取 toml** — 18 个手维护够,等 ≥30 个再考虑
- **上游 P2/P3 PR**: `DEEPSEEK_MAX_OUTPUT_TOKENS` env override + lib `pub mod` 暴露 — pinvou3 偏好≠生态价值,fork 自留更干净
- **产物内嵌预览** docx/pdf/xlsx — 已尝试回退,`↗` 系统应用打开够用

---

## Fork → 上游 PR roadmap (2026-05-26 更新, grep #2146 + max_output #2147 提交后)

上游已 rebrand: **`Hmbown/CodeWhale`**(原 DeepSeek-TUI),crate `codewhale-tui`,当前 v0.8.45。fork 已同步。
CLAUDE.md 规则: 通用优化/bug → PR;pinvou3 专用 → 留 fork。

| 状态 | Commit | 内容 | 备注 |
|---|---|---|---|
| ✅ 已合 | `363dd35` | subagent role system→user (Qwen chat_template 400) | **PR #2057 MERGED** (2026-05-25) |
| ✅ 已合 | `7e5288e` | sub-500K 窗口 compaction 预算 (`_Nk` vendor-agnostic + 分级 reserved) | **PR #2060 MERGED** (2026-05-25) |
| ✅ 已收敛 | `079a3bb` | file_search timeout + spawn_blocking | 上游 **#2035 harvest 等价实现**(连 `FILE_SEARCH_TIMEOUT` 同名)。**#1790/#2044 CLOSED**;HEAD 与上游 `file_search.rs` **逐字节一致, 零漂移, 无可 drop**(合并时自动收敛) |
| 🔵 **待审** | `aaa1920` | grep_files timeout + spawn_blocking | ⚠️ **v0.8.45 合并时被上游 harvest 版覆盖丢失**(上游 grep 只剩 per-file cancel-check, 无 spawn_blocking/硬超时, 大目录仍独占 runtime 线程)。2026-05-26 从 `upstream/main` 拉净分支重做(`run_blocking_grep` 对称 file_search) → **PR #2146 OPEN**(一致性论点)。**未回 fork** |
| ✅ 已合 | — | OpenAI streaming batch tool_calls | **#1686 MERGED** 进上游。分支已删 |
| 🔵 **待审** | `93e9474` | `DEEPSEEK_MAX_OUTPUT_TOKENS` env override (tight-context providers) | **PR #2147 OPEN** (2026-05-26 提) |
| 🟡 待沟通 | `b2f6ef56`/`a25352a` | careful YOLO BLOCK / 多行命令逐行分析 | 碰 **execpolicy/sandbox 信任边界**(CONTRIBUTING 要求预沟通);a25352a 是"放松"安全面更需先开 issue。**不盲提** |
| 🟡 弱适合 | `9860ef1` `15244e6` `dd879db` | MAX_STEPS 默认 / stop-on-failure prompt / test_support | 改默认值/prompt/不常见模式,ROI 低或需先 issue,**暂不主动** |
| ❌ fork-only | `6ac5b97` `47e6abc` / `b9b40ce` `36526ce` `1ba8e41` / `ade944d` 64KB 上限 | lib export / blocklist / 64KB 硬上限(opinionated) | pinvou3 专用或 opinionated,留 fork(64KB 那条若要 PR 需拆且阈值可配置) |

**已合**: #2057 #2060 (2026-05-25)、#1686 #1511。

**下次开工动作**:
1. `gh pr view 2146 2147 --repo Hmbown/CodeWhale` 看反应(merge / harvest / 意见)
2. #2146(grep timeout) 若被 harvest/merge → grep 零漂移白拿;若被拒 → 真实痛点($HOME 卡死)上游已用 per-file cancel-check 覆盖, 零损失, 不回 fork
3. #2147(max_output_tokens env) 待审; 若维护者倾向接 `config.max_output` 而非 env, 按反馈改
4. `a25352a` 若想推, 先开 issue 描述"多行命令一刀切 Dangerous 误伤", 拿 sign-off 再提

---

## 已知问题 / 边界

### subagent 边界
- ✅ **single subagent 完全可用** (context isolation use case, 验证 19s/75s 完成 5.00 分)
- ✅ **2-3 subagent 串行可用** (one_fails 377s PASS 4.83 分, 主 agent 用 checklist_write 拆步)
- 🔓 **后端并发本身不是瓶颈 (2026-05-26)**: 探针 N=4 first-token <1s,`agent_open×3/×4` 全成功并行,当年"多序列卡 SSE"不复现 (planB 4× batched-tokens 之功)。
- ✅ **环境工具两个真 bug 已修 (工作区未提交)**: ① `fetch_url` fake-ip 误杀 → **按 IP 段信任 fake-ip 占位段**(`network_policy is_trusted_fakeip_addr` + bridge `with_trusted_fakeip_cidrs(["198.18.0.0/15"])`;早期 `proxy=["*"]` 会放行任意域名→内网,已废弃改 IP 段,真实私网/元数据仍拦);② `web_search` bing 恒返回 0(`normalize_bing_url` 先 `decode_html_entities`,**已提 PR #2245**)。**注:2026-05-27 重测证明这俩跟多 agent 研究失败无关**——见下方 lesson learned。
- ❌ **多 subagent 并行研究 fan-out 不可用 (2026-05-27 验证)**: 真根因不是工具/后端,是主 agent **编排认知 + 结果提取协议**——看不懂 completed 子 agent 结果在 eval 返回里 → 反复重 spawn 撞超时。建议 app `max_subagents` 回退 1 物理堵死 fan-out(**待 subagent owner 决策**)。详见 lesson learned 2026-05-27。

### LLM 行为不稳 (vLLM 抽奖)
- `chinese_idiomatic` 偶发 detour 写文件 (3/5 次出现)
- `long_output_1500` 偶发 detour 调 web_search
- 跨 baseline 均值看是 noise, 单 sample 信号不下结论

### 工具同步阻塞
- ✅ `file_search` 已修且**上游已收敛**(#2035, fork 零漂移)
- 🔵 `grep_files` fork patch 已在 v0.8.45 合并时丢失;上游现有 per-file cancel-check(大目录场景 OK), spawn_blocking+硬超时走 **PR #2146**(待审, 未回 fork)
- ⚠️ `list_dir` 单层 read_dir 通常 OK 但极端目录边缘 case 可能

### Judge 自身局限
- Claude 跟 Qwen 都是 LLM, 某些"模型味"Claude 看不出来
- 单 session retake 是"演习"不是真 sanity (需新 session 重评)
- ±0.2 是 noise, ±0.5 才算 signal

---

## L1 judge 离群点跟进

> 流程见 `docs/L1-judge-rubric.md` §3 Step 5。任一维度 ≤3 → append 至此。

### 🆕 / 🔁 待处理

### 2026-05-26 · run 1779779654-r2 · subagent_research_topic · 准确性 1/5
- **问题**: final text 只有执行状态说明,没有给出 RAG 2025-2026 综述正文。
- **改进方向**: subagent 全失败时应更早降级,至少在 1-2 个失败后先产出基于现有知识的结构化综述。
- **状态**: 🆕 待处理

### 2026-05-26 · run 1779779654-r2 · subagent_research_topic · 完整性 1/5
- **问题**: prompt 要求覆盖学术新方向、工业落地、开源工具、踩坑经验四类,final text 未覆盖任何一类。
- **改进方向**: 降级输出也必须保留四个必答栏目,不能只输出执行状态。
- **状态**: 🆕 待处理

### 2026-05-26 · run 1779779654-r2 · subagent_research_topic · 简洁性 2/5
- **问题**: 输出主要是“还在跑/让我再等/失败后换思路”的过程复述,没有转化为结果。
- **改进方向**: transcript 内可保留过程,final answer 应只给结论、降级说明和下一步,避免“让我...”流水账。
- **状态**: 🆕 待处理

### 2026-05-26 · run 1779779654-r2 · subagent_research_topic · 工具使用 2/5
- **问题**: 用 `exec_shell sleep` 轮询 4 次,重复 `agent_eval`,且在 prompt 明确禁止主 agent 直接 `web_search` 后仍直接调用 5 次。
- **改进方向**: 禁止用 `exec_shell sleep` 轮询 subagent;优先 `agent_eval block=true` 或有界短轮询,并遵守 scenario 约束。
- **状态**: 🆕 待处理

### ✅ 已结案 (简表)

| 离群点 | 结案方式 | 报告位置 |
|---|---|---|
| `plan_travel_web` · 工具 3/5 (没调 update_plan) | ✅ 修复: vLLM plan B 配置 (`max-num-batched-tokens 131072`) 自动修, 零代码改动 | run 1779089467-r2 / 1779102334 |
| `long_output_1500` · 工具/简洁性 2/5 (detour web_search + 12K 字超 8×) | ✅ 修复: INSTRUCTIONS_MD v4 §3 任务完成定义起效 (122s + 6.7K 字 PASS) | run 1779159923-r2 |
| `plan_mode_list_dir` · 完整性 3/5 (text 悬空) | ✅ 修复: 跟 LLM 复述本性有关, 跟简洁性同因 | 同 lesson learned |
| subagent SSE 失败 + research_topic / compare_3_libs timeout | ✅ 修复: role=system→user (`363dd35`) + STREAM_OPEN_TIMEOUT 45s→180s; multi-subagent 真根因是 Qwen verbose 不是调度 | run 1779095350 / 1779102334 |
| `plan_mode_list_dir` · 简洁性 3/5 ("让我..."过渡语, 3 次出现) | 🤝 接受现状: lesson learned ROI 负, 不再 prompt 工程改造 | run 1779074272/77762/89467 |
| `multi_turn_context_t2` · 简洁性/工具 (exec_shell 算 36岁 overkill) | 🤝 接受现状: 单 sample 信号弱, 累积 3+ 次再评估 | run 1779095350-r2 |

### 🧪 Lesson learned 2026-05-18: reminder 改 ≠ LLM 行为改

尝试 commit `7b983b6`: Plan/Planning system-reminder 加第 5 条 "调完 update_plan 必须给方案 summary"。单 scenario 验证 (run 1779078816):

- 完整性 3→5 ✅ 但简洁性 3→3 持平 (LLM 仍写 5 次"让我...")
- text 暴增 23 → 556 字 (+24×) — reminder 反向起作用
- 副作用 > 改进, 回滚 (`ec2f788` revert)

**学到的**:
- prompt 工程改 LLM 行为需多 scenario A/B 对比, 单样本不可信
- LLM 复述思考过程 (写"让我...") 是 mode-independent 行为, reminder 改不动
- 加 reminder 条款 ROI 要看 (a) 群体改善 ≥ ±0.5 (b) 副作用 (token + system prompt 长度)
- 类似离群点先积累 3+ 次出现证据再考虑改 reminder; 真要改要在 2+ scenario A/B 对比验证

### 🧪 Lesson learned 2026-05-22: prompt 工程对 Qwen3.6 不是 ROI 一刀切, 看规则形态

跟 05-18 lesson 配对修正: 之前判断"prompt ROI 低"过粗。h3c-ppt P7 卡死治本实战发现, **同样是 reminder 改 LLM 行为, 量化具体规则 ROI 高, 抽象意图 ROI 低**:

| reminder 形态 | 实测结果 | 例 |
|---|---|---|
| 抽象意图("写大文件必须拆") | LLM 自由解释边界, 不听 | d58b57e instructions.md 18 行引导 + h3c-ppt SKILL.md "拆 slides" 全失败, 仍写 mega.html |
| 量化具体规则("骨架 ≤ 200 行, 不 inline CSS/JS, append_file 每块 ≤ 200 行") | LLM 严格遵循 | `863e3e7` 加骨架约束后实测 5304 bytes 骨架 + append_file × N 跑通 15 页 PPT |

**学到的**:
- LLM 需要的不是"抽象意图", 是"能精确执行的边界"; "≤ 200 行" 比 "必须拆" 强 10×
- Qwen3.6 弱模型对量化规则的执行力 ≈ 强模型对抽象意图的执行力, 工程投入要换形态不是放弃
- reminder 路径需要全覆盖, 一个 case 漏注入(如 `(Yolo, None)` `c54b149` 之前没 reminder)就是 LLM 自由发挥的入口
- 工具能力 + reminder 协同设计: append_file 不存在时 reminder"必须拆"是空话; 工具 + 规则一起出, LLM 才能落地

**配对结论**: 05-18 lesson 没错(单 scenario 验证不可信 + 复述本性改不动), 但**不要因此放弃整个 prompt 工程路径**。改 LLM 行为前先问: 我的规则是"抽象意图"还是"量化具体边界"? 形态对了 ROI 完全不同。

### 🧪 Lesson learned 2026-05-26: 弱模型下多 subagent 并行不是好设计

本轮测试(`run 1779799386`) 完整验证了多 subagent 并行在 Qwen3.6-35B-A3B-FP8 @ 本地 vLLM 上的表现:

| 模式 | 场景 | 耗时 | 管理开销 | 结果 |
|------|------|------|---------|------|
| 单 subagent | `single_simple` | **23.7s** | open×1, eval×1 | ✅ 完美 |
| 多 subagent 并行 | `compare_3_libs` | **785.9s** | open×4, eval×12, 重试×1 | ⚠️ 能成但极重 |
| 多 subagent 并行 | `research_topic` | **933.2s** | open×5, eval×9, 失败×2, 遗忘×1 | ❌ fallback |
| 多 subagent 并行 | `one_fails` | **293.9s** | open×3, eval×6 | ⚠️ 1个超时 |

**核心判断**: 多 subagent 并行的收益("分工+并发提速")被管理成本完全吞掉:
1. **主 agent 分活质量差**: Qwen3.6 拆分任务能力有限,`research_topic` 把15+工具塞给1个子 agent → 12步不够 → failed
2. **调度开销巨大**: 每步40-70s,`compare_3_libs` 轮询12次 eval 消耗大量时间
3. **注意力分散**: 处理失败重试时遗忘其他子 agent(`research_pitfalls` 从未被检查)
4. **失败处理复杂**: 子 agent failed 后主 agent 需决策重试/降级/fallback,代码路径爆炸
5. **并行不省钱**: 3-4个并发但总时间仍785-933s,因为主 agent 要等最慢的那个

**设计决策**:
- ✅ **保留**: 单 subagent 场景(context isolation、简单委托)——`single_simple` 23.7s 证明干净可用
- ❌ **废弃**: "主 agent 分活 + 多 subagent 并行研究"模式——弱模型下 ROI 为负
- 🔵 **替代**: 复杂研究类任务改为"主 agent 直接工具调用"或"串行单任务子 agent"(1个子 agent 做1件事,串行派发)

**`compare_3_libs` 虽成功但不应视为模式可行**: 3/3 完成是靠12次 eval + 1次重试 + 785s 堆出来的,管理复杂度远超收益。若主 agent 自己做(web_search×3 + fetch_url×3 + 综合),预计 200-400s 且更可控。

### 🧪 Lesson learned 2026-05-27: 并行 fan-out 真根因是「编排认知」,非工具/后端

环境工具修好后重跑:`compare_3_libs` 427s PASS、`research_topic` 1260s FAILED——**两场景全程 0 次 web_search/fetch_url**,证明 05-26「环境工具坏」归因错了。真根因 = 主 agent **编排认知 + 结果提取协议**:看不懂 completed 子 agent 结果就在 eval 返回里 → 反复重 spawn(research 开了 9 个)永不收敛撞超时。工具修不好它。

**结论**: 并行 fan-out 在本地弱模型不可用,只留单 subagent(context isolation)+ 串行单任务。**subagent 行为调优交 owner**;本轮验证侧只保留必要改动,其余 fan-out 调参(`max_steps=12`/`elapsed=600`/budget prompt)全部弃回 committed。

**保留的改动(已落工作区,待提交)**:
- **通用 bug / 环境**(与 agent 数无关): ① `web_search` bing decode(已提 **PR #2245**); ② **fetch_url fake-ip SSRF-safe 重设计**——按 **IP 段**信任 fake-ip 占位段(`network_policy` 加 `with_trusted_fakeip_cidrs`/`is_trusted_fakeip_addr` + `fetch_url validate_dns_resolved_ip` + bridge `with_trusted_fakeip_cidrs(["198.18.0.0/15"])`),只放行 198.18.x 占位、真实私网/元数据仍拦。**早期 `proxy=["*"]` + `*` 通配已废弃**(会放行任意域名→内网,SSRF)。
- **3 个单 subagent 必需**: `tool_agent` model 路由(本地没 `deepseek-v4-flash` 否则 404)、bridge `subagent_api_timeout=300`(本地慢推理 120s 误杀单步)、`resolve_agent_ref` 截断前缀容错
- **已弃**: `fetch_url` 30s+retry(env-specific 且未验证)、所有 fan-out 调参(`max_steps=12`/`elapsed=600`/budget prompt)
- ⚠️ **给 owner 的提醒**: 单 subagent 长任务被 `max_steps`+`elapsed` 双重掐(本地 40-70s/步下 elapsed 600s 仅 ~10 步,光抬 max_steps 无效需同抬 elapsed)。已弃回 committed,owner 调优时重新放宽。

---

## 阶段 K(react-ui-migration 分支): 同事新版 React UI 合并 + 全能力移植

**背景**: UI 同事基于旧版 9006490 重写了一套 **纯 React UI**(JSX-in-Babel 内联 index.html + Tailwind CDN
+ tauri-bridge.js pub/sub), 但比当时 main 的 vanilla `main.js`(4053 行)落后一个版本且砍掉了大量能力。
需求: 用同事的 React 框架替换 vanilla 前端, 跑在 main 现有后端上, 并把 main 全部能力移植过来 + 补 workflow。

**做法**(worktree `react-ui-migration`, 基于 main f43a279):
- 底座: 落地同事 React index.html + tauri-bridge.js(CRLF→LF), 删 vanilla main.js/chat.css(死代码)
- 设计原则: **bridge 做唯一状态源, React 纯渲染**; 各种卡片(plan/用户交互/careful/品悟)作为
  特殊类型 chatItem 按序插入聊天流; 持久 mode/workflow 状态放 state 顶层
- bridge 从 ~16 命令扩到 main 前端全部 37 命令 + 14 事件
- 移植: Plan/YOLO 双模式 · 用户交互卡 · careful 拦截 · 产物面板(md/html iframe/text 预览) ·
  附件/粘贴图 · 编辑上一轮 · 手动压缩 · 品悟审批 v2(GATE→紫气泡→3按钮 override) · **workflow 视图**
  (skill 卡片网格 + phase chip pipeline 三态 + chip 只读预览 + demo modal + session 绑定标签)
- 校验: 无法跑 GUI, 用 `@babel/standalone` 离线编译校验 JSX(每步) + `node --check` bridge

**已知待办**(留给后续):
- ⚠️ **CDN 依赖**: 新 index.html 从 unpkg/cdn 加载 React/Tailwind/Babel, 离线/内网跑不起来 —
  GB10 本地产品上线前需 vendor 化(下载到 vendor/ 本地引用)
- i18n: 新 UI 沿用同事的极简 dict(chrome 中英双语), 移植的卡片用中文领域术语(品悟/方案/工作流)未做
  英文版 — 与同事 ToolCard 等硬编码中文一致, 产品中文优先, 视需要再补
- 主题/语言: 启动未从 saved settings 同步 activeTheme/language(同事原代码遗留, 非本次回归)

---

## 相关文档

- `docs/L1-judge-rubric.md` — Judge 评分 rubric (r2)
- `docs/l1-baselines/README.md` — Baseline 命名约定 + 已有 baseline 表
- `docs/l1-baselines/v0.8.37-vllm3_final-r2-18scn/judge-report.md` — 最终评分报告
- `docs/自动化测试方案.md` — 测试系统现状描述
- `docs/工具表精简方案.md` — 阶段 E 详尽方案 + 附件 pipeline §6.1 + 视觉模型 §6.2
- `docs/system-prompt-与底座的差异.md` — system prompt 分层 (部分内容被阶段 F lesson learned 推翻)
- `docs/DeepSeek-TUI-架构详解.md` — 底座详尽解析
- `docs/验证报告-qwen3.6-deepseek-tui.md` — 阶段 A 实证报告
