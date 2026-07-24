# 华宇元典远程 MCP OAuth 修复方案（PR158）

## 背景

PR158 接入华宇元典法律数据远程 MCP，目标是让工具市场在安装 MCP 配置后继续完成 OAuth 授权，并且只有在授权真实可用时才显示“已连接”。

当前 PR 的方向成立，但凭据生命周期实现需要收敛到底座 OAuth API。否则会出现两类问题：

- 连接状态只判断 keyring 中是否存在非空字符串，可能把损坏、过期或不可刷新 token 误判为已连接。
- 断开连接只删除工具市场安装状态和 `mcp.json` server，没有删除底座保存的 OAuth token。

## 修复目标

1. 工具市场连接状态以底座 OAuth 状态机为准。
2. 工具市场断开连接时同步删除底座 OAuth token。
3. 不扩大无实际调用链的外链白名单。
4. 清理 PR 中与元典接入无关的格式化 diff。

## 具体改动

### 1. 复用底座 OAuth 状态判断

删除 `commands.rs` 中自行复制底座 key 算法的实现：

- `marketplace_oauth_store_key`
- `marketplace_oauth_token_present`

将 `get_marketplace_tool_auth_status` 改为异步 Tauri command。读取 `mcp.json` 中对应 server 后，调用：

```rust
deepseek_tui::mcp::oauth::auth_status_for_server(name, server).await
```

状态映射：

- `McpAuthStatus::OAuth` -> `connected`
- `McpAuthStatus::NotLoggedIn` -> `config_installed_auth_pending`
- `McpAuthStatus::Unsupported` -> 按现有缺配置/未安装逻辑处理
- `McpAuthStatus::BearerToken` -> 不作为元典 OAuth connected，除非后续明确支持手动 Authorization header

### 2. 卸载前删除 OAuth token

在 `uninstall_marketplace_tool(tool_id)` 中，调用 `mgr.uninstall(&tool_id)` 之前：

1. 通过 `mgr.oauth_remote_server_name(&tool_id)` 找到 OAuth server name。
2. 从 `mcp.json` 读取该 server config。
3. 调用：

```rust
deepseek_tui::mcp::oauth::delete_oauth_tokens_for_server(&server_name, &server)
```

处理规则：

- 删除 token 报错时返回 `Err`，不要继续提示断开成功。
- `Ok(false)` 表示没有已存 token，可以继续卸载。
- 如果工具已安装但 `mcp.json` 中找不到 server，可继续卸载，但应记录清晰日志。

顺序必须先删 OAuth token，再删 `mcp.json` server，因为删除后就无法从配置中恢复 server URL。

### 3. 回归测试

测试目标是验证三个承诺：

- 不再把“keyring 中存在非空字符串”误判为 `connected`。
- 断开连接会先删除底座 OAuth token，再删除 MCP 配置。
- 元典 OAuth 不扩大 `open_external_url` 外链能力。

#### 3.1 状态映射单元测试

覆盖 `marketplace_auth_status_fields`：

| 输入 | 预期 |
| --- | --- |
| `McpAuthStatus::OAuth` | `connected`，`oauth_token_present=true` |
| `McpAuthStatus::NotLoggedIn` | `config_installed_auth_pending` |
| `McpAuthStatus::Unsupported` | `config_installed_auth_pending` |
| `McpAuthStatus::BearerToken` | `config_installed_auth_pending` |
| 非 OAuth 工具已安装 | `connected`，`oauth_token_present=false` |

本地命令：

```bash
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace_auth_status --lib
```

Windows 本机如出现 `STATUS_ENTRYPOINT_NOT_FOUND`，使用 `--no-run` 验证编译，并交给 Linux/GitHub Actions 执行测试二进制：

```bash
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace_auth_status --lib --no-run
```

已落地自动化用例：

- `marketplace_auth_status_only_oauth_is_connected_for_oauth_tools`
- `marketplace_auth_status_preserves_non_oauth_installed_semantics`
- `marketplace_auth_status_requires_mcp_config_for_oauth_connected`

#### 3.2 OAuth token 状态集成测试

覆盖 `get_marketplace_tool_auth_status`：

1. 临时 `PINVOU3_HOME`。
2. 写入测试用 `mcp.json`，包含唯一 server name 和本地不可访问 URL。
3. 不写 token 时调用状态查询，断言不是 `connected`。
4. 写入损坏 token，例如 `not-json`，调用状态查询，断言不是 `connected`。
5. 写入可解析但不可用 token，例如空 `client_id` 或空 access token，调用状态查询，断言不是 `connected`。

注意事项：

- 测试只允许在 `#[cfg(test)]` 中复制底座 `store_key` 算法，生产代码不得恢复自定义 key 判断。
- server name 必须带随机后缀，测试结束必须删除 keyring 中的测试 token。
- 可用 token 的完整集成测试如果构造成本过高，先由底座 `auth_status_for_server` 自身测试覆盖，本 PR 侧只验证状态映射和坏 token 不 connected。

已落地自动化用例：

- `marketplace_auth_status_does_not_treat_missing_or_corrupt_token_as_connected`

#### 3.3 断开清理集成测试

覆盖 `uninstall_marketplace_tool`：

1. 临时 `PINVOU3_HOME`。
2. 安装测试 manifest，确认 `installed.json` 和 `mcp.json` server 已写入。
3. 写入测试 OAuth token。
4. 调用 `uninstall_marketplace_tool`。
5. 断言：
   - `installed.json` 不再包含工具 id。
   - `mcp.json` 不再包含 server。
   - 底座 OAuth token 已删除。

失败场景：

- 如果 token 删除返回 Err，`uninstall_marketplace_tool` 必须返回 Err，且不应继续删除 MCP 配置。该场景可通过抽象删除 helper 后用 mock 覆盖。

已落地自动化用例：

- `uninstall_marketplace_tool_deletes_oauth_token_before_mcp_config`
- `uninstall_marketplace_tool_aborts_if_oauth_token_delete_fails`

#### 3.4 外链白名单回归

覆盖 `open_external_url` 白名单：

- `https://open.chineselaw.com/oauth/authorize` 不应通过。
- `https://passport.legalmind.cn/ssologin` 不应通过。
- 既有允许域仍允许：`https://metaso.cn/`、`https://obsidian.md/download`。
- 钓鱼域仍拒绝：`https://metaso.cn.evil.com/`、`http://obsidian.md/`。

已落地自动化用例：

- `external_allowlist_allows_known_targets_rejects_lookalikes`

#### 3.5 前端行为测试

覆盖 `ToolStoreView` OAuth 连接流程：

- 安装成功但 auth status 返回 `config_installed_auth_pending` / `oauth_token_present=false` 时，不显示连接成功或新建会话弹窗。
- OAuth login 返回 `connected` 后，再次查询 auth status 仍为 `oauth_token_present=false` 时，不显示成功。
- OAuth login 返回 `connected` 且 auth status 返回 `oauth_token_present=true` 时，才显示成功态。

已落地自动化：

- 新增 `src/features/tools/oauth-marketplace-logic.js`，把 OAuth 安装结果判定抽成纯函数。
- 新增 `tests/marketplace_oauth_logic.test.js`，覆盖授权超时、登录成功但无 token、登录成功且 token 存在三类结果。
- `npm test` 已串入 `test:marketplace-oauth`。

#### 3.6 构建与门禁

本地命令：

```bash
npm run build:ui
npm test
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace_auth_status --lib --no-run
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace --lib --no-run
git diff --check
```

如果环境允许执行 Rust 测试二进制，再跑：

```bash
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace_auth_status --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace --lib
```

#### 3.7 本次执行记录

执行日期：2026-07-14。

已执行并通过：

```bash
npm run build:ui
npm test
npm run test:marketplace-oauth
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace_auth_status --lib --no-run
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace --lib --no-run
git diff --check
```

已尝试执行测试二进制：

```bash
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace_auth_status --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml uninstall_marketplace_tool --lib
```

结果：两个命令均已完成编译，但在 Windows 本机启动测试二进制时报 `STATUS_ENTRYPOINT_NOT_FOUND`，退出码 `0xc0000139`。该问题发生在测试进程启动阶段，不是新增断言失败。当前本机以 `--no-run` 作为 Rust 编译门禁；完整 Rust 测试二进制执行需交给 Linux/GitHub Actions 或修复本机动态库入口点环境后再跑。前端 Node 自动化测试已在本机完成实际断言执行。

### 4. 撤回无实际调用链的外链白名单

PR158 中新增的：

- `https://open.chineselaw.com/`
- `https://passport.legalmind.cn/`

应从 `open_external_url` 白名单和相关测试中撤回，除非能证明前端存在真实调用链。

当前 OAuth 登录走底座 `perform_oauth_login_for_server`，由底座打开系统浏览器，不经过 `open_external_url`。

### 5. 清理无关格式化 diff

`bundle.rs` 中只保留元典必要改动：

- 新增 `YUANDIAN_MANIFEST_JSON`
- `write_mcp_servers` 写出 `yuandian-mcp/manifest.json`
- 元典相关测试

撤回其他 `include_str!` 常量换行、既有测试链式调用等纯格式化变化。

### 6. PR 文案更新

PR 标题建议改为：

```text
feat: 接入华宇元典远程 MCP OAuth 连接器
```

PR 正文补充：

- 改了什么：新增元典远程 MCP manifest、工具市场 OAuth 登录和状态查询。
- 为什么改：避免安装 MCP 配置后误判为已连接。
- 影响面：工具市场连接/断开流程、远程 MCP OAuth 状态展示。
- 数据说明：法律查询内容会发送到华宇元典云端 MCP 服务。
- 剩余风险：Linux ARM64 实际 OAuth 闭环尚未验证。

## 验收

修复后需要验证：

```bash
npm run build:ui
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml marketplace
./scripts/fork-guard.sh --fast
git diff --check
```

同时更新 PR 分支到最新 `main`，重跑 GitHub Actions。
