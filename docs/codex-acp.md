# Codex ACP 接入

> “代码”模式现已在同一 ACP 链路上支持 Codex、Claude Code 和 Kimi；多 Agent
> 结构、运行时来源和登录边界见 [`multi-agent-acp.md`](./multi-agent-acp.md)。

> 完整架构决策和评审项见 [`Codex-ACP-整体架构决策.md`](./Codex-ACP-整体架构决策.md)；
> 无系统 Node/Codex 时的安装、登录和修复方案见
> [`Codex-ACP-运行环境与首次使用设计.md`](./Codex-ACP-运行环境与首次使用设计.md)。
> 本文说明当前 MVP 的使用、验证和发布方式。

pinvou3 在主页输入区提供“工作 / 代码”两种模式：“工作”保持原有品悟输入框，
“代码”当前使用 Codex。两类会话按最近更新时间混排在左侧统一会话列表中，Codex
会话以代码图标区分，不再占用单独的侧边栏入口。Codex 会话仍使用独立的 ACP
事件、权限和持久化链路，不进入 CodeWhale `ChatView`；原有品悟对话继续固定使用
CodeWhale。

## 开发环境使用

1. 开发源码首次运行前执行 `./pinvou3-app/scripts/prepare-codex-bridge-runtime.sh`；
   正式安装包会自带该 Bridge，不要求系统安装 Node/npm。
2. 启动 `./pinvou3-app/run-dev.sh`。Pinvou 会优先检测系统 Codex；没有检测到时可在
   “代码”模式下载固定版本的托管 Codex。
3. 在主页选择“代码”，输入框下方默认选择“临时会话”；直接发送首条消息时才创建
   Codex 会话，避免只切换模式就产生空记录。也可以在发送前切换工作目录：
   - **选择项目目录**：Codex 的进程 cwd、`session/new` 和 `session/load`
     都使用该真实项目目录。
   - **临时会话**：Codex 使用
     `~/.pinvou3/sessions/<id>/workspace/` 隔离目录。
   - **最近项目**：复用近期选择过的项目目录。
   同一个项目可以创建多个独立会话；会话开始后不能更换目录，需要切换项目时新建会话。
4. 页面会读取 Agent 实际上报的模型、模式和配置项。系统 Codex 缺失时，点击下载会把
   固定版本托管 Codex 放到 `~/.pinvou3/runtimes/codex/`；ACP Bridge 版本固定为
   `1.1.5`。当前 Linux MVP 下载 OpenAI 发布的固定平台归档，官方 registry 不可达时
   允许使用镜像，但 Pinvou 代码内置版本、URL 和 SHA-512；不会执行系统 npm，也不会
   把依赖写进系统环境。
5. 输入消息即可使用流式回答、思考、工具步骤、计划、权限选择、停止生成和会话恢复。

## 会话与权限状态

“代码”模式不会直接把 ACP chunk 渲染成消息卡片。前端保留原始
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

ACP 的配置作用域是单个 session，不提供跨 session 默认值。Pinvou 会把用户成功选择的
模型、权限模式、推理强度等配置按 Agent 写入 `acp-agent-defaults.json`；新建该 Agent
会话后，根据 Agent 实际上报的可选项重新应用。历史会话仍使用自己的 session 配置，
不会被后来修改的新会话默认值覆盖。旧版本升级时会从该 Agent 最近的有效会话迁移一次。

应用被直接关闭时，旧进程中的 ACP Prompt 无法在新进程重新挂接。Pinvou 启动恢复会把
`acp-timeline.jsonl` 中只有 `turn_started`、没有 `turn_completed` 的遗留 Turn 收口为
`Interrupted`；对应的运行中工具、权限和输入项显示为已取消。恢复后的会话保留原历史和
工作目录，并可继续发送新消息，不会永久停在“处理中”。

Codex 自己上报的 `/skills`、`/mcp` 等命令可直接在输入框使用。开发时也可用
`PINVOU3_CODEX_ACP_BIN=/absolute/path/to/codex-acp` 覆盖运行时。

## 数据位置

- `~/.pinvou3/session-agents.json`：pinvou 会话与 ACP session ID / model /
  用户确认配置的轻量索引。
- `~/.pinvou3/acp-agent-defaults.json`：每个 ACP Agent 的新会话默认配置。
- `~/.pinvou3/sessions/<id>/acp-state.json`：Agent、capability、model、mode、config 和最后状态。
- `~/.pinvou3/sessions/<id>/acp-timeline.jsonl`：按 `seq` 追加的完整 ACP 事件时间线。
- `~/.pinvou3/sessions/<id>/workspace/`：仅临时 Codex 会话使用的执行目录。

项目会话只在 `session-agents.json` 中保存 canonical absolute path，Pinvou 的 timeline、
状态和会话文件仍放在 `~/.pinvou3/sessions/<id>/`，不会写进项目仓库。恢复会话时项目
目录必须仍然存在；目录丢失会明确报错，不会静默切到临时目录。

Codex 继续复用用户自己的 `HOME` 和 `~/.codex`，所以登录态、Codex 全局配置、
原生 skills、MCP 与 Codex 自身会话记忆仍由 Codex 管理。Pinvou 不把自身记忆注入 Codex。

## 发布

Linux 发布脚本会自动准备 Bridge。单独执行 Tauri 构建前也可手动运行：

```bash
./pinvou3-app/scripts/prepare-codex-acp-runtime.sh
```

脚本会把当前 Linux 架构的应用隔离 Node 与精简 `codex-acp` Bridge 放到
`resources/platforms/linux/codex-bridge/`。项目统一构建入口也会自动准备该目录。
生成物由 `.gitignore` 排除，不进入源码仓库；Bridge 不包含大体积 Codex 平台
二进制。正式 Linux x64 / arm64 包不依赖系统 Node/npm，系统 Codex 缺失时由应用下载
固定、带完整性校验的托管版本。

## 边界

- ACP Agent 自己负责 Codex 会话、system prompt、tools、tool loop、skills、MCP 和上下文。
- pinvou 负责进程托管、ACP 事件还原、权限交互、时间线持久化和 UI。
- MVP 不向 Codex 注入 pinvou bundle skill、MCP、知识库或 persona。
- 附件入口位于代码输入框；图片按 Agent capability 发送，小型文本资源可内嵌，
  其他文件以资源链接发送。不支持的图片能力或格式会明确报错。
- CodeWhale 的技能市场、知识库、工具、Plan/YOLO、远程控制和历史链路保持原样。
