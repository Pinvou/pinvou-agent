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
- **状态**: 🧪 已尝试改造 reminder (commit `<commit-hash-pending>`,Planning §5 加禁过渡语条款),但 run 1779078816 LLM 仍写了 5 次 "让我..." —— 单靠 reminder 改不动 LLM "复述思考过程" 这类 mode-independent 行为,需 prompt 工程层或 fine-tune 跟进。**降级追踪**:等下次完整 baseline 跑看群体趋势。

### 2026-05-18 · run 1779077762-r1 · plan_mode_list_dir · 完整性 3/5
- **问题**: final text 只一句 "输出被截断了，让我获取完整的目录列表信息。" 像是 turn 没结束就 turn_complete,用户得不到方案 summary,只能看 plan 卡片
- **改进方向**: 同上 (Plan/Planning system-reminder 加 "调 update_plan 后 text 必须给方案 summary 不能悬空,不要说'让我...'之类下一步动作意图")
- **状态**: ✅ 2026-05-18 已改善 — reminder 改造后 run 1779078816 final text 给出具体分类描述(477 条目/pinvou3-l1- 前缀/系统目录),完整性从 3/5 升到 5/5 (单次样本,需完整 baseline 复核)

### 2026-05-18 · run 1779077762-r1 · plan_travel_web · 工具使用 3/5
- **问题**: prompt 明确要求"用 update_plan 给我一个 3 天行程方案",LLM 用 text 表格替代直接交付,没调 update_plan。web_search 4 次全失败 (Bing 0 结果 + 网络 err) 后也没换 fetch_url 等其他工具
- **改进方向**: INSTRUCTIONS_MD 加引导 "prompt 明示要用某工具(如 update_plan),即便数据不足也要调,可以基于常识填内容"。web_search 失败后可尝试 fetch_url 直接拿某个景点 url 内容
- **状态**: 🆕 待处理
