# pinvou3 进度记录

跨阶段待办、决策、follow-up 集中地。
决策细节走 git commit + `docs/`，这里只放**需要单独排期**的事。

---

## 已完成阶段

### 阶段 E：工具表精简 + GUI 体验优化（main `e30b64c`，fork `8e9be9c7`）

- **L1.5 工具表精简**：LLM 可见 85→16+4，schema ~119KB→14KB，翻译 19s→8s
- **OpenAI streaming batch tool_calls 修复** + 上游 PR [#1686](https://github.com/Hmbown/DeepSeek-TUI/pull/1686)
- **路径校验放宽**（commands.rs `validate_user_path` A 方案）允许 /tmp 等
- **Plan 模式 `trust_mode=true`**（修 list_dir 跨 workspace PathEscape）
- **产物 📂 打开所在目录** 按钮（xdg-open parent dir）
- **INSTRUCTIONS_MD workspace 引导** 防止 AI 浪费 turn list_dir 上层

详见 `docs/工具表精简方案.md`。

---

## 进行中

- 等 PR #1686 上游反馈（被动等）

---

## 下一步候选

按推荐顺序：

### A. WorkFlow 视图编排（最重 / 差异化最强）

- 用户明说"准备做编排，要细聊"
- 需先讨论交互模型：todo checklist / 可视化节点流 / 专家协作
- 跟现有 `plan_card` 可能融合
- 设计落定再 2-3 天实现

### A2. 自动化测试 L1+L2（紧迫性高）

每次改 INSTRUCTIONS_MD / bridge / blocklist 都可能 regression，目前只能靠手测。
方案已落档 `docs/自动化测试方案.md`（v2 修正版，按决策 Y/D）：

- **L2 backend 纯函数测试** 9 个，1 天，每 PR `cargo test` 跑 CI block
- **L1 plumbing 改造** 2 处 fork patch（`bridge::boot_with_workspace` + `AppEngine::spawn_headless`），半天
- **L1 MVP** 真 vLLM 5 个 scenario + 健康探针 + DEFAULT_OUTPUT_NEVER，2.5-3 天
- L1 完整 11 scenario 排 M2
- L3 GUI / AI-as-judge 暂缓

### B. 附件预处理 pipeline

bridge 拦截 docx/pdf/image 上传 → pandoc/tesseract 转 md → 嵌 user message。
省 5-10k token/turn + UX 直拿数据 + AI 无需 pandoc_convert/image_ocr 工具决策。
1-2 天实施，详见 `docs/工具表精简方案.md` §6.1。

### C. reminder vs XML 标签 去重观察

观察 1 周（3-5 个 Plan + Yolo 任务）看 Qwen3.6 实际行为：
- 数据正常 → 保留现状分层共存
- reminder 被 XML 削弱 → 砍 Yolo Executing reminder 重复部分
- Plan Planning 失约束 → reminder 加"覆盖 `<act_dont_ask>`"声明

详见 `docs/system-prompt-与底座的差异.md` §8。

### D. 模型预设切换 GUI

bridge 已有 `ModelPreset` 占位，缺 GUI。支持远程 DeepSeek API / OpenRouter。

### E. 视觉模型补足

GB10 同机起 Qwen-VL-Chat 7B / InternVL2-2B（vLLM 并存）→ bridge 自动 caption。
详见 `docs/工具表精简方案.md` §6.2。

### F. AI 行为加固剩余

- 中文字号准确性引导（小六 = 6.5pt 不是 15pt）

---

## 已决策不做

- **上游 P2/P3 PR**：`DEEPSEEK_MAX_OUTPUT_TOKENS` env override 和 lib `pub mod` 暴露——pinvou3 偏好不是生态价值，fork 自留更干净
- **产物内嵌预览 docx/pdf/xlsx**：已尝试回退，`↗` 用系统应用打开体验已够

---

## 基建待办（不阻塞功能）

- **GB10 self-hosted GitHub Actions runner**：让 L1 真 vLLM 测试能进 CI nightly（详 `docs/自动化测试方案.md` §8）。优先级低，等团队≥2 人或发版频率上升再做

---

## L1 judge 离群点跟进 (auto-append by Claude)

> 流程见 `docs/L1-judge-rubric.md` §3 Step 5。任一维度 ≤3 → append 至此。

### 2026-05-18 · run 1779074272-r1 · plan_mode_list_dir · 简洁性 3/5
- **问题**: final text 三句话 "先看看 /tmp 目录的情况" / "结果太多了(66KB被截断)" / "我先做个统计分析" 有跳跃感且语义打架——已 update_plan 还说"我先做个统计分析",给用户感觉方案还没出
- **改进方向**: Plan/Planning 的 system-reminder 加一句 "已调 update_plan 就别再说'我先...'之类的过渡语,直接交付方案"
- **状态**: 🔁 2026-05-18 又出现一次 (run 1779077762-r1) —— Plan 模式 text 悬空问题持续。**已尝试改 reminder 失败,见下方 lesson learned**

### 2026-05-18 · run 1779077762-r1 · plan_mode_list_dir · 完整性 3/5
- **问题**: final text 只一句 "输出被截断了，让我获取完整的目录列表信息。" 像是 turn 没结束就 turn_complete,用户得不到方案 summary,只能看 plan 卡片
- **改进方向**: 同上 (Plan/Planning system-reminder 加 "调 update_plan 后 text 必须给方案 summary 不能悬空,不要说'让我...'之类下一步动作意图")
- **状态**: 🆕 待处理 (跟简洁性同因,见 lesson learned)

### 🧪 Lesson learned 2026-05-18: reminder 改 ≠ LLM 行为改

尝试: commit `7b983b6` 给 Plan/Planning system-reminder 加第 5 条 "调完 update_plan 后必须给方案 summary,禁止过渡语"。

单 scenario 验证 (run 1779078816):
- 完整性 3→5 ✅ (有具体分类描述了)
- 简洁性 3→3 持平 (LLM 仍写了 **5 次 "让我..."**,reminder 禁过渡语没起作用)
- text 暴增 **23 → 556 字** (+24×) — reminder "必须给 summary" 反向起作用,LLM 多写了

判断: 副作用 (output 膨胀 + system prompt 占位) 大于改进 (单样本完整性可能是 LLM 抽奖)。**回滚** (commit `ec2f788` revert)。

学到的:
- **prompt 工程层改 LLM 行为**通常需多 scenario 多次跑验证,单样本不可信
- LLM "复述思考过程" (写 "让我...") 是 mode-independent 行为,单靠 reminder 改不动
- 加 reminder 条款的 ROI 要看 (a) 群体改善是否 ≥ ±0.5 (b) 副作用 (token + system prompt 长度)
- 未来类似离群点先**积累 3+ 次出现证据**再考虑改 reminder; 真要改要在 2+ scenario 上 A/B 对比验证

剩 `plan_travel_web 工具使用 3/5` 待办仍是 🆕,优先级降低 (同样原因:单样本不一定可信)。

### 2026-05-18 · run 1779077762-r1 · plan_travel_web · 工具使用 3/5
- **问题**: prompt 明确要求"用 update_plan 给我一个 3 天行程方案",LLM 用 text 表格替代直接交付,没调 update_plan。web_search 4 次全失败 (Bing 0 结果 + 网络 err) 后也没换 fetch_url 等其他工具
- **改进方向**: INSTRUCTIONS_MD 加引导 "prompt 明示要用某工具(如 update_plan),即便数据不足也要调,可以基于常识填内容"。web_search 失败后可尝试 fetch_url 直接拿某个景点 url 内容
- **状态**: ✅ vLLM 参数优化后自动修复 (run 1779089467-r2 同 prompt LLM 正确调了 update_plan,工具 5/5)。零代码改动免费红利

### 2026-05-18 · run 1779089467-r2 · vLLM subagent SSE 调度问题 (🆕 紧急)
- **问题**: 4 个 subagent scenarios 里 **3 个 subagent 全 SSE 失败** (compare/research/one_fails 共 11+ 个 agent_eval 大多失败或 timeout)。新 vLLM 配置 `max-num-seqs 8 + enable-chunked-prefill + max-num-batched-tokens 32768` 下,多 subagent 并发跑时 SSE 不稳。**跟模型能力无关,是底座调度问题**
- **改进方向**:
  1. 排查 vLLM 日志看 SSE 超时根因 (具体哪个 sequence / chunked-prefill 是否撞 batched-tokens 限制)
  2. 尝试调参:`max-num-seqs 16` 或 `--disable-chunked-prefill` 看是否改善
  3. 如果是 OpenAI streaming bug 类似上游问题,提 PR 给 vLLM
- **状态**: 🆕 待处理 — **subagent 能力评估的真实瓶颈**

### 2026-05-18 · run 1779089467-r2 · subagent_research_topic · 完整性 2/5 + 简洁性 2/5
- **问题**: 主 agent fallback 决策慢 — subagent 失败后等了 **8 分钟** 才决定用自身知识降级,最终综述被 timeout 截断 (532s)
- **改进方向**: 依赖 vLLM 调度问题先解决。如果 vLLM 修不了,prompt 工程引导"subagent failed/无进展 1 分钟立即 fallback"
- **状态**: 🆕 待处理 (低优先,等 vLLM 修了再看是否还存在)

### 2026-05-18 · run 1779089467-r2 · subagent_compare_3_libs · 综合 2/5 + 简洁性 2/5 + 完整性 2/5
- **问题**: 660s timeout,subagent SSE 失败 → 重派 (name collision "already in use" 错误) → 主 agent 用 22 次 exec_shell + curl API 拿真数据 → 综合报告被截断
- **改进方向**: 同上,依赖 vLLM 调度修
- **状态**: 🆕 待处理 (低优先)

### 2026-05-18 · run 1779089467-r2 · plan_mode_list_dir · 简洁性 3/5
- **问题**: 🔁 **第 3 次出现** (run 1779074272-r1 / 1779077762-r1 / 1779089467-r2),Plan 模式 text 仍有"让我..."过渡语
- **状态**: 🔁 持续追踪,不再尝试 prompt 工程改造 (lesson learned 已验证 reminder 改造 ROI 负)

### 2026-05-18 · run 1779095350-r2 · 三 bug 诊断完结 + subagent 链路恢复

经 SSH vLLM logs + 代码定位 + 验证 3 个底座 bug,2 个修了 1 个是 vLLM 调度 trade-off:

| Bug | 位置 | 状态 |
|---|---|---|
| **#1 role=system 触发 chat_template raise** | `DeepSeek-TUI/.../turn_loop.rs:1946` `subagent_completion_runtime_message` | ✅ fork commit `363dd35` (role system→user) |
| **#2 stream_open_timeout 45s 太短** | `DeepSeek-TUI/.../client/chat.rs:26` default | ✅ harness `DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS=180` |
| **#3 多 subagent + chunked-prefill 调度累积** | vLLM 配置 trade-off | ⚠️ ≥3 subagent + 长 prompt 在 max-num-seqs=8 下 >10 分钟。可调 `--disable-chunked-prefill` 或 `--max-num-seqs 16`,但是 latency vs throughput 拍板 |

**修复效果** (run 1779095350-r2):
- ✅ `subagent_single_simple` 新加 → 19s 完成 5.00 分
- ✅ `subagent_one_fails` 4.50 → 5.00 (主 agent 真重派失败 subagent)
- ⚠️ `subagent_compare_3_libs` / `subagent_research_topic` 仍 cargo timeout — Bug #3 限制
- ⚠️ 老 scenarios 抽奖 regression (multi_turn_t2 / plan_travel_web / chinese_idiomatic 各掉一点) — LLM 输出本质不确定,单次跑不能下结论

**上游 PR 候选**: Bug #1 + Bug #2 都是通用 fix (适用所有 Qwen3.6 严格 chat_template + 长 prompt + subagent 场景),值得提 Hmbown/DeepSeek-TUI PR。

### 2026-05-18 · run 1779095350-r2 · subagent_compare_3_libs / subagent_research_topic · cargo timeout
- **问题**: 3 个 long-prompt subagent 同时 prefill,主 agent 4-5 次 agent_eval (block, timeout_ms=60s) 都拿到 status=running。subagent 内部 prefill 排队,完成时间 >10 分钟超过 cargo test max_duration_s=600s
- **改进方向**: vLLM 调参 `--disable-chunked-prefill` (牺牲 throughput) 或 `--max-num-seqs 16` (允许更多并发);或 prompt 工程让主 agent 用更短的 subagent prompt
- **状态**: ⚠️ vLLM 调度限制 — 用户拍板调参 vs 接受 multi-subagent 大任务 partial

### 2026-05-18 · run 1779095350-r2 · multi_turn_context_t2 · 简洁性 2/5 + 工具 3/5
- **问题**: 用 exec_shell python3 算 2026-1990 overkill,且复述两遍"今天是你36岁生日"
- **状态**: 🆕 待处理 (LLM 单次抽奖,3+ 次再考虑 prompt 工程)

### 2026-05-18 · run 1779095350-r2 · plan_travel_web · 工具 3/5
- **问题**: 🔁 没调 update_plan (vllm2-r2-17scn 自动修了,这次又退回)
- **状态**: 🔁 LLM 行为不稳累积证据,等更多样本
