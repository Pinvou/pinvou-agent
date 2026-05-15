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
