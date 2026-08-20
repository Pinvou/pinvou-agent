# 腾讯文档鉴权说明

腾讯文档 MCP 的 Token 由 Pinvou 工具商店的「腾讯文档 MCP」连接器统一管理：

- Token 只保存在本机系统凭据中，连接器会自动注入到四个服务（tencent-docs / tdoc-slide / tdoc-doc / tdoc-sheet，共用同一 Token）的请求头。
- **不要**在对话中向用户索要、展示或手动配置 Token；模型侧无需任何鉴权操作。

## 鉴权类错误处理

调用工具返回鉴权错误时，按错误信息引导用户：

| 错误场景 | 处理方式 |
|------|---------|
| `invalid_token` / `token_invalid`（400006） | 提示用户：「腾讯文档 Token 已失效，请在 Pinvou 工具商店的『腾讯文档 MCP』卡片重新连接，获取新 Token 后粘贴更新。」 |
| `vip_required`（400007） | 提示用户：「当前操作需要腾讯文档 VIP 权限。」（升级入口：https://docs.qq.com/vip?immediate_buy=1&part_aid=persnlspace_mcp ） |
| 网络错误 | 提示用户检查网络或代理后重试 |

用户更新 Token 的官方入口：[腾讯文档开放平台授权页](https://docs.qq.com/scenario/open-claw.html)（QQ / 微信扫码登录后获取个人 Token），更新请在工具商店操作，不在对话中进行。
