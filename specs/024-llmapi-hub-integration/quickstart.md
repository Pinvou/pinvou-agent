# Quickstart：验证 Pinvou 接入 LLM API Hub

## 前置条件

1. 当前分支为 `024-llmapi-hub-integration`。
2. 验证环境为 Windows 桌面端；Linux 和其他非 Windows 平台本次不执行中转站查询/绑定或模型调用验证。
3. LLM API Hub 可通过 `https://www.ma-xiao.com/llmapi/v1` 访问。
4. New API 后台已配置 DeepSeek 渠道和可用模型。
5. Windows 设备可读取有效 BIOS SN。
6. Pinvou 后端可由 BIOS SN 派生 `device_binding_id`，并直接使用该值作为 `pinvou_user_id`。
7. New API 管理凭证已通过安全方式写入 Pinvou 凭证存储，不写入仓库或普通配置。

## 本地配置检查

## 当前实现边界

- 本实现仅在 Windows 上启用，非 Windows 平台后端返回 `unsupported_platform`。
- 聊天链路默认不切换到中转站；需要显式设置 `PINVOU3_USE_LLMAPI_HUB=1` 后，才会在发送消息前校验/绑定，并把中转站作为 OpenAI-compatible 临时模型注入现有 `EnginePool`。
- 已按 QuantumNous/new-api 源码接入标准用户查询接口：`GET /api/user/search`，请求需携带管理员 `Authorization: Bearer <access_token>` 和 `New-Api-User: <admin_user_id>`。
- QuantumNous/new-api 标准 `/api/token` 只允许当前登录用户管理自己的 token，未提供管理员代指定用户创建/读取明文 token 的接口；因此首次查询绑定如需自动准备 token，必须额外配置 `PINVOU3_LLMAPI_CREATE_TOKEN_ENDPOINT` 指向中转站侧新增的管理端点。
- New API 管理凭证支持两种格式：`{"user_id":1001,"access_token":"..."}` 或 `1001:<access_token>`；如只保存 access token，则需设置 `PINVOU3_LLMAPI_ADMIN_USER_ID`。
- 已 ready 的本地绑定不依赖管理员凭据即可用于聊天；只有首次查询绑定、重试绑定或远端禁用同步才需要 New API 管理凭据。
- 本地绑定元数据保存到 `~/.pinvou3/llmapi-hub/bindings.json`，只保存 token 的 `credential_ref`，不保存 token 明文或 BIOS SN 明文。

```powershell
cd E:\Pinvou\pinvou3\pinvou3-app\src-tauri
cargo test -p pinvou3-tauri --lib
```

预期：

- 测试通过。
- 日志中不出现 New API token、DeepSeek API Key、管理凭证或 BIOS SN 明文。

## 首次使用查询绑定验证

1. 使用一台没有中转站绑定的 Windows 测试设备。
2. 确认该设备可读取有效 BIOS SN。
3. 打开 AI 助手并发送一条简单消息，例如“你好，简单介绍一下你自己”。
4. 后端应自动执行：
   - 由当前设备 BIOS SN 派生 `device_binding_id`，并将 `pinvou_user_id` 设为同一值。
   - 查询既有 New API 用户，不创建后台用户。
   - 调用配置的专用管理端点创建或取得 New API token；若未配置该端点，流程停在可恢复失败状态。
   - 如配置了策略端点，则设置默认额度、模型权限和限流；未配置时视为 token 创建端点已完成策略处理。
   - 将 token 写入 `credential_store`。
   - 保存绑定元数据。
   - 继续调用 `https://www.ma-xiao.com/llmapi/v1/chat/completions`。
5. 前端展示模型回复。

验收：

- 用户未输入、未看到 New API token。
- `get_llmapi_status` 返回 `provisioning_status=ready`。
- 绑定记录包含本地设备派生身份、设备绑定标识、New API 用户/token ID，但不包含 token 或 BIOS SN 明文。

## 设备绑定失败验证

1. 模拟 BIOS SN 读取失败、BIOS SN 无效或与可选绑定校验值不一致。
2. 打开 AI 助手并发送消息。

验收：
- 后端不创建 New API 用户或 token。
- 后端不调用 `https://www.ma-xiao.com/llmapi/v1/chat/completions`。
- 前端收到设备绑定相关的友好错误分类。

## 幂等和补偿验证

1. 模拟 lookup_user 成功但 create_token 失败。
2. 再次发送 AI 助手消息或执行管理员重试。
3. 系统应复用已查询到的 New API 用户，并继续创建或取得 token。

验收：

- 同一个 `pinvou_user_id + device_binding_id` 不创建 New API 用户，且不产生多个有效 token；当前实现中两者取值相同。
- 重试后状态可进入 `ready`。
- 失败原因可在管理员状态中查看，且已脱敏。

## 额度展示验证

1. 完成一次模型调用。
2. 调用 `get_llmapi_status`。
3. 检查 quota 快照。

验收：

- `used_tokens` 根据响应 `usage.total_tokens` 增加。
- `remaining_tokens` 对应减少。
- 如果响应缺少 usage，`unmetered_call_count` 增加。

## 禁用用户验证

1. 管理员调用 `set_llmapi_user_enabled` 将测试用户设为 `false`。
2. 测试用户再次发送 AI 助手消息。

验收：

- Pinvou 后端不再调用中转站。
- 用户看到 AI 服务不可用的友好提示。
- 如果 New API 远端禁用失败，本地禁用仍然生效。

## 安全检查

执行以下检查：

```powershell
rg -n "sk-|Bearer [A-Za-z0-9]|DeepSeek API Key|New API token|SerialNumber|BIOS SN" `
  "$env:USERPROFILE\.pinvou3" `
  E:\Pinvou\pinvou3\pinvou3-app\src-tauri\src `
  E:\Pinvou\pinvou3\pinvou3-app\src
```

预期：

- 不应在 `settings.json`、普通日志、前端代码或导出数据中找到真实 token 或 BIOS SN 明文。
- 如命中测试字符串，确认其为单元测试假值。

## 中转站入口验证

无 token 访问模型列表应返回 401，表示请求已到达 New API：

```powershell
curl.exe -i https://www.ma-xiao.com/llmapi/v1/models
```

带测试 token 访问应返回模型列表：

```powershell
curl.exe -i https://www.ma-xiao.com/llmapi/v1/models `
  -H "Authorization: Bearer <测试 New API token>"
```

注意：测试 token 不得提交到仓库，不得写入聊天记录或普通文档。

## 非 Windows 平台验证

在 Linux 或其他非 Windows 平台上：

1. 打开 Pinvou。
2. 尝试进入本功能相关入口或直接调用相关 Tauri 命令。

验收：
- 前端不展示可用的 LLM API Hub 查询绑定入口，或明确显示当前平台暂不支持。
- 后端返回 `unsupported_platform`。
- 不创建 New API 用户、token 或本地中转站绑定。
