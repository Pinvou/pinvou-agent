# 快速开始：使用 Windows 迁移与维护接手手册

## 1. 阅读主文档

打开：

```text
docs/Windows迁移与维护接手手册.md
```

如果你是第一次接触项目，请按章节顺序阅读。即使你熟悉 Tauri 和 Rust，也建议完整阅读 DeepSeek-TUI fork、Windows 风险和维护任务路由章节。

## 2. 确认 Spec Kit 上下文

当前 feature 产物位于：

```text
specs/001-windows-onboarding-docs/
```

关键文件：

```text
spec.md
plan.md
research.md
data-model.md
contracts/documentation-contract.md
quickstart.md
tasks.md
```

## 3. 验证模板占位

运行占位扫描：

```powershell
$patterns = @(
  'NEEDS' + ' CLARIFICATION',
  '[' + 'FEATURE NAME' + ']',
  '$' + 'ARGUMENTS',
  'ACTION' + ' REQUIRED',
  'REMOVE' + ' IF UNUSED',
  'T' + 'XXX'
)
rg -n ($patterns -join '|') specs\\001-windows-onboarding-docs docs\\Windows迁移与维护接手手册.md
```

预期结果：`spec.md`、`plan.md`、`research.md`、`data-model.md`、`quickstart.md`、`tasks.md` 和主文档中没有未解决模板占位。

## 4. 验证中文文档优先

运行中文优先抽查：

```powershell
$englishTemplateTerms = @(
  'Implementation' + ' Plan',
  'Technical' + ' Context',
  'Project' + ' Structure',
  'Complexity' + ' Tracking',
  'Required' + ' Coverage',
  'Validation' + ' Rules',
  'Alternatives' + ' considered',
  'Ration' + 'ale'
)
rg -n ($englishTemplateTerms -join '|') specs\\001-windows-onboarding-docs docs\\Windows迁移与维护接手手册.md
```

预期结果：不应出现可中文表达的英文模板标题或说明。命令、路径、API 字段、工具名和外部专有名词可以保留英文。

## 5. 使用接手手册做 Windows 规划

从 `docs/Windows迁移与维护接手手册.md` 的风险表开始，将后续实现任务分组为：

- P0：submodule、构建、打包、用户目录、依赖探测、安装器和 updater 阻塞项。
- P1：外部工具、脚本、workflow 路径、文件/文件夹打开、vLLM 连通性。
- P2：prompt/path 体验和 Windows 下 fork guard 便利性。

## 6. 继续实现拆解

当前 feature 的任务清单在：

```text
specs/001-windows-onboarding-docs/tasks.md
```

若后续要进入实际 Windows 兼容代码改造，应为代码迁移创建新的规格，避免把文档交付和运行时迁移混成一个不可验收的大任务。
