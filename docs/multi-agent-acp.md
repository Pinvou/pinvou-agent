# 多 Agent ACP 接入

Pinvou 的“代码”模式复用同一套 ACP client、timeline、权限、附件、工作区和会话恢复
链路接入外部代码 Agent，并额外提供品悟原生代码会话。每个会话在创建时绑定一个
Agent，开始后不能切换 Agent 或工作目录（原生会话同样生效）；需要切换时新建会话。

## 当前 Agent

| Agent | 启动方式 | 运行时来源 | 登录 |
|---|---|---|---|
| 品悟（原生） | 进程内 CodeWhale Engine（非 ACP 子进程） | 与“工作”模式相同的应用模型配置 | 无独立登录，沿用工作模式的模型凭据 |
| Codex | `codex-acp` Bridge | Pinvou 内置 Bridge；Codex CLI 优先使用系统安装（≥ 0.144.6），缺失或过旧时经用户确认自动安装或升级（见下文安装与升级矩阵） | Pinvou 内完成 Codex OAuth；也支持 `OPENAI_API_KEY` |
| Claude Code | `claude-agent-acp` Bridge | Pinvou 内置 Bridge（仅 JS 适配器，版本固定为 `0.62.0`）；Claude Code CLI 使用系统安装（≥ 2.0.0），App 不内置 CLI，缺失时经用户确认运行官方安装脚本，过旧时按安装来源升级（见下文安装与升级矩阵） | 在 Pinvou 点击“授权登录”；也支持 `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`CLAUDE_CODE_OAUTH_TOKEN` |
| Kimi | `kimi acp` | 自动检测系统 `PATH` 中的官方 Kimi Code CLI（≥ 0.9.0），缺失时经用户确认运行官方安装脚本，过旧时按安装来源升级 | 在 Pinvou 点击“授权登录”，按提示完成设备码授权；也支持成对设置 `KIMI_MODEL_NAME` 与 `KIMI_MODEL_API_KEY` |

品悟原生会话不是 ACP 后端：它复用“工作”模式的 Engine，通过 `chat` 命令发消息、
`chat:*` 事件推进展示，在 `session-agents.json` 中以 `code_session: true` 标记后
与 ACP 会话共用同一个代码会话列表、工作区面板和时间线展示。原生会话支持临时
工作区与项目目录绑定（“两个根”：LLM 在项目目录干活，应用账本永远在
`~/.pinvou3/sessions/<id>/`）。原生会话使用编码专用系统提示词（共享层 + 代码层：
代码层原样引用底座 core_execution 执行循环并附代码场景纪律，不含产出物/成品卡
语义，成品卡工具同时对代码会话隐藏）。品悟原生会话可开启 Pinvou 多智能体模式；
子智能体仍在执行根工作，但状态与专家名册落在会话私有根。外部 ACP 会话不继承该开关。

开发时可以用以下环境变量覆盖可执行文件：

- `PINVOU3_CODEX_ACP_BIN`
- `PINVOU3_CLAUDE_ACP_BIN`
- `PINVOU3_CLAUDE_CLI_PATH`
- `PINVOU3_KIMI_ACP_BIN`
- `PINVOU3_ACP_NODE_PATH`

## CLI 探测与安装

三个 Agent 走同一套流程：先探测本机 CLI（`PATH`、常见安装位置及以上环境变量覆盖），
再查询各自官方安装器使用的 latest 来源。CLI 缺失或低于最低要求时进入强制安装/升级
引导；低于官方最新版时显示可暂缓的升级提醒。用户确认后调用 `install_acp_agent`
自动安装或升级，
完成后重新探测。版本过旧时先判定安装来源（Homebrew / npm 全局 / 官方脚本），再决定升级
方式，避免同一 CLI 多来源并存。来源判定以「实际被解析使用的那一份 CLI」的路径为准：
先匹配官方脚本安装目录，再要求 brew 前缀 / npm 全局根与包管理器安装记录双重命中；
路径无法判定时才回退 `brew list` / `npm ls -g` 全局查询。探测结果有缓存（前端按秒
轮询），安装/升级成功后自动失效；用户在 App 外手动安装或升级后，点击界面的
「重新检测」会忽略缓存强制重新探测。

官方最新版来源固定为 Codex `https://releases.openai.com/codex/channels/latest`（同时竞速
`https://github.com/openai/codex/releases/latest` 官方回退源）、Claude Code
`https://downloads.claude.ai/claude-code-releases/latest`、Kimi
`https://code.kimi.com/kimi-code/latest`。查询使用独立的异步网络请求，成功与失败结果均缓存
5 分钟，并限制连接/总超时与响应大小；三个 Agent 可并行查询，不阻塞本地 CLI 子进程探测。
离线、超时、非成功 HTTP 状态或响应格式异常时只记录诊断日志，继续按最低兼容版本放行，
避免厂商 latest 接口异常导致已经可用的 Agent 被锁死。

最低版本与 `--version` 输出格式：

| Agent | 最低版本 | 版本输出示例 | 依据 |
|---|---|---|---|
| Codex | 0.144.6 | `codex-cli 0.146.0` | `codex-acp` 1.1.5 的依赖下界 |
| Claude Code | 2.0.0 | `2.1.163 (Claude Code)` | `claude-agent-sdk` 要求 |
| Kimi | 0.9.0 | `0.31.1`（裸 semver） | `kimi acp` 引入版本；旧 Python 版 kimi-cli 已废弃，版本解析失败一律视为不合规 |

版本比较沿用现有 `parse_version` 数字段比较逻辑。无 CLI 的全新安装矩阵（均为免管理员的用户态安装）：

| Agent | 方式 | 说明 |
|---|---|---|
| Codex | 官方脚本 | macOS/Linux：`curl -fsSL https://chatgpt.com/codex/install.sh \| sh`，默认写入 `~/.local/bin`；Windows：`irm https://chatgpt.com/codex/install.ps1 \| iex`，默认写入 `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`；Pinvou 使用平台绝对路径重新探测 |
| Claude Code | 官方脚本 | macOS/Linux：`curl -fsSL https://claude.ai/install.sh \| bash`；Windows：`irm https://claude.ai/install.ps1 \| iex`；装到 `~/.local/bin` 等用户目录 |
| Kimi | 官方脚本 | macOS/Linux：`curl -fsSL https://code.kimi.com/kimi-code/install.sh \| bash`；Windows：`irm https://code.kimi.com/kimi-code/install.ps1 \| iex`；装到 `~/.kimi-code/bin` |

已安装但版本过旧时先判定安装来源，再按来源升级：

| 来源 | 判定 | 升级方式 |
|---|---|---|
| Homebrew（macOS） | CLI 路径位于 brew 前缀下且 `brew list` 命中；kimi-code 为 formula，codex / claude-code 为 cask | `brew upgrade` 对应 formula/cask |
| npm 全局（三端） | CLI 路径位于 `npm prefix -g` 下且 `npm ls -g` 命中 `@openai/codex`、`@anthropic-ai/claude-code`、`@moonshot-ai/kimi-code`；Windows 同时识别 npm 生成的 `.cmd` shim | `npm install -g <包名>@latest` |
| 官方脚本 | 可执行文件位于脚本安装目录（`~/.local/bin`、`~/.kimi-code/bin`、Windows Codex 的 `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`），优先于 brew/npm 判定 | 重新运行官方安装脚本 |

Homebrew / npm 全局来源的旧版一律走对应包管理器升级，避免同一 CLI 多来源并存；脚本来源
或无法识别来源时重新运行官方脚本。可暂缓的最新版提醒只在代码首页选择 Agent 时显示，
不在已有 Session 内重复出现。用户暂不升级时只放行本次进入，CLI 仍可正常使用；该决定
不持久化，离开代码界面后再次从主界面选择对应 Agent 时重新提示。低于最低兼容版本，
或 Agent 明确报告必须升级时，已有 Session 内仍显示阻断提示且不提供暂缓入口。
不静默执行外部命令，也不创建 Pinvou 托管副本。

状态契约：`get_acp_agent_status` / `list_acp_agents` 返回的每个 Agent 状态对象包含
`installed: bool`（CLI 存在且满足最低兼容版本）、
`version: String`（可空，实际探测版本）、`latest_version: String`（可空；检测到官方新版本时
给出升级目标，否则为 `null`）、
`min_version: String`（`"0.144.6"` / `"2.0.0"` / `"0.9.0"`）、
`install_source: String`（`"brew"` / `"npm"` / `"script"` / `null`，
当前探测到 CLI 的安装来源，未安装时为 `null`）、
`install_action: String`（`"none"` / `"brew_upgrade"` / `"npm_upgrade"` /
`"official_script"` / `"manual"`，无需安装或升级时为 `"none"`）、
`update_available: bool`（当前版本低于官方最新版时为 `true`，可暂缓）、
`update_required: bool`（Agent 明确报告必须升级时为 `true`，不可暂缓）；
`authenticated`、`setup_hint` 等既有字段语义不变。Codex/Claude 的
`bridge_ready` 仍表示内置 Bridge 与 Node 是否就绪。
Kimi 不经过独立 Bridge，因此 `bridge_ready` 恒为 `true`；CLI 缺失或版本过低
由 `installed: false` 与 `install_action` 表达，以便前端进入安装或升级流程。
`get_acp_agent_status(agent_id, recheck?)` 传 `recheck: true` 时忽略探测缓存强制
重新探测（「重新检测」按钮）；用户切换 Agent 时也强制重探测，默认状态轮询仍读取缓存。

新增 Tauri 命令 `install_acp_agent(agent, action?)`：按 `install_action` 分派执行安装
或升级（官方脚本、Homebrew `brew upgrade`、npm 全局升级），完成后重新探测并返回最新
状态。Codex 被服务端标记 `update_required` 后，只有重新探测到的实际版本发生变化才
解除升级门禁；包管理器返回“已是最新版”但版本未变时仍保持不可用。安装、升级及运行时
错误也按 Agent 独立保存，不会跨 Agent 展示或互相清除。官方脚本和包管理器升级没有统一
进度协议，前端显示进行中 spinner。旧命令
`prepare_codex_acp`、`install_codex_homebrew` 保留不删除（向后兼容），前端改用新命令。

## 架构边界

- Pinvou 负责进程托管、ACP transport、session/workspace 绑定、事件持久化、权限与 UI。
- Agent 负责 system prompt、模型、工具循环、skills、MCP、登录态和自身配置。
- 前端只消费 Agent 在 `initialize`、`session/new`、`session/load` 中实际上报的
  capability、model、mode、config option 和 command，不按 Agent 名称猜能力。
- 三种 ACP Agent 共用 `acp-timeline.jsonl` 和工作区安全边界，不复制三套 reducer 或文件
  操作实现；品悟原生会话不经过 ACP transport，历史与回合状态来自 SavedSession 与
  `timing_events.jsonl`，展示层与 ACP 会话复用同一个 ConversationTimeline。
- Kimi ACP 会把普通 provider failure 映射为 `end_turn`，仅在会话级
  `logs/kimi-code.log` 写入结构化失败原因。Pinvou 只读取当前回合新增日志中的明确
  `turn ended with failed reason` 记录，将其还原为失败事件；不会把普通空回复当成
  错误，也不会影响 Codex、Claude Code 的协议路径。

这一结构参考 AionUI 的多 Agent 方式：宿主统一 ACP 界面，每个会话绑定独立外部
Agent，各 Agent 保留自己的认证、模型、工具与行为。

## 开发与验证

生成包含 Codex 和 Claude 适配器的隔离 Bridge：

```bash
./pinvou3-app/scripts/prepare-codex-bridge-runtime.sh
```

运行必要检查：

```bash
cd pinvou3-app
npm run test:codex-acp
npm run test:ui-language
npm run build:ui
cd ..
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
python3 scripts/architecture-guard.py
./scripts/fork-guard.sh --fast
```

Windows 与 macOS 还会在各自原生 CI runner 上生成实际安装包 Runtime，并启动
Claude ACP 完成 `initialize`。macOS Runtime 包含 arm64/x64 两套 Node，供 universal
app 在两种架构上选择；Bridge 只携带 JS 适配器，Claude Code 与 Codex 的平台原生
二进制均不随包发布（单个 claude 二进制约 245MB，随包会让 universal dmg 多出约
140MB）。运行时优先解析系统安装；Codex、Claude Code 或 Kimi 缺失时，由各自官方
脚本安装到用户目录；版本过旧则按安装来源经用户确认后用对应包管理器或官方脚本升级，
用户拒绝升级时保持当前状态并挂起升级提示。
最终都通过 `CODEX_PATH` / `CLAUDE_CODE_EXECUTABLE` 注入。

Pinvou 对三种 Agent 复用同一套登录流程，但安装任务、安装状态、登录任务和登录状态
均按 Agent 隔离：启动官方 CLI 登录子进程、只接收授权 URL/设备码等非敏感状态，
并在登录完成后重新调用各 CLI 的状态检查。Claude 的
`claude auth status` 是权威状态，不能以凭证文件存在代替；Kimi 的设备码仍由官方
`kimi login` 生成并轮询，Pinvou 不代管或复制 OAuth token。
