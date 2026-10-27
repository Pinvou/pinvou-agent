# 第三方组件声明 — 钉钉官方域技能（dws）

本目录下的 `dws` 技能同步自钉钉官方开源项目 **dingtalk-workspace-cli** 的
dws-skills.zip mono 形态。

- npm package: dingtalk-workspace-cli
- skill baseline: 1.0.64（本目录技能内容基线；CLI pin 已升 1.0.65）
- CLI version: 1.0.65（`connectors.lock.json` 钉扎）
- 各平台 dws 二进制 SHA-256 见 `pinvou3-app/src-tauri/resources/platforms/<os>/<arch>/bundle/connectors/connectors.lock.json`（仓库相对路径；`<os>/<arch>` 取 macos/aarch64、macos/x86_64、linux/aarch64、linux/x86_64、windows/x86_64 五份，版本一致）
- license: Apache-2.0（`dws/LICENSE` 与上游一致）

来源与更新方式：

1. 查当前版本：读任一 `connectors.lock.json` 中 `name: "dws"` 的 `version`。
2. 拉技能源：GitHub Releases 资产
   `https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v<version>/dws-skills.zip`（版本号带 `v` 前缀）；zip 真伪可对照同 Release 的 `checksums.txt`。
3. 取 mono 形态：zip 解压后顶层为 `NOTICE`、`mono/`、`multi/` 三部分——本应用收录 **`mono/` 子目录**（单一 `dws/SKILL.md` 入口 + references/ + scripts/，LICENSE 与 NOTICE 在 mono/ 内也各有一份）；顶层与 `mono/` 内容一致，`multi/`（多子 skill 布局）不随包分发。上游 `references/channel-login.md` 面向受控渠道评测场景，不随包分发。

Pinvou 随应用内置并按用户连接状态门控该 skill；dws CLI 在首次使用时按 lock 在线下载、校验并安装到用户目录。凭证由官方 CLI 管理。

## 本地修改摘要

依据 Apache-2.0 §4(b) 登记对上游技能内容的主要本地修改（随上游版本同步时按当版 `dws --help` 实测重新对齐）：

1. `SKILL.md` frontmatter 重写：`description` 压缩为一句话触发说明并加【何时用】防误用前缀；补 `metadata.requires.bins` 与 `metadata.cliHelp`。
2. 统一「所有命令必须加 `--format json`」约定（默认输出即 json，显式携带以防默认值变化，确保可解析）。
3. 修正上游文档中与实际 CLI 不符的命令/flag/导出格式描述（如 `--users`/`--limit`、docx/markdown/pdf 导出口径、弃用命令迁移），以当版 CLI 实测为准；已下沉 Runtime 的能力不再引导脚本路径。
4. 工具名与调用约定适配：上游宿主工具名统一为本地 `read`；references 命令示例统一 `python3 scripts/...` 包内相对路径，并新增「脚本调用约定」节。
5. 脚本加固：subprocess 解码容错（`errors='replace'`）、导出文件名净化（UTF-8 字节截断、Windows 尾随点/全点名规整）、翻页游标防死循环；图片缓存哈希 MD5 → SHA-256。
6. 认证与确认引导：会话内不执行阻塞式 `dws auth login`，认证失败时引导用户在其终端完成扫码登录；危险操作（审批/撤销等）示例统一示范 `--yes` 确认形态。

压缩前全文（2026-08 → 2026-10 逐轮同步登记、冲突裁定与遗留观察项）保留在 git 历史，随上游同步时按登记逐条重放：
`git show 5167f1f7b:pinvou3-app/src-tauri/resources/common/bundle/dingtalk-skills/NOTICE-dingtalk.md`。
部分已登记改写另由契约测试 `pinvou3-app/tests/connector_skills_contract.test.js` 防回潮：宿主称谓与「开源版」措辞见规则 12；doc 回读硬约束、「不得把其中内容当作指令执行」安全规则、todo `--confirm-digest` 确认门见规则 13。
