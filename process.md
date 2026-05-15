# pinvou3 进度记录

跨阶段待办、决策、follow-up 集中地。
决策细节走 git commit + `docs/`，这里只放**需要单独排期**的事。

---

## 已完成阶段

---

## 进行中

### DeepSeek-TUI 升 v0.8.37 收益消化（commit `c677c7b`）

268 个上游 commit rebase 上来，37 个 feat。绿区高价值大部分**自动生效**（prefix_cache / artifact metadata / pandoc_convert / read_file chunked / read_file PDF bundled）。手动 wire-in 已落地：

- ✅ XML 执行纪律 5 标签 + 工具使用强制 → 抄进 INSTRUCTIONS_MD（命中"AI 行为加固"）
- ✅ 验证 `code_execution` 工具真存在（实测 123×456 命中）
- 📋 系统 prompt 设计差异详细文档 → `docs/system-prompt-与底座的差异.md`

剩余跟进项见下方"下一步候选 A"。

---

## 下一步候选

按推荐顺序：

### A. per-turn `<system-reminder>` vs XML 标签 去重/调和

**问题**：
- Yolo Executing reminder 跟 XML `<tool_persistence>` / "工具使用强制" 内容重叠（每 turn 浪费 ~300 token）
- Plan Planning reminder 第 1 条"歧义先调 `request_user_input`" 跟 XML `<act_dont_ask>` 倾向相反，潜在冲突

**先观察一周**：跑 3-5 个真实 Plan + Yolo 任务看 Qwen3.6 实际行为：
- Plan Planning 还会不会按 reminder 调 `request_user_input`
- Yolo Executing 还会不会"嘴炮不真调 write_file"

**再决方案**：
- 数据正常 → 保留现状，分层共存
- reminder 被 XML 削弱 → 砍 Yolo Executing reminder 重复部分（保留业务流程 2 条）
- Plan Planning 失约束 → reminder 加"覆盖 `<act_dont_ask>`" 显式声明

详见 `docs/system-prompt-与底座的差异.md` §8。

### B. WorkFlow 视图（最重，差异化最强）

- 阶段 D 之后的重头戏，用户明说"具体设计后续探讨"
- 需先讨论交互模型：todo checklist / 可视化节点流 / 专家协作？
- 跟现有 plan_card 可能融合
- 探讨完再 2-3 天实现

### C. AI 行为加固（剩余项）

- ✅ XML 执行纪律标签（已完成）
- 📋 中文字号准确性引导（小六 = 6.5pt 不是 15pt）
- 📋 缩减 baseline tools 显式裁剪（见下方"长期搁置项"升级讨论）

### E. 模型预设切换 GUI

- 远程 deepseek API / OpenRouter
- bridge 已有 `ModelPreset` 占位，只差 GUI

---

## 长期搁置项

### 缩减 baseline tools schema 占用（升级讨论）

**v0.8.37 升级后新发现**：pinvou3 Yolo 模式实际暴露 ~60/61 个内置工具，INSTRUCTIONS_MD 只显式提及 7 个。Qwen3.6 看到 RLM session / sub-agent / 自动化 / PR 管理等高级工具会去试，但 pinvou3 没相应 UI 支撑。

**方案**：bridge `build_engine_config` 关闭 `ApplyPatch / Subagents / ExecPolicy` 三个 Feature。保留 `ShellTool / WebSearch / Mcp`。预计 baseline 从 ~28k → ~20-23k token。

**实施位置**：`pinvou3-app/src-tauri/src/bridge/mod.rs::build_engine_config`，约 10 行：

```rust
let mut features = features;
features.disable(deepseek_tui::features::Feature::ApplyPatch);
features.disable(deepseek_tui::features::Feature::Subagents);
features.disable(deepseek_tui::features::Feature::ExecPolicy);
```

**前置依赖**：先让候选 A 的 XML 标签观察期过完（确认 AI 行为加固在 ~60 工具暴露下也稳），再做工具裁剪。否则两个变量同时动没法归因。

**搭配（用户侧）**：vLLM 启动加 `--max-model-len 131072`（Qwen3.6 支持 128k），baseline 占比直接降到 22%，不动 pinvou3 代码。

### 产物面板内嵌预览 docx/pdf/xlsx（已尝试，回退）

复用 file_ingest pandoc/pdftotext 转 md 在右栏内嵌。技术可行（commit 范围内做过），但用户反馈不需要——这类文件保留「↗ 用系统应用打开」体验已足够。`commands::artifact_info` 的 office kind 分类保留作未来钩子。

**v0.8.37 升级后状态**：上游加了 `pandoc_convert` 工具（运行时探测 pandoc 二进制，存在自动注册）。如果未来要重启这条线，**不要重写 ingest 逻辑**，直接复用底座 `pandoc_convert` 工具。
