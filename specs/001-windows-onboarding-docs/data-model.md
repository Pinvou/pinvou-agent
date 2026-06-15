# 数据模型：Windows 迁移与接手维护文档

本 feature 不新增应用数据库或运行时数据结构。以下模型定义的是文档需要覆盖和保持一致的概念实体。

## 实体：接手文档

**说明**：面向 Windows 应用开发工程师的主交接文档。

**字段**：

- `title`：文档标题。
- `audience`：目标读者及其接手场景。
- `generated_date`：文档生成或最近实质更新日期。
- `source_basis`：作为依据的仓库文件、源码和已有文档。
- `sections`：文档章节列表。
- `maintenance_note`：后续维护者如何刷新文档。

**验证规则**：

- 必须使用中文作为主语言。
- 必须引用当前真实项目结构。
- 必须区分当前事实和未来迁移建议。
- 不得建议在 pinvou3 中重写 DeepSeek-TUI 已有底座能力。

## 实体：模块概览

**说明**：读者必须理解的仓库目录、源码模块或外部组件。

**字段**：

- `path`：仓库相对路径或外部组件名称。
- `responsibility`：模块职责。
- `maintenance_notes`：修改前需要注意的维护事项。
- `windows_relevance`：是否与 Windows 迁移相关。

**关系**：

- 一个接手文档包含多个模块概览。
- 模块概览可被调用流程、依赖项或 Windows 风险项引用。

## 实体：调用流程

**说明**：需要被文档化的运行时流程或维护流程。

**字段**：

- `name`：流程名称，例如聊天、附件 ingestion、session 持久化、工作流、更新安装或 fork 同步。
- `steps`：按顺序列出的文件、命令、事件或外部服务。
- `entry_points`：触发流程的 UI、命令、脚本或事件。
- `outputs`：流程产生的事件、文件或状态变化。
- `risk_notes`：Windows 或维护相关注意事项。

**验证规则**：

- 至少覆盖聊天、附件 ingestion、session 持久化、工作流、更新安装和 fork 同步。
- 每个流程必须给出足够路径信息，让新工程师能定位代码。

## 实体：依赖项

**说明**：运行时、构建、外部工具、模型后端、submodule 或文档依据。

**字段**：

- `name`：依赖名称。
- `category`：构建、运行时、打包、模型后端、系统工具、submodule 或文档来源。
- `current_assumption`：当前 Linux、本地算力或仓库假设。
- `windows_consideration`：Windows 下需要修改或验证的内容。
- `affected_modules`：与该依赖相关的仓库位置。

**验证规则**：

- 必须包含 DeepSeek-TUI submodule、Rust/Tauri、Node、本地 vLLM/Qwen、文档解析工具和安装/更新工具。

## 实体：Windows 风险项

**说明**：可能阻塞或影响 Windows 使用的具体迁移风险。

**字段**：

- `risk`：风险摘要。
- `affected_modules`：受影响文件或子系统。
- `priority`：P0、P1 或 P2。
- `recommended_direction`：建议处理方向或决策边界。

**验证规则**：

- 至少列出 10 项风险。
- 必须包含依赖探测、打包、用户目录、更新安装、脚本和外部文档工具相关风险。

## 实体：维护边界

**说明**：防止架构漂移的规则。

**字段**：

- `rule`：边界规则。
- `reason`：规则存在的原因。
- `preferred_extension_point`：优先使用的扩展点。
- `source`：规则来源，例如 `AGENTS.md`、项目宪章或 fork 文档。

**验证规则**：

- 必须包含 DeepSeek-TUI 底座优先原则。
- 必须覆盖 fork PR 策略和本地算力基线。

## 实体：验证步骤

**说明**：用于验证文档或后续迁移改动的命令、检查项或人工复核。

**字段**：

- `name`：验证名称。
- `kind`：静态审阅、命令、运行时 smoke 或人工清单。
- `command_or_action`：可执行命令或具体操作。
- `expected_result`：通过标准。
- `when_to_run`：基线、Windows 迁移后、fork 同步后或文档更新后。

**验证规则**：

- 文档验证必须包含占位符扫描和章节覆盖检查。
- fork 验证必须包含 fork guard、system prompt 和工具集合检查。
