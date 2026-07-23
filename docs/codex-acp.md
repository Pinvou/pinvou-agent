# Codex ACP 接入

> 完整架构决策和评审项见 [`Codex-ACP-整体架构决策.md`](./Codex-ACP-整体架构决策.md)。本文说明当前 MVP 的使用、验证和发布方式。

pinvou3 在同一个主窗口中提供独立的 Codex 页面。Codex 会话使用独立的 ACP
事件、权限和持久化链路，不进入 DeepSeek `ChatView`；原有品悟对话继续固定使用
DeepSeek-TUI。

## 开发环境使用

1. 安装 Node.js 20 或更高版本，并使用 `codex login` 完成登录。
2. 启动 `./pinvou3-app/run-dev.sh`。
3. 展开主侧栏，点击“Codex”，再点“新建 Codex 会话”：
   - **选择项目目录**：Codex 的进程 cwd、`session/new` 和 `session/load`
     都使用该真实项目目录。
   - **临时会话**：Codex 使用
     `~/.pinvou3/sessions/<id>/workspace/` 隔离目录。
   同一个项目可以创建多个独立会话；会话开始后不能更换目录，需要切换项目时新建会话。
4. 页面会读取 Agent 实际上报的模型、模式和配置项。首次使用若没有内置运行时，
   点击安装会把固定版本 `1.1.5` 放到
   `~/.pinvou3/runtimes/codex-acp-1.1.5/`。
5. 输入消息即可使用流式回答、思考、工具步骤、计划、权限选择、停止生成和会话恢复。

## 会话与权限状态

Codex 页面不会直接把 ACP chunk 渲染成消息卡片。前端保留原始
`acp-timeline.jsonl`，再投影为 Codex 的会话模型：

```text
Thread
  └── Turn
       ├── Reasoning Item
       ├── Command Execution Item
       ├── File Change / Tool Item
       ├── Permission Item
       ├── Plan Item
       └── Agent Message Item
```

每个 Item 按 `started → delta → completed/failed` 更新。运行中的 Turn 和
Reasoning 使用事件时间戳显示耗时；完成的工具步骤默认折叠，命令、工作目录、
终端输出和退出码按结构化字段展示，ACP 原始 JSON 不进入普通会话界面。

权限模式以 ACP `config_options.mode` 为主；只有 Agent 未上报该配置时才回退到
`session/set_mode`。Pinvou 不再在每次 Prompt 前重复设置模式。用户确认的模式写入
`session-agents.json`，ACP `session/new`、`session/load` 或进程重连后会先重新应用，
再把会话标记为可发送。配置应用期间和 Turn 运行期间不能发送另一项配置修改。

Codex 自己上报的 `/skills`、`/mcp` 等命令可直接在输入框使用。开发时也可用
`PINVOU3_CODEX_ACP_BIN=/absolute/path/to/codex-acp` 覆盖运行时。

## 数据位置

- `~/.pinvou3/session-agents.json`：pinvou 会话与 ACP session ID / model /
  用户确认权限模式的轻量索引。
- `~/.pinvou3/sessions/<id>/acp-state.json`：Agent、capability、model、mode、config 和最后状态。
- `~/.pinvou3/sessions/<id>/acp-timeline.jsonl`：按 `seq` 追加的完整 ACP 事件时间线。
- `~/.pinvou3/sessions/<id>/workspace/`：仅临时 Codex 会话使用的执行目录。

项目会话只在 `session-agents.json` 中保存 canonical absolute path，Pinvou 的 timeline、
状态和会话文件仍放在 `~/.pinvou3/sessions/<id>/`，不会写进项目仓库。恢复会话时项目
目录必须仍然存在；目录丢失会明确报错，不会静默切到临时目录。

Codex 继续复用用户自己的 `HOME` 和 `~/.codex`，所以登录态、Codex 全局配置、
原生 skills、MCP 与 Codex 自身会话记忆仍由 Codex 管理。Pinvou 不把自身记忆注入 Codex。

## 发布

Linux 发布构建前运行：

```bash
./pinvou3-app/scripts/prepare-codex-acp-runtime.sh
```

脚本会把当前 Linux 架构的完整 npm 运行时（ACP 适配器、Codex CLI 与原生依赖）放到
Tauri resource 目录。生成物由 `.gitignore` 排除，不进入源码仓库。这里保留完整依赖
树，是因为适配器运行时会动态解析 `@openai/codex`，不能安全压成单文件。

当前 MVP 的目标机器仍需提供 Node.js 20+；正式 Linux x64 / arm64 发布包内置私有
Node runtime 的工作属于下一阶段。

## 边界

- ACP Agent 自己负责 Codex 会话、system prompt、tools、tool loop、skills、MCP 和上下文。
- pinvou 负责进程托管、ACP 事件还原、权限交互、时间线持久化和 UI。
- MVP 不向 Codex 注入 pinvou bundle skill、MCP、知识库或 persona。
- 附件虽由当前 Agent capability 上报为支持图片，但 pinvou 发送链路尚未实现，所以
  MVP 不显示伪附件入口。
- DeepSeek-TUI 的技能市场、知识库、工具、Plan/YOLO、远程控制和历史链路保持原样。
