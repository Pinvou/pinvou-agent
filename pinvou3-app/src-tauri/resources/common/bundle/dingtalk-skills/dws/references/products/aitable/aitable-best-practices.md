# AI 表格最佳实践

## 1. 字段可写性分类

| 字段类型 | 可写 | 正确方式 |
|----------|------|----------|
| 文本/数字/日期/单选/多选/复选框/URL | ✅ | record create/update |
| 附件 | ⚠️ | 必须先走 [attachment upload 流程](./aitable-attachment.md) |
| 创建人/修改人/创建时间/修改时间 | ❌ | 系统字段，只读 |
| 公式/查找引用 | ❌ | 只读，由系统计算 |
| AI 字段 | ❌ | 只读，由 AI 自动计算 |

## 2. 查询执行契约与任务选路

四条要点：优先 `--filters` 在服务端过滤，不要拉全量后在 context 里手动统计；`has_more=true` 时数据可能不完整，禁止下全局结论；字段名（fieldId）必须来自 `table get` 真实返回，不要猜测；全量导出为文件走 `export data`（脚本 `aitable_export_via_task.py`），不要 `--all` 拉全量再写文件。查询细则（`--all`/`--page-limit`/filters 写法）见 [aitable-record-query.md](./aitable-record-query.md)，分析任务选路见 [aitable-data-analysis-sop.md](./aitable-data-analysis-sop.md)。

## 4. 创建/修改后回读确认

执行写操作后，建议立即回读确认结果：

| 写操作 | 建议回读命令 | 确认内容 |
|--------|-------------|----------|
| `table create` | `table get --table-ids <新tableId>` | 表名、字段列表是否符合预期 |
| `field create` | `table get --table-ids <tableId>` | 新字段是否出现在字段列表中 |
| `record create/update` | `record query --record-ids <新recordId>` | 写入值是否正确 |

## 5. AI 字段注意事项

- AI 字段的 prompt **必须至少包含一个 `fieldRef` 引用**，纯文本 prompt 会被后端拒绝
- 先创建/确认被引用字段的 fieldId，再在 prompt 中引用
- `outputType` 必须与字段类型一致（如 `outputType=text` 配 `--type text`）
