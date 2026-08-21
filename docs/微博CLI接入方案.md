# 微博 CLI 接入方案

## 背景

目标是在 `agent/weibo-connector` 分支接入微博官方 CLI，让 Pinvou Agent 可以像现有飞书、企微、钉钉、腾讯会议一样，通过工具商店完成安装、授权、启用技能、断开授权的闭环。

微博官方能力入口：

- 官方页面：`https://open.weibo.com/cli`
- npm 包：`@weibo-ai/weibo-cli`
- 当前核对版本：`0.9.1`
- bin：`weibo-cli`、`weibo`、`wb`
- license：MIT
- Node 要求：`>=18`

微博 CLI 支持：

- `weibo-cli auth login`
- `weibo-cli auth login --device`
- `weibo-cli auth whoami`
- `weibo-cli auth logout`
- `weibo-cli doctor`
- `weibo-cli commands list`
- `weibo-cli commands show <group> <action>`
- `weibo-cli <group> <action> --output json`

包内 README 说明 token 优先从环境变量读取，其次从本机系统 keychain 或 `~/.weibo-cli/` 加密文件读取。Pinvou 不保存微博 token，不把 token 写进仓库、对话或 `mcp.json`。

Pinvou 首版只支持 `weibo-cli auth login --device` 后由微博 CLI 自己写入的 keychain / `~/.weibo-cli/` 授权态，不支持依赖 `WEIBO_CLI_TOKEN`、`WEIBO_TOKEN`、`WEIBO_CLI_REFRESH_TOKEN`、`WEIBO_REFRESH_TOKEN` 的 env-token 授权模式。原因是 Pinvou agent shell 当前会过滤 `TOKEN` / `*_TOKEN` / `*_KEY` 形态的敏感环境变量；如果前端状态探测依赖 env token，而会话执行时变量被过滤，会出现“工具商店显示已连接但 agent 调用失败”的不一致。

## 现有 CLI 接入方式

仓库现有连接器有两种实现路径。

### 原生二进制锁定下载

代表：飞书、企微、钉钉。

相关文件：

- `pinvou3-app/src-tauri/src/features/connectors/feishu.rs`
- `pinvou3-app/src-tauri/src/features/connectors/wecom.rs`
- `pinvou3-app/src-tauri/src/features/connectors/dingtalk.rs`
- `pinvou3-app/src-tauri/src/features/connectors/native_installer.rs`
- `pinvou3-app/src-tauri/resources/platforms/*/*/bundle/connectors/connectors.lock.json`

特点：

- 每个平台 lock 文件钉住下载 URL、归档 hash、可执行文件 hash。
- 首次连接时 `ensure_native_cli("<name>")` 下载、校验、解包到 `~/.pinvou3/connectors/<platform>/bin/`。
- 适合厂家发布跨平台二进制归档的 CLI。

微博当前是 npm 包，不走这条路径。

### npm 在线安装

代表：腾讯会议。

相关文件：

- `pinvou3-app/src-tauri/src/features/connectors/tmeet.rs`
- `pinvou3-app/src-tauri/src/features/connectors/connector_cli.rs`
- `pinvou3-app/src-tauri/src/platform/os/linux/linux_path.rs`
- `pinvou3-app/src-tauri/src/platform/os/macos/macos_path.rs`
- `pinvou3-app/src/features/tools/ToolStoreView.jsx`
- `pinvou3-app/src/features/tools/tool-common.jsx`

特点：

- `tmeet.rs` 钉住 npm spec：`@tencentcloud/tmeet@1.0.15`。
- 用应用内 Node/npm 或用户 npm prefix 执行 `npm install -g <spec>`。
- `connector_cli::run_with_timeout` 统一处理无 stdin、安装超时和 `~/.pinvou3/cli-install.log`。
- `CliCtx` 统一处理 CLI 命令构造、授权 URL 抓取和二维码生成。
- 授权流程通过 Tauri 事件驱动前端：`<id>:qr`、`<id>:connected`、`<id>:error`。

微博应复用这条路径。

## 推荐方案

微博接入按“腾讯会议 npm CLI + 钉钉 device-code 授权”混合模式实现。

### 1. 新增后端连接器模块

新增：

- `pinvou3-app/src-tauri/src/features/connectors/weibo.rs`

并在：

- `pinvou3-app/src-tauri/src/features/connectors/mod.rs`
- `pinvou3-app/src-tauri/src/app/commands/connectors.rs`
- `pinvou3-app/src-tauri/src/lib.rs`
- `pinvou3-app/src-tauri/src/app/commands/protocol_tests.rs`

注册以下命令：

- `weibo_ensure_cli`
- `weibo_status`
- `weibo_connect_begin`
- `weibo_cancel`
- `weibo_logout`
- `weibo_apply_skills`
- `set_weibo_enabled`
- `weibo_skills_state`

建议常量：

```rust
const ID: &str = "weibo";
const WEIBO_NPM_SPEC: &str = "@weibo-ai/weibo-cli@0.9.1";
const WEIBO_MIN_VERSION: (u64, u64, u64) = (0, 9, 1);

const WEIBO_CTX: CliCtx = CliCtx {
    cli_bin: "weibo-cli",
    envs: &[],
    auth_domains: &["open.weibo.com", "open-dev.weibo.com"],
};
```

实现复用 `tmeet.rs` 的 npm 安装方式：

- 先跑 `weibo-cli --version` 判断是否已安装且版本满足最低要求。
- 未安装或版本过低时执行 `npm install -g @weibo-ai/weibo-cli@0.9.1`。
- 安装命令必须通过 `connector_cli::run_with_timeout`，避免 GUI 无 stdin 时卡死。
- 状态探测和授权后验证也必须使用有界超时；超时按未连接处理，不阻塞工具商店或首屏后的门控刷新。

### 2. 授权流程

微博 CLI 支持 browser 和 device-code。Pinvou 建议默认使用：

```bash
weibo-cli auth login --device --name Pinvou
```

原因：

- Tauri GUI 场景下 browser flow 可能依赖 CLI 自己打开浏览器和本地回调端口，不利于统一进度展示。
- device-code 会输出 URL 和 user code，形态更接近钉钉，能复用二维码流程卡。
- 用户仍在浏览器中确认授权，Pinvou 不接触 token。

后端流程：

1. `weibo_connect_begin` 重置 `ConnectorConn`。
2. spawn `weibo-cli auth login --device --name Pinvou`，stdout/stderr 均 pipe。
3. 复用或抽出钉钉的 `AuthEvent` 解析逻辑：
   - 从输出中提取 `https://open.weibo.com/...` 或 `https://open-dev.weibo.com/...`。
   - 从输出中提取 `user_code` / `user code` / `code:` / `验证码`。
   - 如果 URL 没带 `user_code`，用 `?user_code=<code>` 或 `&user_code=<code>` 拼上。
4. emit `weibo:qr`，payload 包含：
   - `phase: "authorize"`
   - `url`
   - `user_code`
   - `qr_data_url`
5. 等授权进程退出后调用 `weibo-cli auth whoami --output json` 验证是否已登录，验证命令必须有短超时。
6. 成功 emit `weibo:connected`，失败 emit `weibo:error`。

超时建议：

- 获取 device-code URL：参考 `tmeet`，60 秒内没有解析到授权 URL 则 kill 授权进程并返回脱敏错误。
- 等用户完成授权：保持可取消，按 CLI 自身 device-code 生命周期等待；用户点取消时 tree-kill 子进程。
- `auth whoami` / `doctor` / `--version` 等状态探测：使用短超时，避免 `refresh_connector_auth_gates` 被网络或微博开放平台异常拖住。

状态判断建议：

- 未安装：`{ ok:false, connected:false, installed:false }`
- 已安装未登录：`{ ok:false, connected:false, installed:true }`
- 已登录：`{ ok:true, connected:true, installed:true }`

`auth whoami --output json` 未登录时当前返回中文错误“缺少登录令牌...”，不应把 stderr 原样带进 webview；只返回布尔和必要错误提示。

### 3. 技能门控

新增微博独立技能树：

- `pinvou3-app/src-tauri/resources/common/bundle/weibo-skills/weibo-cli/SKILL.md`
- `pinvou3-app/src-tauri/resources/common/bundle/weibo-skills/NOTICE-weibo.md`

微博 npm 包当前没有随包发布 `SKILL.md` 目录，不能假称“官方技能同步”。建议用 Pinvou 适配技能，技能正文基于微博官方 CLI README 和 `commands list/show` 的运行时目录。

`SKILL.md` frontmatter 必须符合现有 contract：

```yaml
---
name: weibo-cli
description: 何时用：当用户需要操作微博内容、评论、关注关系、搜索、用户信息、热搜/趋势或微博开放平台命令时使用。使用前先确认微博 CLI 已登录，所有写操作执行前必须向用户确认。
requires:
  bins: ["weibo-cli"]
---
```

技能正文原则：

- 先用 `weibo-cli auth whoami --output json` 或 `weibo-cli me --output json` 确认登录。
- 查询命令默认加 `--output json`。
- 未知命令先查 `weibo-cli commands list --available --output json` 和 `weibo-cli commands show <group> <action>`。
- 发布、评论、转发、关注、取关等写操作必须先复述影响对象和内容，获得用户明确确认后再执行。
- 不调用 `weibo-cli upgrade`，升级由 Pinvou 宿主管理。
- 不输出、不请求、不持久化 `WEIBO_CLI_TOKEN` / `WEIBO_TOKEN` / `WEIBO_CLI_REFRESH_TOKEN` / `WEIBO_REFRESH_TOKEN`。
- 不指导用户通过 env token 登录；如果检测到未登录，提示用户从 Pinvou 工具商店重新连接微博。

runtime bundle 增加：

- `WEIBO_SKILLS_DIR`
- `WEIBO_SKILL_DIRS`
- `apply_weibo_skills(show)`
- `cached_weibo_skills_visible()`

并把 `refresh_connector_auth_gates` 扩展为同时刷新微博门控。

启动 bundle 解包流程也要加入微博 cached gate：

- `extract()` 中读取 `cached_weibo_skills_visible()`。
- `bundle_changed || !weibo_show` 时调用 `apply_weibo_skills(weibo_show)`。
- 启动日志增加 `bundle_extract:weibo_cached_gate`。

新增技能树后必须 bump `BUNDLE_VERSION` 的语义版本段，并更新版本注释。现有实现说明连接器技能树不参与内容 hash；如果不 bump，已连接用户启动时不能稳定刷新到新增微博技能。

新增本地停用标志：

- `~/.pinvou3/weibo_disabled`

对应：

- `platform/connector_state.rs` 增加 `weibo_skills_visible()`
- `weibo.rs` 实现 `ConnectorSkillGate`

### 4. 前端工具商店

在工具卡里新增微博条目，位置放在“沟通协作”类 CLI 连接器区域：

- `backendId: 'weibo'`
- `weiboCli: true`
- `title: '微博'`
- `type: 'CLI + Pinvou 适配技能'`
- `version: 'v0.9.1'`
- `authRequired: true`
- `configFields: []`

建议文案：

- subtitle：`以你本人身份操作微博发布、互动、检索和趋势`
- desc：`接入微博官方 CLI（@weibo-ai/weibo-cli，MIT）+ Pinvou 适配技能：让 AI 以你本人身份检索微博、查看用户和关系、处理评论互动，并在确认后执行发布类操作。点「连接」打开微博授权页登录，全程不填 key。`
- welcomeQueries：
  - `查一下微博热搜趋势`
  - `搜索某个关键词的微博`
  - `查询这个微博用户的信息`
  - `帮我草拟一条微博，确认后发布`

需要改：

- `pinvou3-app/src/features/tools/tool-common.jsx`
- `pinvou3-app/src/features/tools/ToolStoreView.jsx`
- `pinvou3-app/src/shared/i18n.js`
- `pinvou3-app/src/features/settings/SettingsView.jsx`
- `pinvou3-app/src/platform/web/access-policy.json`

前端连接流程直接复制 `tmeet`/`dingtalk` 的模式：

- `weiboConn` 跨视图 flow store
- `ensureWeiboListeners`
- `connectWeibo`
- `weiboResetFlow`
- `weiboRetry`
- `disconnectWeibo`
- 在 `handleAction` 中新增 `backendId === 'weibo'` 分支
- 连接成功后调用 `weibo_apply_skills`

如果 `weibo:qr` 带 URL，前端必须像 `tmeet` 一样调用 `open_external_url` 自动打开系统默认浏览器，同时保留 user code 和“重新打开”入口。微博 device-code 授权仍要求用户在浏览器中确认授权，Pinvou 不控制浏览器页面、不自动填码、不自动点击确认。

授权卡片体验要求：

- 收到 `weibo:qr` 后立即调用 `open_external_url(url)`。
- 打开失败时把 flow 标记为 `browserOpenFailed`，卡片显示“浏览器未能自动打开，请点击重新打开”。
- 打开成功或未返回错误时显示“已自动打开浏览器登录页，请在页面输入验证码完成授权”。
- 验证码使用等宽大字号展示，并提供“一键复制”按钮；复制成功后短暂显示“已复制”。
- “重新打开”按钮保留，点击后再次调用同一个 URL。
- 后端仍等待 CLI 授权进程退出，随后二次执行 `weibo-cli auth whoami --output json` 确认真实登录态。

不做：

- 不自动填写验证码或控制用户浏览器页面。
- 不跳过微博授权确认。
- 不改用 env token 授权。

### 5. 能力包注册表

在：

- `pinvou3-app/src-tauri/src/features/marketplace/bundle.rs`

把微博加入 `BUILTIN_CLI_BUNDLES`：

```rust
("weibo", "微博"),
```

这会让统一能力包模型把微博归为 CLI 包，并参与“一个包 = 一个开关”的治理。

### 6. 平台路径

微博和腾讯会议一样是 npm shim，需要确认现有平台适配是否只对白名单 `tmeet` 生效。

相关文件：

- `pinvou3-app/src-tauri/src/platform/os/linux/linux_path.rs`
- `pinvou3-app/src-tauri/src/platform/os/macos/macos_path.rs`
- `pinvou3-app/src-tauri/src/platform/os/windows/windows_path.rs`

macOS / Linux 当前对 npm shim 的 bundled Node 包装只对白名单 `tmeet` 生效，需要改为：

- `matches!(cli_bin, "tmeet" | "weibo-cli")`

Windows 当前 npm shim 解析是通用路径，通常不需要同样的白名单修改；只需补 contract 测试确认 `weibo-cli` 会走 npm shim 候选路径。整体目标是防止微博在 macOS/Linux 正式包里找不到 Node 或 npm shim。

### 7. 测试与契约

需要更新：

- `pinvou3-app/tests/connector_online_install_contract.test.js`
  - 断言 `@weibo-ai/weibo-cli@0.9.1`
  - 断言工具卡版本等于后端 npm pin
  - 断言 macOS/Linux npm CLI 路径适配包含 `weibo-cli`
  - 断言 Windows npm shim 解析不需要新增 `tmeet` 白名单
- `pinvou3-app/tests/connector_skills_contract.test.js`
  - `packs` 增加 `weibo-skills`
  - `binsByPack` 增加 `"weibo-skills/weibo-cli": "weibo-cli"`
  - 黑名单继续禁止 skill 中出现 npm 全局安装教学和自更新教学
  - 黑名单继续禁止 skill 教用户配置 `WEIBO*_TOKEN` env token
- `pinvou3-app/tests/tool_store_smoke.js`
  - 增加微博连接流程 smoke
  - mock `weibo_ensure_cli` / `weibo_connect_begin` / `weibo_apply_skills`
  - emit `weibo:qr` / `weibo:connected`
- `pinvou3-app/tests/web_access_contract.test.mjs`
  - web 只允许 `weibo_skills_state`
  - 禁止 web 调用 `weibo_ensure_cli` / `weibo_connect_begin` / `weibo_apply_skills` / `set_weibo_enabled`
- Rust 单测：
  - version parse
  - `auth whoami` 登录态解析
  - device-code URL + user_code 组合
  - 状态探测超时按未连接处理
  - token 行脱敏
  - `weibo_disabled` flag roundtrip
  - bundle `apply_weibo_skills(true/false)` 解包/删除
  - bundle version bump 后 cached gate 会刷新微博技能树

建议验证命令：

```bash
cd pinvou3-app
node tests/connector_online_install_contract.test.js
node tests/connector_skills_contract.test.js
node tests/tool_store_smoke.js
node tests/web_access_contract.test.mjs
cd src-tauri
cargo test -p pinvou3-tauri connectors::weibo
cargo test -p pinvou3-tauri runtime_bundle::platform
```

如果改到架构边界或跨平台路径，还需要在仓库根目录运行：

```bash
python3 scripts/architecture-guard.py
```

## 风险与处理

1. 微博 CLI 设备码输出格式可能调整。
   - 处理：解析逻辑只依赖 URL 域名和 user code 常见标签，保留最近 32 行脱敏日志用于错误提示。

2. CLI 依赖微博开放平台账号状态、开发者认证、套餐或额度。
   - 处理：连接成功只代表登录成功；技能执行具体命令前优先用 `weibo-cli doctor --output json` 或命令错误解释账号/权限问题。

3. env-token 授权和 Pinvou agent shell 敏感变量过滤冲突。
   - 处理：首版不支持 env-token 授权，不把 `WEIBO*_TOKEN` 作为连接状态来源；用户必须通过 Pinvou 工具商店触发 `auth login --device`，由微博 CLI 自己管理本地授权态。

4. 新增技能树不参与 bundle 内容 hash。
   - 处理：新增微博技能时同步 bump `BUNDLE_VERSION`，并覆盖 cached gate 测试，确保已连接用户升级后能刷新技能。

5. npm 包不是二进制 lock 下载。
   - 处理：和 `tmeet` 一样钉住 npm spec，不使用 `latest`，升级必须改常量、工具卡版本和测试。

6. 技能不是微博官方随包发布。
   - 处理：文案明确为“Pinvou 适配技能”，NOTICE 记录来源和本地修改边界，不把它标成官方同步技能。

7. 微博写操作风险高。
   - 处理：Skill 中强制写前确认；前端连接卡也说明这是 opt-in 联网能力。

## 分阶段落地

第一阶段：后端最小闭环。

- 新增 `weibo.rs`。
- 完成 install/status/connect/logout/cancel/apply skills。
- 新增 weibo skill 门控和 bundle 解包。
- 补 Rust 单测。

第二阶段：前端工具商店接入。

- 新增微博工具卡。
- 接入连接流程卡、断开流程、设置页开关。
- 补 i18n 和 web access contract。
- 补工具商店 smoke。

第三阶段：技能质量补齐。

- 新增 `weibo-cli/SKILL.md` 和 NOTICE。
- 基于 `commands list/show` 写最小可用工作流。
- 明确查询默认 JSON、写操作确认、token 禁令、自更新禁令。

第四阶段：联调。

- 未安装状态。
- 首次 npm 安装。
- device-code 获取 URL 和 user code。
- 授权成功后技能目录出现。
- 断开后技能目录删除。
- 新会话可按 `weibo-cli` skill 调用微博命令。
