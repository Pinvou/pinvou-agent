# Codex ACP runtime

发布构建前运行 `pinvou3-app/scripts/prepare-codex-acp-runtime.sh`，脚本会在本目录生成
当前 Linux 架构的完整 npm 运行时（包括 Codex 原生程序）。生成物不进 Git；开发环境
缺少内置运行时时，应用会自动安装固定版本的 npm 包到 `~/.pinvou3/runtimes/`。

不要把适配器强行编译成单个 Bun 可执行文件：它会在运行时动态解析
`@openai/codex/bin/codex.js`，单文件无法独立工作。
