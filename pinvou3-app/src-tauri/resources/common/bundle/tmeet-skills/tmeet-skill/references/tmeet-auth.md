# tmeet auth — 认证

管理腾讯会议 CLI 的登录状态。

---

## login — 登录

```bash
# 前台运行（推荐）
tmeet auth login

# 禁用自动打开浏览器，仅输出授权 URL
tmeet auth login --no-browser
```

执行后先输出授权 URL，然后**阻塞等待最多 300s** 用户在浏览器完成腾讯会议 OAuth 授权，授权成功后 CLI 自动将凭证保存到本地。

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|:----:|--------|------|
| `--no-browser` | bool | — | `false` | 禁用自动打开浏览器。`false`（默认）会尝试自动打开系统默认浏览器跳转到授权 URL；`true` 则仅输出授权 URL，需用户手动在浏览器中打开 |

**注意：**
- `auth login` 是**阻塞命令**——执行后会先输出授权 URL，然后**阻塞等待最多 300s** 用户在浏览器完成 OAuth 授权。**必须前台运行**：前台运行保持进程与终端连接，OAuth 回调能正常将凭证写入本地。**不要**用后台方式（`&`）运行——后台进程脱离控制终端会导致凭证写入失败。
- 如果当前 Agent 是 **Hermes** 且系统**没有默认浏览器**，请**不要**尝试执行 `tmeet auth login` 命令，而是直接告知用户：「请在终端中手动执行 `tmeet auth login` 完成登录授权，获取授权 URL 后在浏览器中打开完成授权。」
- 第一次调用 `auth login` 命令或者切换了模型，必须将当前使用的 agent 类型（如 Cursor / Claude Desktop / Cline / CodeBuddy）和模型名称（如 Claude 3.5 Sonnet / GPT-4o / DeepSeek）分别写入 `TMEET_AGENT` 和 `TMEET_MODEL` 环境变量，**不需要向用户询问**。
- 执行后必须从输出中提取授权 URL，**完整展示给用户**，并明确提示用户在浏览器中打开完成授权，不得省略。
- 如果已经登录，命令返回错误 `user has been initialized`，无需重复登录。
- 如需重新登录，先执行 `tmeet auth logout` 清除凭证后再登录。

**典型输出：**
```
Please open the following URL in your browser to authorize
authorize url: https://...
Login successful. Start managing your meetings using tmeet.
```

---

## logout — 登出

```bash
tmeet auth logout
```

清除本地保存的所有认证凭证。登出后所有需要认证的命令都将失败，需重新执行 `auth login`。

**无任何可选参数。**

---

## status — 查看登录状态

```bash
tmeet auth status
```

显示当前登录状态及凭证信息，**无需登录即可执行**。

**无任何可选参数。**

**典型输出（已登录，Token 有效）：**
```
Logged in
  OpenId:  xxx
  UserName:  xxx
  AccessToken:  valid (expires at 2026-05-01 10:00:00, remaining 25d 0h 6m)
  RefreshToken: valid (expires at 2026-07-01 10:00:00, remaining 86d 0h 6m)
```

**典型输出（已登录，Token 已过期）：**
```
Logged in
  OpenId:  xxx
  AccessToken:  expired (at 2026-03-01 10:00:00)
  RefreshToken: valid (expires at 2026-07-01 10:00:00, remaining 86d 0h 6m)
```

**典型输出（未登录）：**
```
Not logged in. Please use 'tmeet auth login' to authenticate.
```

---

## 常见错误

| 错误现象 | 原因 | 解决方案 |
|---------|------|---------|
| `user has been initialized` | 已登录，重复执行 login | 直接使用，或先 logout 再 login |
| `user config is empty` | 未登录就执行其他命令 | 先执行 `tmeet auth login` |

## 参考

- [tmeet](../SKILL.md) — 全部命令概览
