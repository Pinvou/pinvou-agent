# 契约：DeepSeek-TUI 源码职责分析文档

## 目标文件

`docs/DeepSeek-TUI源码职责分析.md`

## 读者

刚接手 pinvou3 的 Windows 应用开发工程师，以及后续维护 DeepSeek-TUI fork / pinvou3-app bridge 的工程师。

## 必须包含的章节

1. `阅读导向`
   - 说明本文回答什么问题。
   - 说明哪些内容不在本文范围内。

2. `一句话定位`
   - 用简短中文说明 DeepSeek-TUI 是 pinvou3 的 agent 底座。
   - 明确 pinvou3-app 是 Tauri UI、Rust wrapper 和配置/状态适配层。

3. `源码全景`
   - 覆盖 `DeepSeek-TUI/Cargo.toml` 工作区。
   - 按 crate 或目录说明主要职责。
   - 标注当前直接接入、间接依赖、当前未直接接入。

4. `pinvou3 接入边界`
   - 覆盖 `pinvou3-app/src-tauri/src/bridge/mod.rs`。
   - 覆盖 `engine.rs`、`engine_pool.rs`、`commands.rs`、`bridge/sessions.rs`。
   - 说明 bridge 如何构造配置、如何使用 EngineHandle、如何管理 session。

5. `关键调用链`
   - 至少 6 条。
   - 每条包含：入口、pinvou3 适配层、DeepSeek-TUI 能力、输出或副作用。

6. `不要重复造轮子的能力`
   - 至少覆盖 Engine、ToolRegistry、流式事件、Session、SkillRegistry、Commands、MCP、Hooks、Cycle、Compaction。
   - 每项说明推荐扩展方式。

7. `Windows 与维护注意事项`
   - 至少 8 条。
   - 必须包含子模块版本、`Cargo.lock`、Rust 工具链、release exe 进程占用、用户目录路径、打包产物、日志/冒烟检查。

8. `按问题类型排查`
   - 将白屏/闪退、编译失败、会话异常、工具不可用、技能不可用、工作流异常、模型配置异常映射到源码区域。

9. `验收清单`
   - 明确证据点、调用链、风险项是否达标。

## 证据要求

- 至少 12 个源码证据点。
- 证据点必须是仓库相对路径、crate 名、关键类型或调用点。
- 不得只写“源码中有相关实现”这类不可追溯描述。

## 风格要求

- 中文为主。
- 英文仅保留技术名词、命令、路径、crate、类型名、API 字段。
- 避免把上游 README 翻译成全文；重点是当前项目维护视角。
- 避免大段源码引用。

## 非目标

- 不修改 DeepSeek-TUI 或 pinvou3-app 业务代码。
- 不重构底座。
- 不创建新的运行时 API。
- 不替代 DeepSeek-TUI 官方架构文档。
