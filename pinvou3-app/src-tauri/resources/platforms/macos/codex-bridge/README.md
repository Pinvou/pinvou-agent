# macOS Codex ACP Bridge Runtime

正式构建前运行：

```bash
./pinvou3-app/scripts/prepare-codex-bridge-runtime.sh
```

脚本会在本目录生成应用隔离的 arm64/x64 Node，以及包含 `codex-acp`、
`claude-agent-acp` 适配器的 universal Bridge。生成物不包含 Codex 或 Claude 平台
二进制并由 `.gitignore` 排除；运行时通过 `CODEX_PATH` 使用系统 Codex（系统缺失时
引导通过官方安装脚本安装/升级，与 Linux 一致；Homebrew 仅用于升级既有 brew 来源的
安装），Claude Code 同样使用
系统安装（缺失时提示安装，如 `brew install --cask claude-code`），适配器通过
`CLAUDE_CODE_EXECUTABLE` 指向解析到的 `claude`。
