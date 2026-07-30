# macOS Codex ACP Bridge Runtime

正式构建前运行：

```bash
./pinvou3-app/scripts/prepare-codex-bridge-runtime.sh
```

脚本会在本目录生成应用隔离的 arm64/x64 Node，以及包含 `codex-acp`、
`claude-agent-acp` 和两种架构 Claude 原生程序的 universal Bridge。生成物不包含
Codex 平台二进制并由 `.gitignore` 排除；运行时通过 `CODEX_PATH` 使用系统 Codex
（macOS 不提供托管下载，系统缺失时引导通过 Homebrew 安装 `codex` cask）。
