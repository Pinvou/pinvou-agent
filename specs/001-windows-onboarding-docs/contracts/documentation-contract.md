# 契约：Windows 接手文档

本契约定义 `docs/Windows迁移与维护接手手册.md` 对读者暴露的文档接口。

## 读者契约

文档 MUST 明确目标读者是刚接触 pinvou3、需要迁移到 Windows 并负责后续迭代维护的 Windows 应用开发工程师。

## 覆盖范围契约

文档 MUST 覆盖以下内容：

- 项目定位与 DeepSeek-TUI 底座边界。
- 仓库结构和主要目录职责。
- 聊天请求从前端到 Rust bridge、DeepSeek-TUI、vLLM 再回到 UI 的调用流程。
- 前端事件监听模型和 session 事件分流。
- 后端启动流程和 Tauri command 注册。
- 数据目录和持久化布局。
- 附件 ingestion 管线和外部工具依赖。
- workflow、harness、SubAgent 流程。
- DeepSeek-TUI fork 依赖、受保护 fork 主题和同步验证。
- 本地 vLLM/Qwen3.6 256K 模型假设。
- 当前打包、安装和更新机制。
- Windows 迁移风险表，包含优先级和影响模块。
- 常见维护任务路由表。
- 验证建议。
- 已下线、已推翻或容易误读的历史方案。

## Windows 风险契约

Windows 风险表 MUST 至少包含 10 个具体风险。每个风险 MUST 提供：

- 风险描述。
- 受影响模块。
- 优先级。
- 迁移或处理方向。

风险表 MUST 包含以下 P0 项：

- `DeepSeek-TUI` submodule 未初始化。
- Tauri bundle 仅配置 Linux `.deb` 目标。
- `HOME` 环境变量假设。
- `which` 依赖探测。
- `pkexec` / `apt` 依赖安装。
- `.deb` updater 流程。

## 边界契约

文档 MUST 明确 pinvou3 不得重新实现以下 DeepSeek-TUI 底座能力：

- Engine。
- ToolRegistry。
- 流式 SSE。
- Session。
- SkillRegistry。
- Commands 路由。
- MCP client。
- Hooks。
- Cycle。
- Compaction。

文档 MUST 为常见维护任务提供推荐扩展点，包括 `SKILL.md`、slash command、MCP server、`.deepseek/instructions.md`、`pinvou3-app` Rust/UI 和 DeepSeek-TUI fork。

## 准确性契约

文档 MUST 区分当前项目事实和未来建议。

文档 MUST 标注已下线、已推翻或已搁置方案，避免新维护者按 archived 方案继续实现。

文档 SHOULD 为关键代码和文档来源提供仓库相对路径。

## 验证契约

文档被视为完成前 MUST 通过以下检查：

- 搜索未解决模板占位或澄清标记。
- 确认文档覆盖本契约要求的所有章节。
- 确认没有建议违反 DeepSeek-TUI 底座边界。
- 确认 Windows 风险表至少有 10 项。
- 确认 quickstart 能指导读者进入 `/speckit-tasks` 或后续实现拆解。
