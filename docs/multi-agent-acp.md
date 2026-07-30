# 多 Agent ACP 接入

Pinvou 的“代码”模式复用同一套 ACP client、timeline、权限、附件、工作区和会话恢复
链路接入外部代码 Agent。每个会话在创建时绑定一个 Agent，开始后不能切换 Agent 或
工作目录；需要切换时新建会话。

## 当前 Agent

| Agent | 启动方式 | 运行时来源 | 登录 |
|---|---|---|---|
| Codex | `codex-acp` Bridge | Pinvou 内置 Bridge；Codex CLI 可使用系统安装或 Pinvou 托管版本（托管下载仅 Linux/Windows，macOS 引导通过 Homebrew 安装） | Pinvou 内完成 Codex OAuth；也支持 `OPENAI_API_KEY` |
| Claude Code | `claude-agent-acp` Bridge | Pinvou 内置 Bridge，版本固定为 `0.62.0` | 在 Pinvou 点击“授权登录”；也支持 `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`CLAUDE_CODE_OAUTH_TOKEN` |
| Kimi | `kimi acp` | 自动检测系统 `PATH` 中的官方 Kimi Code CLI | 在 Pinvou 点击“授权登录”，按提示完成设备码授权；也支持 `KIMI_API_KEY` |

开发时可以用以下环境变量覆盖可执行文件：

- `PINVOU3_CODEX_ACP_BIN`
- `PINVOU3_CLAUDE_ACP_BIN`
- `PINVOU3_CLAUDE_CLI_PATH`
- `PINVOU3_KIMI_ACP_BIN`
- `PINVOU3_ACP_NODE_PATH`

## 架构边界

- Pinvou 负责进程托管、ACP transport、session/workspace 绑定、事件持久化、权限与 UI。
- Agent 负责 system prompt、模型、工具循环、skills、MCP、登录态和自身配置。
- 前端只消费 Agent 在 `initialize`、`session/new`、`session/load` 中实际上报的
  capability、model、mode、config option 和 command，不按 Agent 名称猜能力。
- 三种 Agent 共用 `acp-timeline.jsonl` 和工作区安全边界，不复制三套 reducer 或文件
  操作实现。
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
Claude ACP 完成 `initialize`。macOS Runtime 同时包含 arm64/x64 Node 与 Claude
原生程序，供 universal app 在两种架构上选择。

Pinvou 对三种 Agent 复用同一套登录状态机：启动官方 CLI 登录子进程、只接收授权
URL/设备码等非敏感状态，并在登录完成后重新调用各 CLI 的状态检查。Claude 的
`claude auth status` 是权威状态，不能以凭证文件存在代替；Kimi 的设备码仍由官方
`kimi login` 生成并轮询，Pinvou 不代管或复制 OAuth token。
