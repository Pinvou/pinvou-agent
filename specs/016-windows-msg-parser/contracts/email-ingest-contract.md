# 契约：邮件附件导入

## 范围

本契约描述 `file_ingest` 对 `.eml` 与 `.msg` 邮件文件的用户可见行为。它不是外部 HTTP API，而是 Tauri 后端附件解析命令返回给前端和模型上下文的内部契约。

## 输入

| 输入 | 要求 |
|---|---|
| 文件路径 | 指向用户选择或上传的本地文件 |
| 扩展名 | `.eml` 或 `.msg` |
| 平台 | Windows、Linux 或其他平台 |

## 输出

成功时：

```json
{
  "kind": "msg",
  "basename": "sample.msg",
  "path": "C:\\Users\\...\\sample.msg",
  "markdown": "发件人: ...\n收件人: ...\n主题: ...\n日期: ...\n\n正文:\n...\n\n附件: a.pdf",
  "token_estimate": 123,
  "byte_size": 45678,
  "warning": null
}
```

失败或降级时：

```json
{
  "kind": "msg",
  "basename": "broken.msg",
  "path": "C:\\Users\\...\\broken.msg",
  "markdown": null,
  "token_estimate": 0,
  "byte_size": 45678,
  "warning": ".msg 解析失败: 文件不是有效 Outlook MSG 格式"
}
```

## Windows `.msg` 行为

- MUST 不调用 `msgconvert`。
- MUST 不要求安装 Perl 或 `libemail-outlook-message-perl`。
- MUST 尽量提取：发件人、收件人、抄送、密送、主题、日期、正文、附件名。
- SHOULD 优先使用纯文本正文；纯文本为空时使用 HTML 或可读替代正文。
- SHOULD 保留中文内容和附件名。
- MUST 在解析失败时返回 warning，不崩溃。

## `.eml` 行为

- MUST 保持现有输出字段和顺序。
- MUST 继续优先纯文本正文，回退 HTML 正文。
- MUST 继续只列出附件名，不递归解析附件内容。

## Linux `.msg` 行为

- SHOULD 保持现有 `msgconvert` 转 `.eml` 再解析的路径。
- 缺少依赖时 MAY 继续提示 `sudo apt install libemail-outlook-message-perl`。

## 验收用例

1. Windows，无 Perl/msgconvert，导入有效 `.msg`：返回 `markdown`，`warning = null`。
2. Windows，导入损坏 `.msg`：`markdown = null`，`warning` 包含明确原因。
3. Windows，导入 `.eml`：输出与旧版本一致。
4. Linux，缺少 `msgconvert` 导入 `.msg`：继续返回 Linux 安装提示。
