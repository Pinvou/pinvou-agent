---
name: wecomcli-sheet
description: 何时用:仅当用户明确指向企业微信『在线表格』或提供 doc.weixin.qq.com/sheet/ 链接时使用;泛指做表格默认先按消歧确认,智能表格内容操作走 wecomcli-smartsheet,本地表格默认走本地工具。在线表格:新建、导入 CSV/Excel、读取信息与数据、修改内容、追加行、子表管理;文档公共操作走 wecomcli-doc-manage。
metadata:
  requires:
    bins: ["wecom-cli"]
---

# 企业微信在线表格管理

> 执行任何 `wecom-cli` 命令前，必须先读取并完成 `wecomcli-shared` 技能的公共前置检查。

资源型 skill，负责在线表格（`sheet`）的新建、导入与内容读写及子表管理。

## 适用范围

### 适用

- 新建 / 导入企微在线表格
- 读取 / 修改 / 追加在线表格内容
- 添加 / 删除在线表格子表

### 不适用

- 搜索文档 / 修改文档权限 / 重命名 / 加成员 → 改用 `wecomcli-doc-manage`
- 用户给的链接是 `https://doc.weixin.qq.com/smartsheet/...` → 改用 `wecomcli-smartsheet`
- 若遇到的 `docid` 以 `s3` 开头（形如 `s3_xxxx`）→ 改用 `wecomcli-smartsheet`
- 用户泛指「做表格」、未明确在线表格还是智能表格 → 先用简洁自然语言消歧确认，不得默认本技能


## 接口路由表

> **硬规则**：第二列是 `references/xxx.md` 链接的, 命中这一行后先用 `File(action="read")` 读取对应 references 文件，再构造命令。

| 用户意图 | 参考位置 |
|---|---|
| 新建在线表格 | 见下方「新建在线表格」 |
| 导入本地 CSV / Excel 文件为企微在线表格 | 见下方「导入在线表格」 |
| 读取在线表格基础信息与子表列表 | 见下方「读取在线表格」 |
| 读取在线表格子表数据 | [references/sheet-ranges-get.md](references/sheet-ranges-get.md) |
| 修改在线表格指定区域内容 | [references/sheet-contents-update.md](references/sheet-contents-update.md) |
| 在线表格末尾追加一行数据 | [references/sheet-rows-append.md](references/sheet-rows-append.md) |
| 添加在线表格子工作表 | [references/sheet-subsheets-add.md](references/sheet-subsheets-add.md) |
| 删除在线表格子工作表 | [references/sheet-subsheets-delete.md](references/sheet-subsheets-delete.md) |

## 写操作确认

- 删除子表（`sheet-subsheets-delete`）与 contents update 覆写前，必须向用户复述影响范围并获得明确确认。

## 接口详述

### 新建在线表格

从零新建一篇企微在线表格：空白，或带初始数据（二维表格数据）。**本接口不接受任何文件路径参数**——"用本地文件建/导入"走「导入在线表格」。

#### 命令

```bash
wecom-cli sheet create --json '<JSON 参数>'
```

#### 参数

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `doc_name` | string | 是 | — | 表格标题 |
| `grid_data` | object | 否 | — | 默认子表初始化数据；子结构见下方 |

`grid_data` 对象结构：

| 子字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `start_row` / `start_column` | int | 否 | `0` | 起始行 / 列号，从 0 起 |
| `rows` | array | 否 | — | 各行数据；每项 `values` 为单元格数组 |
| `rows[].values[].cell_value` | object | 否 | — | 单元格值，见下方「cell_value 类型选择」 |
| `rows[].values[].data_type` | string | 否 | — | 枚举：`TEXT` / `NUMBER` / `LINK` / `FORMULA` |

##### cell_value 类型选择

> 硬规则：选择 `cell_value` 形态后，必须同时把同级的 `data_type` 设置为下表对应值。

| 形态 | 对应 `data_type` | 结构 | 适用场景 |
|---|---|---|---|
| `text` | `TEXT` | `{"text": "<纯文本>"}` | 纯文本内容（如姓名、说明、标签、编号字符串等） |
| `number` | `NUMBER` | `{"number": 123.45}` | 数值，用于金额、数量、比率等需要参与公式计算或聚合的数据；值为 JSON 数字类型，不加引号 |
| `formula` | `FORMULA` | `{"formula": "=SUM(A1,A2)"}` | 任何以 `=` 开头的公式，包括 `=SUM(...)`、`=A1+B1`、`=IF(...)`、`=VLOOKUP(...)` 等 |
| `link` | `LINK` | `{"link": {"url": "<URL>", "text": "<显示文本>"}}` | 超链接 |

#### 返回

| 字段 | 类型 | 说明 |
|---|---|---|
| `docid` | string | 新建表格 ID |
| `url` | string | 表格访问链接 |

#### 使用规则

- **何时走 import 而非本接口**：用户提到具体文件路径、或明确说"导入 / 用这个文件建"，一律走「导入在线表格」。

### 导入在线表格

把本地文件（`.csv` / `.xls` / `.xlsx`）导入为企微在线表格。

#### 命令

```bash
wecom-cli sheet import --json '<JSON 参数>'
```

#### 参数

| 字段          | 类型 | 必填 | 默认值 | 语义 |
|-------------|---|---|---|---|
| `file_name` | string | 是 | — | 二进制文件名（含后缀），用于业务判断源文件类型 |
| `file_path` | string | 是 | — | 源文件的本地绝对路径 |
| `passwd`    | string | 否 | — | Office 文件加密密码（若有） |

#### 返回

| 字段 | 类型 | 说明 |
|---|---|---|
| `docid` | string | 导入完成后的表格 ID |
| `url` | string | 导入完成后的访问链接 |
| `task_status` | string | 任务状态枚举，如 `succ` 成功 |

### 读取在线表格

根据 `docid` 读取**在线表格**的基础信息，包括工作表列表、文档名称与访问链接。所有后续 `sheet *` 接口的 `sheet_id` 都从本接口返回的 `sheets[]` 中取。

#### 命令

```bash
wecom-cli sheet get --json '<JSON 参数>'
```

#### 参数

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `docid` | string | 是 | — | 在线表格 ID |

#### 返回

| 字段 | 类型 | 说明 |
|---|---|---|
| `sheets` | array | 工作表列表；每项含 `sheet_id` / `title` / `row_count` / `column_count` / `data_range` 等基础信息 |
| `url` | string | 文档访问链接 |
| `name` | string | 文档名称 |

#### 使用规则

- 拿到 `sheet_id` 后**继续读取子表数据是另一个接口**，命令字符串、参数名、是否分页等都没有在本节出现，**必须**先用 `File(action="read")` 读 `references/sheet-ranges-get.md`，再据此构造命令。

## 跨技能依赖

| 依赖技能 | 典型协作场景                                                | 数据流向 |
|---|-------------------------------------------------------|---|
| `wecomcli-doc-manage` | 用户只给表格名称/关键词，需搜索获取 `docid` 后再读写表格；或需要文件级操作（改名、权限等） | `wecomcli-doc-manage` 的「搜索文档」接口 → 返回 `docid` → 本 skill 的读取/修改/追加接口|

> 必填参数缺失 / `docid` 多候选 / 新建 vs 导入等歧义场景，用简洁自然语言仅追问缺失或有歧义的信息；有候选项时在文字中列出供用户选择，不得自行猜测。

#### `docid` 使用规则

`docid`仅cli使用。
最终展示用户时，不应展示 `docid`，而是使用文档 URL：


```
[doc_name](doc_url)
```


`docid` 是文档的唯一标识符，调用任何文档内容操作技能时均需提供。禁止自造 `docid`，按以下优先级获取：

1. 从文档链接提取（优先）：用户提供了企微文档 URL 时，直接从 URL 中解析。URL 格式为 `https://doc.weixin.qq.com/<type>/<docid>?scode=...`，取 `/<type>/` 后、`?` 前的部分即为 docid。
2. 通过文档搜索获取（备选）：用户仅提供文档名称或关键词、未给链接时，先调用 `wecomcli-doc-manage` 搜索文档，从返回结果中取 `docid`。
3. 用户直接提供：用户明确给出了完整 `docid`，可直接使用，无需再提取或搜索。
