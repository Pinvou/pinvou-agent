# 数据模型：Windows MSG 邮件解析

## EmailAttachment

用户导入的邮件类附件。

| 字段 | 类型 | 说明 | 校验 |
|---|---|---|---|
| `path` | Path | 本地文件路径 | 必须存在且可读 |
| `basename` | String | 展示文件名 | 从路径提取，缺失时使用兜底名 |
| `extension` | String | 扩展名 | `eml` 或 `msg` |
| `byte_size` | u64 | 文件大小 | 用于结果展示和 token/风险判断 |

关系：

- 输入到 `EmailParseRequest`
- 产出 `EmailParseResult`

## EmailParseRequest

邮件解析动作的内部请求。

| 字段 | 类型 | 说明 | 校验 |
|---|---|---|---|
| `attachment` | EmailAttachment | 待解析邮件文件 | 必填 |
| `platform` | PlatformCapability | 当前平台能力 | 必填 |
| `kind` | String | `eml` 或 `msg` | 必须和扩展名一致 |

状态：

```text
created -> parsing -> parsed
created -> parsing -> failed
```

## EmailParseResult

系统给模型和 UI 使用的解析结果。

| 字段 | 类型 | 说明 | 校验 |
|---|---|---|---|
| `kind` | String | 文件类型，保持 `eml` 或 `msg` | 必填 |
| `basename` | String | 文件名 | 必填 |
| `path` | String | 原文件路径字符串 | 必填 |
| `markdown` | Option<String> | 可读邮件文本 | 成功时存在 |
| `warning` | Option<String> | 用户可理解的失败或降级提示 | 失败/降级时存在 |
| `token_estimate` | u32 | 对 `markdown` 的 token 粗估 | 无正文时为 0 |
| `byte_size` | u64 | 原文件大小 | 必填 |

内容格式：

```text
发件人: ...
收件人: ...
抄送: ...
密送: ...
主题: ...
日期: ...

正文:
...

附件: a.pdf, b.docx
```

说明：

- `.eml` 继续使用现有输出字段；`.msg` 应尽量对齐该格式。
- 字段为空时可以省略或输出空值，但不能导致整体失败。
- 附件只要求列出文件名，不递归解析附件内容。

## PlatformCapability

描述不同平台对邮件解析依赖的策略。

| 字段 | 类型 | 说明 |
|---|---|---|
| `os` | Enum | `windows`、`linux`、`unsupported` |
| `msg_native_supported` | bool | 是否可原生解析 `.msg` |
| `eml_supported` | bool | 是否可解析 `.eml` |
| `external_msg_converter_required` | bool | 是否需要外部 `.msg` 转换工具 |
| `dependency_hint` | String | 依赖体检展示的补全提示 |

平台规则：

- Windows：`msg_native_supported = true`，`external_msg_converter_required = false`。
- Linux：保持现有 `msgconvert` 策略。
- Unsupported：返回清晰 warning，不展示 Linux 安装命令。

## DependencyCheckItem

前端依赖体检使用的能力项。

| 字段 | 类型 | 说明 |
|---|---|---|
| `key` | String | 能力键，例如 `email` |
| `installed` | bool | 当前能力是否可用 |
| `apt` | String | Linux 补全包名；Windows 不应包含 Linux 邮件包 |

校验：

- Windows 邮件项不得包含 `libemail-outlook-message-perl`。
- Linux 邮件项可继续包含 `python3 libemail-outlook-message-perl`。
