# Codex ACP 接入

> “代码”模式现已在同一 ACP 链路上支持 Codex、Claude Code 和 Kimi，并额外提供
> 品悟原生代码会话（进程内 CodeWhale Engine，非 ACP 子进程）；多 Agent
> 结构、运行时来源和登录边界见 [`multi-agent-acp.md`](./multi-agent-acp.md)。

> 品悟原生会话的设计决策、开发节点与改动说明见
> [`code-native-agent.md`](./code-native-agent.md)。
> 本文说明当前 MVP 的使用、验证和发布方式。

pinvou3 在主页输入区提供“工作 / 代码”两种模式：“工作”保持原有品悟输入框，
“代码”可选 Codex / Claude Code / Kimi（ACP）或品悟原生。两类会话按最近更新时间
混排在左侧统一会话列表中，以各自 Agent 图标区分，不再占用单独的侧边栏入口。
ACP 会话仍使用独立的 ACP 事件、权限和持久化链路，不进入 CodeWhale `ChatView`；
品悟原生代码会话复用 CodeWhale Engine 与 `chat` 命令、`chat:*` 事件链路。原生
会话的“两个根”、编码专用系统提示词与多智能体开关的完整语义见
[`multi-agent-acp.md`](./multi-agent-acp.md)（多 Agent 单一真相源）。

## 开发环境使用

1. 开发源码首次运行前执行 `./pinvou3-app/scripts/prepare-codex-bridge-runtime.sh`；
   正式安装包会自带该 Bridge，不要求系统安装 Node/npm。
2. 启动 `./pinvou3-app/run-dev.sh`。Pinvou 会优先检测系统 Codex：存在且版本不低于
   0.144.6 时直接使用，不提示升级；缺失或版本过旧时，经用户确认后按
   [`multi-agent-acp.md`](./multi-agent-acp.md) 的安装与升级矩阵自动安装或升级。
3. 在主页选择“代码”，输入框下方默认选择“临时会话”；直接发送首条消息时才创建
   会话，避免只切换模式就产生空记录。也可以在发送前切换工作目录：
   - **选择项目目录**：Codex 的进程 cwd、`session/new` 和 `session/load`
     都使用该真实项目目录。
   - **临时会话**：Codex 使用
     `~/.pinvou3/sessions/<id>/workspace/` 隔离目录。
   - **最近项目**：复用近期选择过的项目目录。
   同一个项目可以创建多个独立会话；会话开始后不能更换目录，需要切换项目时新建会话。
   品悟原生会话同样支持临时会话与项目目录两种工作区：绑项目后 LLM 直接在项目目录
   中执行，而附件、审计等应用账本仍写入会话私有目录（“两个根”）。
4. 页面会读取 Agent 实际上报的模型、模式和配置项。系统 Codex 缺失或版本过旧时，
   经用户确认后安装/升级（安装来源判定与各渠道升级命令见
   [`multi-agent-acp.md`](./multi-agent-acp.md) 安装矩阵）；用户拒绝时保持不可用，
   不静默安装第二份副本。ACP Bridge 版本固定为 `1.1.5`。
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
  用户确认配置的轻量索引；品悟原生代码会话在其中以 `code_session: true` 标记，
  后端凭该标记把会话纳入代码会话列表并解析临时工作区。
- `~/.pinvou3/acp-agent-defaults.json`：每个 ACP Agent 的新会话默认配置。
- `~/.pinvou3/sessions/<id>/acp-state.json`：Agent、capability、model、mode、config 和最后状态。
- `~/.pinvou3/sessions/<id>/acp-timeline.jsonl`：按 `seq` 追加的完整 ACP 事件时间线。
- `~/.pinvou3/sessions/<id>/workspace/`：仅临时 Codex 会话使用的执行目录。

项目会话只在 `session-agents.json` 中保存 canonical absolute path，Pinvou 的 timeline、
状态和会话文件仍放在 `~/.pinvou3/sessions/<id>/`，不会写进项目仓库。恢复会话时项目
目录必须仍然存在；目录丢失会明确报错，不会静默切到临时目录。

Codex 继续复用用户自己的 `HOME` 和 `~/.codex`，所以登录态、Codex 全局配置、
原生 skills、MCP 与 Codex 自身会话记忆仍由 Codex 管理。Pinvou 不把自身记忆注入 Codex。

## Provider 管理（第三方中转）

三个 ACP Agent（Codex / Claude Code / Kimi）支持配置第三方中转 Provider：
预设/自定义 base URL、wire 协议（Anthropic 兼容 / OpenAI 兼容）、模型与 API key，
一键切换或恢复官方登录。入口：设置 →「Provider 管理」；服务失败横幅也提供
「管理 Provider」深链；会话 composer 可选「会话 Provider」固定本会话使用的中转。
**切换是本机级操作**：会影响所有使用该 CLI 的入口（终端、IDE 插件），不仅是
Pinvou 的「代码」模式。设置页「生效中配置」区展示 CLI 配置文件当前实际生效的
base URL / 模型（不含密钥），以便与终端/插件中的行为核对。

### 涉及的文件

| Agent | 文件 | 写入内容 |
|---|---|---|
| Claude Code | `~/.claude/settings.json` | `env.ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_MODEL` |
| Codex | `~/.codex/config.toml` | `model_providers.<id>`（`env_key="OPENAI_API_KEY"`）+ 顶层 `model_provider` / `model` |
| Kimi | `~/.kimi-code/config.toml` | `providers.<id>` + `models.<id>-main` + `default_model` |

- App 自己的记录在 `~/.pinvou3/acp-providers.json`（按 Agent 分键，版本化，原子写）。
- API key 只存系统凭据库（keyring，service `pinvou3-acp-provider-key`），
  JSON/日志/仓库均无明文；**导出功能除外**——导出文件含明文 key，请勿分享。
- Codex 的 config.toml 无明文 key 字段：key 由 Pinvou 在 spawn Codex 子进程时注入
  `OPENAI_API_KEY`（仅当进程 env 未设置时）。认证探测以 config.toml 存在指向有效
  表且 `env_key` 非空的 `model_provider` 判定已认证。
- 环境变量优先于配置文件：`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` 等已设置时，
  切换 Provider 可能不生效，界面会显式警告。

### 回退语义

- 「恢复官方登录」只删除本功能写入的键/表（claude 的三个 env 键；codex/kimi 的
  `pv-*` 块与指向它们的顶层字段），**保留用户其他配置**。
- 每次受管写入前将原文件备份为 `<file>.pinvou3-bak`（仅首次）；TOML/JSON 文件
  不可解析时**拒绝覆盖**并明确报错。
- 切换后该 Agent 的运行中会话会被安全重启，新会话使用新凭据。
- 会话级 Provider 解析优先级：会话选项 > 全局当前 Provider > 官方登录。

### 安全提示

- 第三方中转会把你的请求与密钥发送到填写的地址；只使用可信中转。
- 导入/导出是本地 JSON 交换：导入会做结构/URL 校验，id 冲突自动重建；
  导出前强警告明文 key。

## 发布

Linux 发布脚本会自动准备 Bridge。单独执行 Tauri 构建前也可手动运行：

```bash
./pinvou3-app/scripts/prepare-codex-bridge-runtime.sh
```

脚本会把当前平台架构的应用隔离 Node 与包含 `codex-acp`、`claude-agent-acp`
适配器的精简 Bridge 放到 `resources/platforms/<os>/codex-bridge/`。项目统一构建
入口也会自动准备该目录。
生成物由 `.gitignore` 排除，不进入源码仓库；Bridge 不包含 Codex CLI。正式包不依赖
系统 Node/npm 来运行 ACP Bridge；系统 Codex 缺失时由应用经用户确认运行 OpenAI 官方
安装脚本。旧版升级按来源分流：npm 来源经用户确认用 `npm install -g
@openai/codex@latest`、brew 来源用 `brew upgrade --cask codex`，其余来源按官方脚本
升级（详见 `multi-agent-acp.md` 的 CLI 探测与安装矩阵）。

## 边界

- ACP Agent 自己负责 Codex 会话、system prompt、tools、tool loop、skills、MCP 和上下文。
- pinvou 负责进程托管、ACP 事件还原、权限交互、时间线持久化和 UI。
- MVP 不向 Codex 注入 pinvou bundle skill、MCP、知识库或 persona。
- 附件入口位于代码输入框；图片按 Agent capability 发送，小型文本资源可内嵌，
  其他文件以资源链接发送。不支持的图片能力或格式会明确报错。
- CodeWhale 的技能市场、知识库、工具、Plan/YOLO、远程控制和历史链路保持原样。
