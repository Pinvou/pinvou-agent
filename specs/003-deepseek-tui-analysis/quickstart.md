# 快速开始：验证 DeepSeek-TUI 源码职责分析交付

## 1. 确认当前 feature

```powershell
git branch --show-current
Get-Content .specify/feature.json
```

预期：
- 分支为 `003-deepseek-tui-analysis`。
- `feature_directory` 指向 `specs/003-deepseek-tui-analysis`。

## 2. 检查计划工件

```powershell
Get-ChildItem specs/003-deepseek-tui-analysis
Get-ChildItem specs/003-deepseek-tui-analysis/contracts
```

预期至少存在：
- `spec.md`
- `plan.md`
- `research.md`
- `data-model.md`
- `quickstart.md`
- `contracts/analysis-document.md`

## 3. 实现阶段应生成最终文档

目标文件：

```text
docs/DeepSeek-TUI源码职责分析.md
```

## 4. 文档契约验收

阅读目标文档，检查：

- 是否为中文主体。
- 是否说明 DeepSeek-TUI 是 pinvou3 的 agent 底座。
- 是否覆盖 DeepSeek-TUI 工作区主要 crate。
- 是否覆盖 pinvou3-app 的 bridge、engine、engine_pool、commands、sessions 接入点。
- 是否至少包含 12 个源码证据点。
- 是否至少包含 6 条关键调用链。
- 是否至少包含 8 条维护风险或检查项。
- 是否明确哪些能力不应在 pinvou3-app 重复实现。
- 是否提供按问题类型排查的源码入口。

## 5. 确认没有误改业务代码

```powershell
git status --short
```

预期：
- 实现该 feature 时，除 `docs/` 和 `specs/003-deepseek-tui-analysis/` 相关文件外，不应出现业务代码改动。

## 6. 可选源码证据抽查

```powershell
rg "deepseek_tui::core::engine|EngineConfig|SessionManager|SkillRegistry|SpawnSubAgent" pinvou3-app/src-tauri/src
rg --files DeepSeek-TUI/crates
```

预期：
- 能找到 pinvou3-app 对 DeepSeek-TUI 的关键接入点。
- 能看到 DeepSeek-TUI 工作区的 crate 结构。
