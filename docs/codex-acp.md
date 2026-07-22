# Codex ACP 接入

pinvou3 现在支持按会话选择两套 Agent：原有 DeepSeek-TUI，以及官方
`@agentclientprotocol/codex-acp`。旧会话和默认新会话仍走 DeepSeek-TUI，不改变原行为。

## 使用

1. 可复用 `codex login` 的登录态；若尚未登录，可在 Agent 菜单点“登录 ChatGPT”。
2. 启动 `./pinvou3-app/run-dev.sh`。
3. 新建对话，在输入框下方点击 Agent 图标，选择“Codex ACP”。
4. 发送首条消息。首次使用如果没有内置运行时，应用会把固定版本 `1.1.5` 安装到
   `~/.pinvou3/runtimes/codex-acp-1.1.5/`。
5. 会话创建后可以选择 Codex 模型与推理强度；Plan / YOLO、停止生成、流式文本、
   工具调用卡和重启后的 ACP 会话恢复都沿用现有聊天 UI。

开发时可用 `PINVOU3_CODEX_ACP_BIN=/absolute/path/to/codex-acp` 覆盖运行时。

## 发布

Linux 发布构建前运行：

```bash
./pinvou3-app/scripts/prepare-codex-acp-runtime.sh
```

脚本会把当前 Linux 架构的完整 npm 运行时（ACP 适配器、Codex CLI 与原生依赖）放到
Tauri resource 目录。生成物由 `.gitignore` 排除，不进入源码仓库。这里保留完整依赖
树，是因为适配器运行时会动态解析 `@openai/codex`，不能安全压成单文件。目标机器仍需
提供 Node.js；若发布包没有内置运行时，应用会使用 npm 在线安装回退。

## 边界

- ACP Agent 自己负责 Codex 会话、工具循环、模型和协议能力，pinvou3 不复制这些实现。
- `session-agents.json` 只保存 pinvou 会话到 ACP 会话 ID/模型的映射。
- DeepSeek-TUI 专属技能市场、知识库挂载和工具开关只在原后端显示；Codex 使用自己的
  内置工具与 Codex 配置中的 MCP。
