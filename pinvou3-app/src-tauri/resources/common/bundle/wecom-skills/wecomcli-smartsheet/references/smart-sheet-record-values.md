# 记录值（Record Value）类型参考

本文件主要说明 `wecom-cli smartsheet records add/update/delete` 中记录值的写入格式。记录的 `fields` / `values` 是一个 key-value 映射，key 为字段名，value 的格式取决于字段类型。

> **与 `records query` 返回值区分**：`wecom-cli smartsheet records query` 是 SQL 查询接口，命令返回体外层为 `errcode` + `values string[]`；每个 `values[i]` 解析后读取其中的 `rows`。SQL 中用字段名查询，解析后的 `rows` key 默认也是字段名；解析查询结果时以 `references/smart-sheet-read.md` 的记录读取章节为准，不要把下表的写入格式原样套用到 SQL 查询返回。

| 字段类型短枚举值 | value 格式 | 示例 |
| --- | --- | --- |
| `text` | string | `"文本字符串"` |
| `number` | double 数值 | `123.45` |
| `checkbox` | bool 布尔值 | `true` |
| `date_time` | string | 必须严格按照 `"YYYY-MM-DD HH:mm:ss"` 标准时间格式 |
| `image` | CellImageValue 数组 | `[{"id": "xxx", "title": "图片", "imageUrl": "https://..."}]` |
| `attachment` | CellAttachmentValue 数组 | `[{"id": "xxx", "title": "文件名", "fileUrl": "https://..."}]` |
| `user` | CellUserValue 数组 | 读取时返回 `[{"userId": "<userid>", "userName": "<姓名>"}]`；写入时优先传 `userName` 写入（若报错则改传 `userId`，通过 `wecomcli-contact` 技能获取） |
| `url` | CellUrlValue 数组 | `[{"text": "链接名", "link": "https://..."}]` |
| `select` | Option 数组 | `[{"id": "服务端返回的选项ID", "text": "选项A"}]` |
| `progress` | double（0~100） | `75.5` |
| `phone_number` | string | `"<phone_number>"` |
| `email` | string | `"<email>"` |
| `single_select` | Option 数组 | `[{"id": "服务端返回的选项ID", "text": "选项A"}]` |
| `reference` | CellReferenceValue 数组 | `[{"record_id": "rec_xxx"}]`（关联的记录 ID） |
| `location` | CellLocationValue 数组 | `[{"id": "<腾讯地图给的UID>", "source_type": 1, "title": "<地点名称>", "latitude": "<纬度>", "longitude": "<经度>", "address": "<详细地址>"}]` |
| `autonumber` | 只读 | 系统自动生成，不可写入 |
| `currency` | double | `99.99` |
| `wwgroup` | CellGroupValue 数组 | `[{"chat_id": "<chat_id>"}]` |
| `percentage` | double（0~1） | `0.85`（显示为 85%） |
| `barcode` | string | `"<barcode_text>"` |

---

## 上传附件到文档空间

根据文件类型选择上传命令，并获取文件对应的 URL：

- 图片使用 `wecom-cli smartsheet images upload`。
- PDF、Office 文件、`.zip` 压缩包等非图片文件使用 `wecom-cli smartsheet files upload`。

写入智能表格的图片字段（`CellImageValue.imageUrl`）或文件字段（`CellAttachmentValue.fileUrl`）时，必须先通过对应命令将文件上传到目标智能表格所在文档空间，再把返回的 `url` 写入记录字段。两个命令的参数完全相同：

```bash
# 图片
wecom-cli smartsheet images upload --json '{"media_id": "<media_id>", "docid": "<文档ID>"}'

# 非图片文件
wecom-cli smartsheet files upload --json '{"media_id": "<media_id>", "docid": "<文档ID>"}'
```

**入参：**

| 参数 | 类型 | 必填 | 说明 |
|------|------|:----:|------|
| `media_id` | string | 是 | 媒体文件 ID，用户的消息中主动提供，或通过 `wecomcli-media` 的 `media upload` 获取 |
| `docid` | string | 是 | 目标智能表格的文档 ID |

**出参：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `url` | string | 上传后的文件访问 URL。图片返回直接图片资源 URL，通常形如 `https://w...qpic.cn/...`；非图片文件返回文件分享链接，通常形如 `https://d...qq.com/...?k=...` |

**调用示例：**

```bash
# 上传图片
wecom-cli smartsheet images upload --json '{"media_id": "mcabc123...", "docid": "a1_xxx"}'

# 上传非图片文件
wecom-cli smartsheet files upload --json '{"media_id": "mcabc123...", "docid": "a1_xxx"}'
```

---

## 各类型 CellValue 详细结构

### CellUserValue（人员）

```json
[{ "userId": "<userid>", "userName": "<姓名>" }]
```

> **读取与写入规范**：
> - **读取**：始终返回 `userId` 和 `userName`。
> - **写入**：优先支持直接传 `userName` 写入（如 `[{"userName": "张三"}]`）。如果传 `userName` 报错（例如姓名错误或存在同名人员），则**必须**使用 `wecomcli-contact` 技能搜索该人员的 `userid`，再通过 `userId` 进行重试写入（如 `[{"userId": "xxx"}]`）。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `userId` | string | userid。读取时必返；写入时，若按 `userName` 写入失败，则必须通过 `wecomcli-contact` 获取 `userid` 并传入此字段 |
| `userName` | string | 姓名。读取时必返；写入时，优先直接传入此字段进行写入 |

### CellUrlValue（超链接）

```json
[{ "text": "<链接名>", "link": "<url>" }]
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `text` | string | 链接显示文本 |
| `link` | string | 链接地址 |

### CellImageValue（图片）

```json
[{ "title": "图片名", "imageUrl": "https://..." }]
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `title` | string | 图片标题 |
| `imageUrl` | string | 图片 URL。通过 `wecom-cli smartsheet images upload` 上传图片后，取返回的 `url` 写入。详见“上传附件到文档空间” |

### CellAttachmentValue（文件）

```json
[{ "title": "文件名.pdf", "fileUrl": "https://..." }]
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `title` | string | 文件名（读取返回字段，写入可不传） |
| `fileUrl` | string | 文件 URL。通过 `wecom-cli smartsheet files upload` 上传非图片文件后，取返回的 `url` 写入。详见“上传附件到文档空间” |

### CellLocationValue（地理位置）

```json
[{
  "id": "<腾讯地图的UID>", // 必填，由腾讯地图提供，不可捏造
  "source_type": 1, // 来自腾讯地图
  "title": "<地点名称>",
  "latitude": "<纬度>",
  "longitude": "<经度>",
  "address": "<详细地址>"
}]
```

> 目前没有接口获取腾讯地图位置信息，故目前无法插入地图信息。若用到相关功能，请提醒用户手动插入。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | **必填且不能为空**。|
| `source_type` | int | **必填**。目前只支持填入1，表示来自腾讯地图 |
| `title` | string | 位置名称 |
| `latitude` | string | 纬度 |
| `longitude` | string | 经度 |
| `address` | string | 详细地址 |

### CellReferenceValue（关联）

```json
[{ "record_id": "rec_001" }]
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `record_id` | string | 关联的记录 ID |

### CellGroupValue（群）

```json
[{ "chat_id": "<chat_id>" }]
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `chat_id` | string | 群聊 ID |

### 条码（barcode）

```json
"<barcode_text>"
```

条码字段直接传入条码内容字符串，例如：`"BARCODE-TEST-001"`

### 电话（phone_number）

电话字段直接传字符串：

```json
"13800138000"
```

或：

```json
"0755-12345678"
```

禁止写成数组，禁止写成 `CellTextValue`。只允许数字和合法分隔符，禁止写入 `x`、`*`、`#`、中文占位符或脱敏号码。如果用户提供`138xxxx0001`、`138****0001` 等脱敏号码，需要用简洁自然语言询问用户选择：转换为文本字段，或统一转为纯数字占位号码（如 `13800000001`、`13800000002`，同一批内保持唯一）；不得自行猜测。

### Option（单选/多选）

```json
[{ "id": "选项ID", "text": "选项文本" }]
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 选项 ID（必须使用服务端返回的真实 ID） |
| `text` | string | 选项文本 |
