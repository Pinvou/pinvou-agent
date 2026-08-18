---
name: wecomcli-smartsheet
description: 何时用:仅当用户明确指向企业微信智能表格(链接 `https://doc.weixin.qq.com/smartsheet/*`、明说企微智能表格)时使用;泛指做个表格默认走本地工具,普通企微文档/在线表格见 wecomcli-doc。企微智能表格结构管理(子表、字段/列)与数据管理(记录增删改查),支持 docid 或 URL 定位;从零新建智能表格走 `wecom-cli doc create_doc`(doc_type=10)。
metadata:
  requires:
    bins: ["wecom-cli"]
  cliHelp: "wecom-cli doc --help"
---

# 企业微信智能表格管理

> `wecom-cli` 是企业微信提供的命令行程序，所有操作通过执行 `wecom-cli` 命令完成。

资源型技能，负责**智能表格**（`/smartsheet/*`，doc_type=10）的新建、结构（子表、字段/列）与数据（记录）管理。所有接口支持通过 `docid` 或 `url` 二选一定位文档。

## 调用方式

通过 `wecom-cli` 调用，品类为 `doc`：

```bash
wecom-cli doc <tool_name> '<json_params>'
```

> 智能表格各接口的 `docid`/`url` 二选一传入即可，以下示例以 `docid` 为主，URL 传入方式以此类推。

## 返回格式说明

所有接口返回 JSON 对象，包含以下公共字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `errcode` | integer | 返回码，`0` 表示成功，非 `0` 表示失败 |
| `errmsg` | string | 错误信息，成功时为 `"ok"` |

当 `errcode` 不为 `0` 时，说明接口调用失败，可重试 1 次；若仍失败，将 `errcode` 和 `errmsg` 展示给用户。

### 特殊错误码

| errcode | errmsg | 含义 | 处理方式 |
|---------|--------|------|----------|
| `851002` | `incompatible doc type` | 文档品类与所调用的接口不匹配 | 确认目标 URL 为 `/smartsheet/*`；若不是，请跳转到对应品类的 skill |

## 接口路由表

> **硬规则**：第二列是 `references/xxx.md` 链接的，命中这一行后**先用 `File(action="read")` 读取对应 references 文件，再构造命令**。写入/读取记录前，先用 `smartsheet_get_sheet` 拿到目标子表的 `sheet_id`，并用 `smartsheet_get_fields` 了解字段类型。

| 用户意图 | 参考位置 |
|---|---|
| 从零新建智能表格（空白） | 见下方「新建智能表格」 |
| 查询文档中所有子表 | 见下方「查询子表」 |
| 添加 / 改名 / 删除子表 | 见下方「子表管理」 |
| 查询子表字段/列 | 见下方「查询字段」 |
| 添加字段/列 | [references/smartsheet-field-types.md](references/smartsheet-field-types.md) |
| 改名 / 删除字段 | 见下方「字段管理」 |
| 查询子表记录 | [references/smartsheet-get-records.md](references/smartsheet-get-records.md) |
| 添加记录 / 更新记录 | [references/smartsheet-cell-value-formats.md](references/smartsheet-cell-value-formats.md) |
| 删除记录 | 见下方「删除记录」 |

---

## 一、新建智能表格

### 新建智能表格

从零新建一篇企微**智能表格**（doc_type=10）：空白。创建成功后返回 `docid` 和 `url`。

**命令**

```bash
wecom-cli doc create_doc '<JSON 参数>'
```

**参数**

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|---|---|---|---|---|
| `doc_type` | int | 是 | — | 固定传 `10`（智能表格） |
| `doc_name` | string | 是 | — | 表格标题，最多 255 个字符，超过会被截断 |

**注意事项**

- 新建智能表格文档**默认已含一个子表**，可通过 `smartsheet_get_sheet` 查询其 `sheet_id`，仅需多个子表时才调用 `smartsheet_add_sheet`。
- `docid` 仅在创建时返回，后续无法再获取，务必保存。

---

## 二、智能表格结构管理

### 查询子表

查询文档中所有子表信息，返回 `sheet_id`、`title`、类型等。

```bash
# 通过 docid 查询
wecom-cli doc smartsheet_get_sheet '{"docid": "DOCID"}'
# 通过 url 查询
wecom-cli doc smartsheet_get_sheet '{"url": "https://doc.weixin.qq.com/smartsheet/xxx"}'
```

### 子表管理

**添加子表** —— 添加空子表。新子表不含视图、记录和字段，需通过其他接口补充。

```bash
wecom-cli doc smartsheet_add_sheet '{"docid": "DOCID", "properties": {"title": "新子表"}}'
```

> 注意：新建智能表格文档默认已含一个子表，仅需多个子表时调用。

**修改子表标题** —— 需提供 `sheet_id` 和新 `title`。

```bash
wecom-cli doc smartsheet_update_sheet '{"docid": "DOCID", "properties":{"sheet_id":"SHEET_ID", "title":"新子表"}}'
```

**删除子表** —— 永久删除子表，**操作不可逆**。

```bash
wecom-cli doc smartsheet_delete_sheet '{"docid": "DOCID", "sheet_id": "SHEETID"}'
```

### 查询字段

查询子表的所有字段信息，返回 `field_id`、`field_title`、`field_type`。

```bash
wecom-cli doc smartsheet_get_fields '{"docid": "DOCID", "sheet_id": "SHEETID"}'
```

### 字段管理

**添加字段** —— 向子表添加一个或多个字段。单个子表最多 150 个字段。

```bash
wecom-cli doc smartsheet_add_fields '{"docid": "DOCID", "sheet_id": "SHEETID", "fields": [{"field_title": "任务名称", "field_type": "FIELD_TYPE_TEXT"}]}'
```

在添加字段前，请先参阅所有字段类型和定义 [字段类型参考](references/smartsheet-field-types.md)。

> 注意：如果是首次创建表并调用这个方法添加字段的情况下，调用本接口前，你必须确认已完成以下操作，否则会多出一个无用的默认列：
> 1. 已调用 `smartsheet_get_fields` 查看子表现有字段（新子表会自带一个默认文本字段）
> 2. 已调用 `smartsheet_update_fields` 将该默认字段重命名为你需要的第一个字段名，然后在本接口中只传入剩余的字段（不包含第一个字段）。

**更新字段标题** —— **只能改名，不能改类型**（`field_type` 必须传原始类型）。`field_title` 不能更新为原值。

```bash
wecom-cli doc smartsheet_update_fields '{"docid": "DOCID", "sheet_id": "SHEETID", "fields": [{"field_id": "FIELDID", "field_title": "新标题", "field_type": "FIELD_TYPE_TEXT"}]}'
```

**删除字段** —— 删除一列或多列字段，**操作不可逆**。`field_id` 可通过 `smartsheet_get_fields` 获取。

```bash
wecom-cli doc smartsheet_delete_fields '{"docid": "DOCID", "sheet_id": "SHEETID", "field_ids": ["FIELDID"]}'
```

---

## 三、智能表格数据管理

### 查询记录

查询子表全部记录。

```bash
# 通过 docid
wecom-cli doc smartsheet_get_records '{"docid": "DOCID", "sheet_id": "SHEETID"}'
# 或通过 URL
wecom-cli doc smartsheet_get_records '{"url": "https://doc.weixin.qq.com/smartsheet/xxx", "sheet_id": "SHEETID"}'
```

参见 [API 详情](references/smartsheet-get-records.md)。

### 添加记录（不带图片或文件）

添加一行或多行记录，单次建议 500 行内。

**调用前**必须先了解目标表的字段类型（通过 `smartsheet_get_fields`），重点关注 `field_type`。对于单选/多选（Option）字段，需注意匹配已有选项的 `id`。

```bash
wecom-cli doc smartsheet_add_records '{"docid": "DOCID", "sheet_id": "SHEETID", "records": [{"values": {"任务名称": [{"type": "text", "text": "完成需求文档"}], "优先级": [{"text": "高"}]}}]}'
```

各字段类型的值格式参见 [单元格值格式参考](references/smartsheet-cell-value-formats.md)。

### 添加记录（带图片或文件）

添加一行或多行记录，单次建议 500 行内。与 `smartsheet_add_records` 不同之处在于，可支持本地路径传入图片、文件。对于需要添加带图片或文件的记录，请使用此接口。传入后台后，后台将自动存储并转换为 `image_url`。

```bash
wecom-cli doc +smartsheet_add_records_auto_file '{"docid":"DOCID","sheet_id":"SHEETID","records":[{"values":{"图片":[{"image_path":"/path/to/image.jpg"}],"文件":[{"file_path":"/path/to/file.txt"}]}}]}'
```

### 更新记录（不带图片或文件）

更新一行或多行记录，单次建议在 500 行内。需提供 `record_id`（通过 `smartsheet_get_records` 获取）。支持通过 `key_type` 指定 values 的 key 使用字段标题或字段 ID：

- `CELL_VALUE_KEY_TYPE_FIELD_TITLE`：key 为字段标题
- `CELL_VALUE_KEY_TYPE_FIELD_ID`：key 为字段 ID

```bash
wecom-cli doc smartsheet_update_records '{"docid": "DOCID", "sheet_id": "SHEETID", "key_type": "CELL_VALUE_KEY_TYPE_FIELD_TITLE", "records": [{"record_id": "RECORDID", "values": {"任务名称": [{"type": "text", "text": "更新后的内容"}]}}]}'
```

**注意**：创建时间、最后编辑时间、创建人、最后编辑人字段不可更新。

### 更新记录（更新图片或文件字段）

更新一行或多行记录，单次建议在 500 行内。与 `smartsheet_update_records` 不同之处在于，可支持本地路径传入图片、文件。对于需要更新记录中的图片或文件，请使用此接口。传入后台后，后台将自动存储并转换为 `image_url`。

```bash
wecom-cli doc +smartsheet_update_records_auto_file '{"docid": "DOCID", "sheet_id": "SHEETID", "key_type": "CELL_VALUE_KEY_TYPE_FIELD_TITLE", "records": [{"record_id": "RECORDID", "values": {"图片":[{"image_path":"/path/to/image.jpg"}],"文件":[{"file_path":"/path/to/file.txt"}]}}]}'
```

### 删除记录

删除一行或多行记录，单次必须在 500 行内。**操作不可逆**。`record_id` 通过 `smartsheet_get_records` 获取。极速版智能表格不支持此接口。

```bash
wecom-cli doc smartsheet_delete_records '{"docid": "DOCID", "sheet_id": "SHEETID", "record_ids": ["RECORDID1", "RECORDID2"]}'
```

---

## 典型工作流

### 新建并搭建表结构

1. **新建智能表格** →
```bash
wecom-cli doc create_doc '{"doc_type": 10, "doc_name": "项目任务表"}'
```
，保存返回的 `docid`。
2. **了解默认子表** → `smartsheet_get_sheet` 拿到默认子表的 `sheet_id` → `smartsheet_get_fields` 查看默认字段。
3. **搭建列** → 先 `smartsheet_update_fields` 改默认字段名，再 `smartsheet_add_fields` 补充其余字段。

### 智能表格结构操作

1. **了解表结构** →
```bash
wecom-cli doc smartsheet_get_sheet '{"docid": "DOCID"}'
```
 →
```bash
wecom-cli doc smartsheet_get_fields '{"docid": "DOCID", "sheet_id": "SHEETID"}'
```
2. **创建表结构** → `smartsheet_add_sheet` 添加子表 → `smartsheet_add_fields` 定义列
3. **修改表结构** → `smartsheet_update_fields` 改列名 / `smartsheet_delete_fields` 删列

### 智能表格数据操作

1. **读取数据** →
```bash
wecom-cli doc smartsheet_get_records '{"docid":"DOCID","sheet_id":"SHEETID"}'
```
2. **写入数据** → 先 `smartsheet_get_fields` 了解列类型 → 若涉及成员（USER）字段，先通过 `wecomcli-contact` 的 `get_userlist` 查找人员 userid → `smartsheet_add_records` 写入
3. **更新数据** → 先 `smartsheet_get_records` 获取 record_id → 若涉及成员（USER）字段，先通过 `wecomcli-contact` 的 `get_userlist` 查找人员 userid → `smartsheet_update_records` 更新
4. **删除数据** → 先 `smartsheet_get_records` 确认 record_id → `smartsheet_delete_records` 删除

## 跨技能依赖

| 依赖技能 | 典型协作场景 | 数据流向 |
|---|---|---|
| `wecomcli-contact` | 成员（USER）类型字段需填 `user_id`，不能直接用姓名 | `get_userlist` 按姓名查到 userid → 本 skill 写入 |
| `wecomcli-msg` | 用户要求把智能表格链接发给某人/某群 | 本 skill 新建后返回 `url` → `wecomcli-msg` 发送链接 |

> **注意**：成员（USER）类型字段需要填写 `user_id`，不能直接使用姓名。必须先通过 `wecomcli-contact` 技能的 `get_userlist` 接口按姓名查找到对应的 `userid` 后再使用。
