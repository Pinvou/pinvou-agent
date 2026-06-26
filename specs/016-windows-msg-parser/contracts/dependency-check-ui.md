# 契约：邮件依赖体检

## 范围

本契约描述设置页“依赖体检”中邮件项的跨平台展示要求。

## 能力项

| key | 展示含义 |
|---|---|
| `email` | 邮件（.eml / .msg）解析能力 |

## Windows 行为

- MUST 不展示 `libemail-outlook-message-perl`。
- MUST 不展示 `sudo apt install ...`。
- MUST 不要求 `msgconvert` 才判定 `.msg` 可用。
- SHOULD 将 Windows 原生 `.msg` 解析能力视为内置能力。
- 如果 `.eml` 仍依赖现有 Python 路径，缺失提示不得混淆为 `.msg` 依赖缺失。

示例：

```json
{
  "key": "email",
  "installed": true,
  "apt": ""
}
```

## Linux 行为

- SHOULD 保持现有安装提示：

```json
{
  "key": "email",
  "installed": false,
  "apt": "python3 libemail-outlook-message-perl"
}
```

## 不支持平台

- SHOULD 返回可理解的能力不可用状态。
- MUST 不展示 Linux 专用命令，除非该平台明确使用 Linux 包管理器。

## 验收检查

- Windows 依赖体检文本中搜索不到 `libemail-outlook-message-perl`。
- Windows 依赖体检文本中搜索不到 `msgconvert`。
- Linux 依赖体检仍能给出 `python3 libemail-outlook-message-perl`。
