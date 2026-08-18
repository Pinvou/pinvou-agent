# 添加文档成员 — `wecom-cli doc members update`

向一份文档添加管理员 / 可编辑 / 仅浏览成员，支持批量添加。

## 命令

```bash
wecom-cli doc members update --json '<JSON 参数>'
```

## 参数

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `docid` | string | 是 | — | 目标文档 ID |
| `add_member_list` | object | 是 | — | 要添加的成员列表 |
| `add_member_list.items` | array | 是 | — | 成员数组 |
| `add_member_list.items[].userid` | string | 是 | — | 成员 ID（`user_type=user`） |
| `add_member_list.items[].user_type` | string | 是 | — | 类别枚举：`user` 用户 |
| `add_member_list.items[].user_auth` | string | 是 | — | 权限枚举：`manager` 管理员 / `edit` 可编辑 / `read` 仅浏览 |

## 返回

添加成功返回空对象。
