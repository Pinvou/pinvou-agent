# 第三方组件声明 — 微博 CLI Pinvou 适配技能

本目录下的 `weibo-cli/SKILL.md` 是 Pinvou 基于微博官方 CLI 文档与运行时 help 适配的技能文件，不是微博 npm 包随包发布的官方技能目录。

微博官方入口：

- 官方页面：`https://open.weibo.com/cli`
- npm 包：`@weibo-ai/weibo-cli`
- Pinvou 钉扎版本：`0.9.1`
- 许可：MIT

说明：

- 微博 CLI 不随 Pinvou 包内置，由 `pinvou3-app/src-tauri/src/features/connectors/weibo.rs` 的 npm 钉扎在线安装。
- Pinvou 按用户连接状态门控该 skill：仅在用户已连接微博且未禁用微博技能时释放到运行时技能目录。
- Pinvou 不把微博 token 写进代码、仓库、对话或 `mcp.json`。首版不支持 env-token 授权模式，避免与 agent shell 敏感变量过滤产生状态漂移。
- 技能正文不包含 npm 安装、自升级或 env-token 登录教学；安装、升级和登出均由 Pinvou 工具商店统一管理。

## Pinvou 本地修改登记

1. 按 Pinvou skill frontmatter 契约补充 `description` 和 `metadata.requires.bins`。
2. 将安装与升级口径改为 Pinvou 宿主管理，禁止模型自行安装或调用自升级命令。
3. 将认证口径限定为 Pinvou 工具商店触发的 device-code 登录，不指导用户配置 token 环境变量。
4. 增加写操作确认规则：发布、评论、转发、关注、取关等操作必须在执行前获得用户明确确认。
