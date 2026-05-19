# pinvou3 进度记录

跨阶段待办、决策、follow-up 集中地。决策细节走 git commit + `docs/`,这里只放**需要单独排期**的事。

最后更新: 2026-05-19

---

## 当前状态

- **main HEAD**: `5f53848` (阶段 H — submodule bump + careful Dangerous BLOCK)
- **fork HEAD**: `b2f6ef56` (`pinvou3-patches` 分支, 14 commits ahead `origin/main`)
- **进行中**: 等 PR [#1790](https://github.com/Hmbown/DeepSeek-TUI/pull/1790) (file_search timeout) 上游反馈
- **worktrees**: 无

---

## 已完成阶段

> 决策细节走 git log,这里只留**定位 + 数据点 + 文档兜底**。

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
- **Multi-subagent 大研究任务** (subagent_compare_3_libs / research_topic 仍 cargo timeout): stress test 边界,生产场景用 single subagent 已够。若真要支持需要 prompt 工程减 verbosity + 用户接受 10+ 分钟等待
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

## Fork → 上游 PR roadmap (2026-05-19 整理,阶段 G/H 后)

Fork 现 14 commits ahead `origin/main` (Hmbown upstream):

| 状态 | Commit | 内容 | 备注 |
|---|---|---|---|
| ✅ 已 PR | `d866274` | file_search spawn_blocking + 30s timeout (英文 clean 版) | **PR #1790** + Issue #1791, 等反馈 |
| 🟢 强适合 PR | `363dd35` | subagent completion role=system→user (修 Qwen 严格 chat_template 400) | 通用 bug fix, 纯净 7 行, **PR #1790 接受后立即提** |
| 🟢 强适合 PR | `9860ef1` | `DEFAULT_MAX_STEPS` 100→20 + `DEFAULT_SUBAGENT_ELAPSED_MAX` 300s | 对齐 CrewAI 行业共识; 改默认值有争议, **先开 issue 讨论再 PR** |
| 🟢 强适合 PR | `7e5288e3` | B1+B2: `_Nk` hint 提到所有 vendor 前 + `context_input_budget` 按窗口分级 reserved | **通用 bug,只影响 <500K 窗口模型**(V4 1M 路径不变,有测试锁定);自托管/小窗口社区现成受益方,**PR #1790 合后单独 PR** |
| 🟡 待 PR | `aaa1920` | grep_files spawn_blocking + 30s timeout (中文,需英文 clean) | 等 #1790 反馈; 若 reviewer 问其他同步 tool 就合进或单独 PR |
| 🟡 弱适合 | `b2f6ef56` | careful: shell Dangerous 命令在 YOLO 模式下也 BLOCKED | 强 pinvou3 业务语义(careful hook 是品悟 v2 配套),上游接受度低,**不主动 PR** |
| 🟡 弱适合 | `15244e6` | `GENERAL_AGENT_INTRO` 加 stop-on-failure 条款 | prompt 改动 PR 社区接受度低, ROI 低, **不主动 PR** |
| 🟡 弱适合 | `dd879db` | `#[cfg(test)] pub mod test_support` | 不常见模式, reviewer 可能质疑, **不主动 PR** |
| ❌ fork-only | `6ac5b97` `47e6abc` | lib export internal modules + RPIT trait | pinvou3-app Rust wrap 专用 |
| ❌ fork-only | `93e9474` | `DEEPSEEK_MAX_OUTPUT_TOKENS` env override | pinvou3 vLLM 撞顶专用 |
| ❌ fork-only | `b9b40ce` `36526ce` `1ba8e41` | blocklist 52 工具 / tool_catalog 优先 / `PINVOU3_BLOCKLIST_OVERRIDE` env | pinvou3 业务定制 |

**下次开工动作**:
1. `gh pr view 1790 --repo Hmbown/DeepSeek-TUI` 查状态
2. 合了 → cherry-pick `363dd35` 到新 PR 分支,跑 fork `cargo test` 后提 PR
3. `7e5288e3` (B1+B2) 单独 PR,卖点"自托管/小窗口模型 auto compact 失效";改动局部、有现成测试,接受度比 `9860ef1` 高
4. `9860ef1` 先开 issue 讨论默认值 (20 还是 30/50),收 maintainer 意见再 PR

---

## 已知问题 / 边界

### subagent 边界
- ✅ **single subagent 完全可用** (context isolation use case, 验证 19s/75s 完成 5.00 分)
- ✅ **2-3 subagent 串行可用** (one_fails 377s PASS 4.83 分, 主 agent 用 checklist_write 拆步)
- 🔒 **multi-subagent 并发**: 工程层 `max_subagents=1` 硬锁, LLM 第 2 个 spawn 拿 "Sub-agent limit reached" 自然 fallback
- ⚠️ **3+ long-prompt subagent + 需联网**: 当前 setup 不可用 (compare/research scenarios 5-10 分钟超 cargo cap)

### LLM 行为不稳 (vLLM 抽奖)
- `chinese_idiomatic` 偶发 detour 写文件 (3/5 次出现)
- `long_output_1500` 偶发 detour 调 web_search
- 跨 baseline 均值看是 noise, 单 sample 信号不下结论

### 工具同步阻塞
- ✅ `file_search` / `grep_files` 已修 (fork patch + 上游 PR)
- ⚠️ `list_dir` 单层 read_dir 通常 OK 但极端目录边缘 case 可能

### Judge 自身局限
- Claude 跟 Qwen 都是 LLM, 某些"模型味"Claude 看不出来
- 单 session retake 是"演习"不是真 sanity (需新 session 重评)
- ±0.2 是 noise, ±0.5 才算 signal

---

## L1 judge 离群点跟进

> 流程见 `docs/L1-judge-rubric.md` §3 Step 5。任一维度 ≤3 → append 至此。

### 🆕 / 🔁 待处理

暂无。新离群点出现时按 `docs/L1-judge-rubric.md` §3 Step 5 流程 append。

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
