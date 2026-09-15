# drive +version-revert

> **前置条件：** 先阅读 [`../lark-shared/SKILL.md`](../../lark-shared/SKILL.md) 了解认证、全局参数和安全规则。

将文件回滚到指定历史版本。该 shortcut 同时支持 `--as user` 和 `--as bot`；自动化场景推荐使用 `--as bot`。

> [!CAUTION]
> 回滚会用指定历史版本**覆盖当前内容**。该命令在 CLI 中为 write 级，不设 `--yes` 审批门（同族仅 `+version-delete` 为 high-risk-write 需要 `--yes`）；真实执行前先确认目标 `--file-token` 与 `--version` 就是用户要回滚到的版本，并按 lark-drive SKILL.md 高风险写三条件取得用户对具体版本的确认。

## 命令

```bash
lark-cli drive +version-revert \
  --file-token boxcnxxxxxxxx \
  --version 7633658129540910621 \
  --as bot

lark-cli drive +version-revert \
  --file-token boxcnxxxxxxxx \
  --version 7633658129540910621 \
  --as user
```

## 参数

| 参数 | 必填 | 说明 |
|------|------|------|
| `--file-token` | 是 | 目标文件 token |
| `--version` | 是 | `drive +version-history` 返回的长数字 `version` 字段，不是 `tag` |

## 返回值

无额外业务字段，以命令成功 / 失败为准。

## 参考

- [lark-drive](../SKILL.md) -- 云空间（云盘/云存储）全部命令
- [lark-shared](../../lark-shared/SKILL.md) -- 认证和全局参数
