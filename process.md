# pinvou3 进度记录

跨阶段待办、决策、follow-up 集中地。
决策细节走 git commit + `~/.claude/plans/`，这里只放**需要单独排期**的事。

---

## 已完成阶段

| 阶段 | 节点 | 简述 |
|---|---|---|
| A | 验证 | Qwen3.6 + DeepSeek-TUI 5/5 验证通过 |
| B | commit `7d9cde1` | Tauri MVP + bridge 抽象层 + 4-view 壳 + 三主题 |
| C | commit `6f160a5` → `8ff8fa2` | 多对话 / 产物面板 / 文件上传 / token 警告条 / 敏感目录 hook / 自定义 modal / per-session workspace 隔离 / file watcher 自动跟踪产物 |

阶段 C 全部 task 闭合（含 #34 / #35 / 多个用户反馈 bug）。

---

## 下一步候选

按推荐顺序：

### A. Plan / YOLO 双模式（已拍板设计，待实施）
- 完整决策文档：`docs/Plan-YOLO双模式-设计决策.md`
- 复用底座 Plan mode + update_plan 工具 + Op::SendMessage{mode} 字段
- pinvou3 增量：bridge 状态机 + [💡] 按钮 + 消息流内嵌 plan_card + chip [⚡ 直接动手]
- 工作量 2.7 天，0 侵入 DeepSeek-TUI

### B. WorkFlow 视图（最重，差异化最强）
- plan 里 Phase D 重头戏，用户明说「具体设计后续探讨」
- 需先讨论交互模型：todo checklist / 可视化节点流 / 专家协作？
- 跟 A 项 plan_card 可能融合
- 探讨完再 2-3 天实现

### C. AI 行为加固
- `instructions.md` 防 hallucination（曾出现 AI 声称写文件但没真调 write_file / 字号注释跟代码不一致）
- 中文字号准确性引导（小六 = 6.5pt 不是 15pt）
- 缩减 baseline tools（见下方独立项）

### D. bundle 领域 skill
- 把常见场景（旅游规划 / 文档总结 / 翻译润色 / 数据分析）做成 SKILL.md
- 让 AI 应对高频场景更稳

### E. 模型预设切换 GUI
- 远程 deepseek API / OpenRouter
- bridge 已有 `ModelPreset` 占位，只差 GUI

---

## 长期搁置项

### 缩减 baseline tools schema 占用（延后）

**问题**：deepseek-tui 默认注册 41 个工具，baseline 占 ~28k tokens / 66k 上下文 = **44%**。普通用户用不到 90% 的 tools。

**方案**：bridge `build_engine_config` 关闭 `ApplyPatch / Subagents / ExecPolicy` 三个 Feature。保留 `ShellTool / WebSearch / Mcp`。预计 baseline 降到 ~20-23k（占比 30-35%）。

实施位置：`pinvou3-app/src-tauri/src/bridge/mod.rs::build_engine_config`，约 10 行：

```rust
let mut features = features;
features.disable(deepseek_tui::features::Feature::ApplyPatch);
features.disable(deepseek_tui::features::Feature::Subagents);
features.disable(deepseek_tui::features::Feature::ExecPolicy);
```

**为什么不做**：当前 compaction 已兜底，先观察实际使用是否真触顶。

**搭配（用户侧）**：vLLM 启动加 `--max-model-len 131072`（Qwen3.6 支持 128k），baseline 占比直接降到 22%，不动 pinvou3 代码。

### 产物面板内嵌预览 docx/pdf/xlsx（已尝试，回退）

复用 file_ingest pandoc/pdftotext 转 md 在右栏内嵌。技术可行（commit 范围内做过），但用户反馈不需要——这类文件保留「↗ 用系统应用打开」体验已足够。`commands::artifact_info` 的 office kind 分类保留作未来钩子。
