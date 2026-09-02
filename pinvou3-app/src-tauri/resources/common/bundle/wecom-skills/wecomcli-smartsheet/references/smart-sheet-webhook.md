# 智能表格 Webhook 兜底写入

本文档是 `wecom-cli smartsheet records add` / `wecom-cli smartsheet records update` 的 fallback 参考。当 CLI 因企业规模限制无法写入智能表格时，通过企业微信智能表格 Webhook 直接写入数据。

> **格式隔离**：本文的字段值格式只适用于 Webhook，与 CLI `records add` / `records update` 使用的 `references/smart-sheet-record-values.md` 格式不同。文本、链接、图片、日期等写法均可能不同，禁止混用。

## 一、Fallback 触发流程

### 何时切换到 Webhook

先走 CLI 正常链路。仅在以下情况切换：

- 优先判据：CLI 返回 `errcode: 851003`，或 `errmsg` 包含 `no authority`。这通常意味着企业可见范围超过 10 人，CLI 写入接口被限制。
- 或错误信息明确指向企业规模、可见范围或成员数超限。
- 参数错误、字段错误、文档不存在等其他错误不切换 Webhook，应按原错误排查。
- 仅 `records add` 与 `records update` 支持此兜底；删除记录或修改表结构不走 Webhook。

### 向用户临时索取两项信息

触发切换后，每次对话内临时获取，用完即弃，不写入文件、配置、日志说明或其他持久化位置：

1. **Webhook 完整 URL**
   - 在智能表格右上角菜单选择「接收外部数据」→ 选择目标工作表 → 开启 → 复制。
   - 格式形如 `https://qyapi.weixin.qq.com/cgi-bin/wedoc/smartsheet/webhook?key=XXXXXX`。
   - URL 相当于目标表的写入密钥；用户可关闭「接收外部数据」使其失效，不得在回复中回显完整 URL 或 key。
2. **schema 示例 JSON**
   - 从同一「接收外部数据」页面复制。
   - 内容包含字段 ID 到字段名的映射（`schema`），以及各字段的 Webhook 写入格式示例（`add_records`）。

示例：

```json
{
  "schema": {
    "fABCD1": "任务名称",
    "fABCD2": "状态",
    "fABCD3": "负责人",
    "fABCD4": "截止日期"
  },
  "add_records": [
    {
      "values": {
        "fABCD1": "示例任务",
        "fABCD2": [{"text": "未开始"}],
        "fABCD3": [{"user_id": ""}],
        "fABCD4": "1742400000000"
      }
    }
  ]
}
```

可使用以下话术：

> CLI 写入接口返回了 `851003 no authority`，通常是企业可见范围超过 10 人导致的限制。请把目标表的 Webhook 地址和「接收外部数据」页面的示例 JSON 发我，我会通过 Webhook 写入；这些信息仅在本轮使用，不会保存到本地。

## 二、构建并发送请求

### 字段匹配

从用户提供的 `schema` 将自然语言字段名映射到字段 ID：

- 可基于近义词匹配，例如「标题」对应标题、名称或主题，「状态」对应状态或阶段，「处理人」对应负责人或责任人。
- 匹配不唯一时先向用户确认，禁止猜测字段。
- `values` 的 key 必须使用 schema 中真实存在的字段 ID。

需要更多 payload 示例时，按需阅读 `smart-sheet-webhook-examples.md`。

### 日期处理

用户输入「今天」「明天」「3 月 15 日」或 `2025-03-01 09:00` 等自然语言日期时，根据当前日期及时区换算为毫秒时间戳字符串，例如 `"1742400000000"`。Webhook 不接受 CLI 使用的可读日期字符串。

### 请求结构

Webhook 是标准 HTTP 接口，不经过 `wecom-cli`。使用当前环境可用的 HTTP 客户端发送请求：

| 项 | 值 |
| --- | --- |
| Method | `POST` |
| URL | 用户提供的 Webhook 完整 URL（含 `?key=XXX`） |
| Header | `Content-Type: application/json` |
| Body | 包含 `add_records` 和/或 `update_records` 的 JSON 对象 |

不要把包含 Webhook URL 的命令写入脚本或仓库文件，也不要把完整 URL 输出给用户。

仅新增：

```json
{
  "add_records": [
    {"values": {"fABCD1": "...", "fABCD2": [{"text": "..."}]}}
  ]
}
```

仅更新：

```json
{
  "update_records": [
    {"record_id": "REC_xxx", "values": {"fABCD2": [{"text": "已完成"}]}}
  ]
}
```

Webhook 只能更新此前通过 Webhook 写入的记录，人工创建或通过普通接口创建的记录无法更新。

同一请求同时新增和更新：

```json
{
  "add_records": [{"values": {"fABCD1": "..."}}],
  "update_records": [{"record_id": "REC_xxx", "values": {"fABCD2": [{"text": "已完成"}]}}]
}
```

### 结果处理

- Webhook 返回成功后，按 `references/smart-sheet-read.md` 读取目标数据，确认真实状态与预期一致。
- 向用户简洁说明已通过 Webhook 写入；遵守 `SKILL.md` 的交互规范，不在回复中暴露内部 ID。
- 返回非 0 `errcode` 时按下方错误码处理，不盲目重试。

## 三、Webhook 字段值格式

| 字段类型 | value 示例 | 说明 |
| --- | --- | --- |
| 文本 | `"产品登录页白屏"` 或 `[{"type":"text","text":"产品登录页白屏"}]` | 简单字符串更简洁 |
| 数字 / 货币 | `58000` | 使用数字，不加引号 |
| 进度 / 百分数 | `30` | `30` 表示 30%；不要传 `0.3` |
| 复选框 | `true` / `false` | JSON 布尔值 |
| 日期 | `"1740806400000"` | 毫秒时间戳字符串 |
| 成员 | `[{"user_id":"lisi"}]`、`["张三"]` 或 `[]` | 优先使用 userid；不指定时传空数组 |
| 单选 | `[{"text":"已完成"}]` | 选项文本必须与表格预设完全一致 |
| 多选 | `[{"text":"前端"},{"text":"后端"}]` | 每个选项一个对象 |
| 链接 | `[{"text":"需求文档","link":"https://doc.example.com"}]` | 数组格式 |
| 地理位置 | `[{"latitude":"31.23040","longitude":"121.47370","source_type":1,"title":"上海市徐汇区"}]` | 最多一条 |
| 图片 | `[{"title":"screenshot.png","image_base64":"iVBORw0KGgo..."}]` | 只传纯 base64，不带 `data:image/...;base64,` 前缀 |
| 电话 / 邮箱 / 条码 | `"13800138000"` | 字符串 |

## 四、不支持的字段

以下字段由系统维护或结构特殊，Webhook 写入时跳过，不要因为这些字段中止整次写入：

公式、自动编号、查找引用、关联字段、创建人、最后编辑人、创建时间、最后编辑时间、群聊、文件附件。

## 五、频率与批量限制

- 单工作表不超过 3000 条/分钟。
- 单文档不超过 10000 条/分钟。
- 数据量大时分批发送，每批不超过 500 条。
- 同时遵守 `SKILL.md` 中超过 100 条写入前必须获得用户确认的规则。

## 六、常见错误码

| errcode | 原因 | 处理方式 |
| --- | --- | --- |
| `2023033` | 图片 base64 带有 `data:image/...;base64,` 前缀 | 去掉前缀，只传纯 base64 |
| `40014` | Webhook key 无效或已过期 | 请用户重新从「接收外部数据」获取 Webhook 地址 |
| `45033` | 超出频率限制 | 降低速率或缩小批次 |
| `-100035` | testapi 域名不稳定或超时 | 改用正式域名 `qyapi.weixin.qq.com` |
| `2023001` | 字段 ID 不存在 | 对照用户提供的 schema 检查字段 ID |
| `2023010` | 单选或多选的值不在预设列表 | 确认选项文本完全一致，包括大小写 |
| `2023012` | 更新时 record_id 不存在或不可更新 | 只更新此前通过 Webhook 写入的记录 |

## 七、参考文件

- 真实场景示例：`smart-sheet-webhook-examples.md`

仅在需要示例时阅读，避免每次加载无关内容。
