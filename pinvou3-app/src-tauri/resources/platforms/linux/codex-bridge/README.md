# Linux Codex ACP Bridge Runtime

正式构建前运行：

```bash
./pinvou3-app/scripts/prepare-codex-bridge-runtime.sh
```

脚本会在本目录生成应用隔离 Node，以及包含 `codex-acp`、`claude-agent-acp` 和
Claude 原生程序（glibc 版本，Anthropic SDK 同时发布的 musl 变体已剔除）的 Bridge。
生成物不包含 Codex 平台二进制并由 `.gitignore` 排除；运行时通过 `CODEX_PATH` 使用
系统 Codex，系统缺失时由 Pinvou 下载固定版本的托管 Codex。
