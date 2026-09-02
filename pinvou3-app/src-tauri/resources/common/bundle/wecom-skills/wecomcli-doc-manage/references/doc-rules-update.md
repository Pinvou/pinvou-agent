# 设置文档加入规则 — `wecom-cli doc rules update`

设置通过链接加入文档时的权限规则，包括是否开启成员加入确认、企业内 / 外成员的加入权限。

## 命令

```bash
wecom-cli doc rules update --json '<JSON 参数>'
```

## 参数

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `docid` | string | 是 | — | 目标文档 ID |
| `enable_member_join_admin_check` | boolean | 是 | — | 是否开启成员加入确认 |
| `corp_internal_join_auth` | string | 否 | — | 企业内成员加入权限枚举：`edit` / `read` / `apply`；仅当 `enable_member_join_admin_check=false` 时生效，不传则保持现状 |
| `corp_external_join_auth` | string | 否 | — | 企业外成员加入权限枚举：`edit` / `read` / `apply` / `deny`；仅当 `enable_member_join_admin_check=false` 时生效，不传则保持现状 |

## 返回

设置成功返回空对象。
