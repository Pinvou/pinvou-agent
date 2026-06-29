# Vendored skill 来源（改造版）

- 上游：https://github.com/obra/superpowers （子目录 `skills/brainstorming`，作者 Jesse Vincent）
- License：MIT（见 LICENSE）
- 抓取/改造日期：2026-06-23
- **这是改造版，非原样 vendored**。改动：
  1. **去 skill 串联**：原第 9 步「invoke `writing-plans` skill」改成**直接产出 md 实现计划文档**，并把 writing-plans 的方法论精华（plan header / 文件结构 / bite-sized TDD 任务）内联进来。原 skill 依赖 superpowers 生态多个兄弟 skill（writing-plans 自己还 invoke subagent-driven-development / executing-plans / using-git-worktrees）——本地 Qwen3.6 跑不动 skill 串联，故斩断，让本 skill 自包含。
  2. **砍 visual-companion**：原 `scripts/`（node web server）+ `visual-companion.md` + 检查清单第 2 步「offer visual companion」全删——pinvou3 客户机不一定有 node，且是可选功能。
  3. **路径去 superpowers 化**：`docs/superpowers/specs/` → `docs/specs/`。
  4. **中文化**：整份 prompt 译为中文（Qwen3.6 读中文遵循率通常更高），frontmatter `name` 保留英文 `brainstorming` 作标识符。
  5. **提问走 `request_user_input`**：原 skill 用自由文本提问；改成所有提问/确认/选方案都用 pinvou3 的 `request_user_input` 工具（GUI 渲染成选择气泡，每题 1 个问题 + 2-3 个选项，气泡自动带「其他(自己写)」自由输入兜底，开放问题也不被框死）。结构化提问对 Qwen3.6 比自由文本更可靠。

> 改造后是一个完全自包含的「想法→设计→实现计划」skill，全程产出两份 md 文档（设计 doc + 计划 doc），零兄弟 skill 依赖，所有提问走 pinvou3 选择气泡。
