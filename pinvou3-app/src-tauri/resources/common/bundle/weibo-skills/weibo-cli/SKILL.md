---
name: weibo-cli
version: 0.9.1
description: "何时用：用户明确需要通过微博官方 CLI 操作微博内容、评论、转发、关注关系、用户信息、搜索、热搜/趋势、开放平台命令目录或账号/权限诊断时。所有发布、评论、转发、关注、取关等写操作必须先向用户复述对象和内容并取得明确确认。"
metadata:
  requires:
    bins: ["weibo-cli"]
  cliHelp: "weibo-cli --help"
---

# weibo-cli

微博官方 CLI，支持 OAuth 授权、账号信息、微博内容、评论、转发、关注关系、用户信息、搜索、热搜/趋势以及开放平台命令目录查询。

## 安装与初始化

在品悟中，`weibo-cli` 由应用代为安装与管理，模型不需要也不要自行执行安装或升级命令。版本随品悟应用更新自动就位。

## 认证

使用微博能力前必须先在品悟工具商店完成微博连接。品悟会调用微博 CLI 的 device-code 登录流程，由用户在浏览器中确认授权，微博 CLI 自行把授权态写入本机 keychain 或 `~/.weibo-cli/`。

不要引导用户通过微博 token 环境变量登录；品悟 agent shell 会过滤敏感环境变量，依赖这类变量会导致工具商店连接态和实际会话调用不一致。检测到未登录时，提示用户回到品悟工具商店重新连接微博。

登录状态检查：

```bash
weibo-cli auth whoami --output json
```

也可以在需要排查账号、认证、额度或开放平台权限时运行：

```bash
weibo-cli doctor --output json
```

登出由品悟工具商店执行，不要在普通任务中主动登出用户。

## 命令发现

微博开放平台命令可能随账号套餐、开发者认证和权限变化。遇到不确定的命令或参数时，先查询当前账号可用命令，再查看目标命令详情。

```bash
weibo-cli commands list --available --output json
weibo-cli commands show <group> <action> --output json
```

如果用户要了解所有命令，包括当前账号不可用的能力，可以使用：

```bash
weibo-cli commands list --all --output json
```

## 常见入口

```bash
# 查看当前账号、套餐和使用情况
weibo-cli me --output json

# 查看首页时间线
weibo-cli statuses friends_timeline/biz --output json

# 按微博 ID 批量获取内容
weibo-cli statuses show_batch/biz --ids <ids> --output json

# 按昵称批量查询用户
weibo-cli users show_batch/other --screen_name <screen_name> --output json

# 查看关注列表
weibo-cli friendships friends/biz --output json
```

## 安全规则

- 查询类命令默认使用 `--output json`，便于后续解析、筛选和引用。
- 未知命令先使用 `commands list/show` 确认，不要凭记忆猜参数。
- 发布、评论、转发、关注、取关等写操作执行前，必须向用户复述目标对象、将要提交的文本或影响范围，并等待用户明确确认。
- 用户只要求“写一条微博”“拟一条评论”时，只生成草稿，不执行发布。
- 不调用 `weibo-cli upgrade`、`weibo-cli check_update`，升级由品悟宿主管理。
- 不输出、不请求、不保存微博 token、Cookie 或任何授权凭证。
- 不把微博返回的私人信息、账号标识或互动对象扩散到无关上下文；只保留完成当前任务所需的最小内容。

## 失败处理

- 未登录或提示缺少登录令牌：请用户到品悟工具商店连接微博。
- 权限不足、套餐限制、开发者认证不足：运行 `weibo-cli doctor --output json`，根据返回结果说明缺少的账号状态或权限。
- 命令不存在或参数错误：运行 `weibo-cli commands list --available --output json` 和 `weibo-cli commands show <group> <action> --output json` 后重选命令。
- 网络、超时或开放平台错误：说明当前命令未完成，不要假设操作成功；写操作失败时不要自动重试，除非用户再次确认。
