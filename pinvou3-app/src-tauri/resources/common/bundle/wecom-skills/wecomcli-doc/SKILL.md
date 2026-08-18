---
name: wecomcli-doc
description: 何时用:仅当用户明确指定 doc/docx/word/在线文档或提供 doc.weixin.qq.com/doc/ 链接时使用;泛指「做个文档/写文档/整理成文档」默认走 wecomcli-smartpage 或本地工具,本技能不得抢占。doc 内容操作:新建、导入、读取、追加、覆盖写入;结构化数据需求改用智能文档/智能表格;公共管理走 wecomcli-doc-manage。
metadata:
  requires:
    bins: ["wecom-cli"]
---

# 企业微信doc文档管理

> 执行任何 `wecom-cli` 命令前，必须先读取并完成 `wecomcli-shared` 技能的公共前置检查。

资源型 skill，负责doc文档（`doc`）的新建、导入与内容读写。

## 适用范围

### 适用

- 新建 / 导入企微 doc 文档
- 读取 doc 文档内容
- 向 doc 文档追加一行 / 覆盖写入doc 文档

### 不适用

- 搜索文档 / 修改文档权限 / 重命名 / 加成员 → 改用 `wecomcli-doc-manage`

### 易混淆场景路由

- 用户说"创建文档 / 写文档 / 整理成文档" 且未指定 doc 类型 → 改用 `wecomcli-smartpage`（智能文档为默认）
- 用户给的链接是 `https://doc.weixin.qq.com/smartpage/...` 或者 `https://page.weixin.qq.com/smartpage/...` → 改用 `wecomcli-smartpage`
- 若遇到的 `docid` 以 `a1` 或者 `b1` 开头（形如 `a1_xxxx`, `b1_xxxx`）→ 改用 `wecomcli-smartpage`

## 接口路由表

路由表第二列若是 `references/xxx.md` 链接 → 必须先用 `read` 工具读完该文件，再构造命令。

| 用户意图 | 参考位置                                                          |
|---|---------------------------------------------------------------|
| 新建doc文档（在线） | 见下方「新建doc文档」                                                  |
| 导入本地文件为企微doc文档 | 见下方「导入doc文档」                                                  |
| 读取doc文档内容 | 见下方「读取doc文档内容」                                                |
| 追加文本到doc文档末尾 | [+contents-append](references/doc-contents-append.md)       |
| 全量覆盖doc文档内容 | [+contents-overwrite](references/doc-contents-overwrite.md) |

### 写入语义裁定（追加 vs 覆盖）

- 默认追加：用户用「写入 / 写到 / 记录 / 补充 / 加进去 / 记一下」等中性动词，且未明确要求清空或替换时，一律走 `append`（追加，不破坏原有内容）。
- 仅显式覆盖：仅当用户明确出现「覆盖 / 重写 / 替换 / 清空重写 / 整个换成」等强语义词时，才走 `overwrite`。

## 接口详述

### 新建doc文档

新建企微doc文档统一走「**生成 `.docx` → 导入**」两步流程：

1. 生成 `.docx` 文件：按 [+doc-create](references/doc-create.md) 生成 `.docx` 文件。
2. 导入为企微doc文档：使用下方「导入doc文档」接口将生成的 `.docx` 文件导入为企微doc文档。注意import导入的时候 `file_name` 应和文档标题保持一致。

### 导入doc文档

把本地文件（`.doc` / `.docx` / `.txt`）导入为企微doc文档。

**命令**

```bash
wecom-cli doc import --json '<JSON 参数>'
```

**参数**

| 字段          | 类型 | 必填 | 默认值 | 语义 |
|-------------|---|---|---|---|
| `doc_type`  | string | 是 | `doc` | 固定为 `doc`（doc文档） |
| `file_name` | string | 是 | — | 二进制文件名（含后缀），用于业务判断源文件类型 |
| `file_path` | string | 是 | — | 源文件的本地绝对路径 |
| `passwd`    | string | 否 | — | Office 文件加密密码（若有） |

**返回**

| 字段 | 类型 | 说明 |
|---|---|---|
| `docid` | string | 导入完成后的文档 ID |
| `url` | string | 导入完成后的访问链接 |
| `task_status` | string | 任务状态枚举，如 `succ` 成功 |

### 读取doc文档内容

读取**doc文档**的文档内容。

**命令**

```bash
wecom-cli doc contents get --json '<JSON 参数>'
```

**参数**

| 字段 | 类型 | 必填 | 默认值 | 语义                                      |
|---|---|----|---|-----------------------------------------|
| `docid` | string | 是  | — | doc文档 ID                                |
| `content_type` | string | 否  | `markdown` | 返回内容格式枚举：`text` / `markdown` / `ooxml`； |

**返回**

| 字段 | 类型 | 说明 |
|---|---|---|
| `url` | string | 文档访问链接 |
| `name` | string | 文档名称 |
| `content` | string | 文档内容较短时直接返回的原文 |
| `file_path` | string | 文档内容较长时自动落盘的**本地文件路径**；需用 `File(action="read")` 读取路径内文本后再展示 |
| `document` | object | `content_type=ooxml` 时返回的文档对象 |
| `version` | int | 文档版本号 |

## 跨技能依赖

| 依赖技能 | 何时触发 | 使用被依赖 skill 做什么                                                                                                             |
|---|---|-----------------------------------------------------------------------------------------------------------------------------|
| `wecomcli-doc-manage` | 用户只给文档名称/关键词，需先拿 `docid` 再读写内容 | 使用 `wecomcli-doc-manage` skill 搜索文档拿 `docid`                                                                                   |
| `wecomcli-smartpage` | 读取doc文档内容后，用户要求"做成智能文档/排版成 smartpage" | 使用 `wecomcli-smartpage` skill 生成智能文档                                                                                           |

> 参数缺失 / `docid` 搜索多候选等歧义场景，用简洁自然语言仅追问缺失或有歧义的信息；有候选项时在文字中列出供用户选择，不得自行猜测。

## `docid` 使用规则

`docid`仅cli使用。
最终展示用户时，不应展示 `docid`，而是使用文档 URL：

```
[doc_name](doc_url)
```


`docid` 是文档的唯一标识符，调用任何文档内容操作技能时均需提供。禁止自造 `docid`，按以下优先级获取：

1. 从文档链接提取（优先）：用户提供了企微文档 URL 时，直接从 URL 中解析。URL 格式为 `https://doc.weixin.qq.com/<type>/<docid>?scode=...`，取 `/<type>/` 后、`?` 前的部分即为 docid。
2. 通过文档搜索获取（备选）：用户仅提供文档名称或关键词、未给链接时，先调用 `wecomcli-doc-manage` 搜索文档，从返回结果中取 `docid`。
3. 用户直接提供：用户明确给出了完整 `docid`，可直接使用，无需再提取或搜索。
