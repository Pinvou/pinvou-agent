# 工作汇报的数据表模版

## 包含表格模版

- **日报周报表**：同时管理日报和周报，记录工作总结、下一步计划及所属项目，支持日报/周报提交数量统计和项目分布分析。
- **团队日报汇总**：汇总团队成员日报，记录今日工作总结、明日计划及困难反馈，支持团队日报提交情况统计和项目任务分布。
- **工作周报简表**：简洁的周报提交模版，记录本周工作总结、下周计划及附件，支持各员工累计提交数和每周提交趋势统计。

## 日报周报表

### 日报

| 字段 | 类型 |
| --- | --- |
| 负责人 | FIELD_TYPE_USER |
| 汇报时间 | FIELD_TYPE_DATE_TIME |
| 相关资料 | FIELD_TYPE_ATTACHMENT |
| 下一步计划 | FIELD_TYPE_TEXT |
| 所属项目 | FIELD_TYPE_SELECT |
| 今日工作总结 | FIELD_TYPE_TEXT |

### 周报

| 字段 | 类型 |
| --- | --- |
| 负责人 | FIELD_TYPE_USER |
| 汇报时间 | FIELD_TYPE_DATE_TIME |
| 相关资料 | FIELD_TYPE_ATTACHMENT |
| 所属项目 | FIELD_TYPE_SELECT |
| 周报内容 | FIELD_TYPE_TEXT |

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 日报提交总数 | numberCard | [4, 0] | [4, 3] |
| 今日提交周报数 | numberCard | [0, 3] | [4, 3] |
| 日报所属项目分布 | column | [8, 0] | [4, 3] |
| 周报所属项目分布 | column | [8, 3] | [4, 3] |
| 周报提交总数 | numberCard | [4, 3] | [4, 3] |
| 今日提交日报数 | numberCard | [0, 0] | [4, 3] |

## 团队日报汇总

### 日报情况统计（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 项目任务数 | pie | [0, 4] | [12, 4] |
| 今日日报总数 | numberCard | [0, 1] | [6, 3] |
| 团队日报情况 | stackbar | [6, 1] | [6, 3] |

### 团队日报汇总

| 字段 | 类型 |
| --- | --- |
| 汇报给 | FIELD_TYPE_USER |
| 今日工作总结 | FIELD_TYPE_TEXT |
| 困难及需要的支持 | FIELD_TYPE_TEXT |
| 是否涉及多部门合作 | FIELD_TYPE_CHECKBOX |
| 项目 | FIELD_TYPE_SELECT |
| 提交人 | FIELD_TYPE_SELECT |
| 明日工作计划 | FIELD_TYPE_TEXT |
| 附件 | FIELD_TYPE_ATTACHMENT |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 日报提交日期 | FIELD_TYPE_DATE_TIME |

### 团队成员管理

| 字段 | 类型 |
| --- | --- |
| 资料创建人 | FIELD_TYPE_SELECT |
| 是否提交今日月报 | FIELD_TYPE_FORMULA |
| 部门 | FIELD_TYPE_SELECT |
| 最近修改时间 | FIELD_TYPE_DATE_TIME |
| 是否提交日报 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 工号 | FIELD_TYPE_NUMBER |
| 备注 | FIELD_TYPE_TEXT |

## 工作周报简表

### 周报简表

| 字段 | 类型 |
| --- | --- |
| 提交人 | FIELD_TYPE_USER |
| 其他事项 | FIELD_TYPE_TEXT |
| 群聊 | FIELD_TYPE_WWGROUP |
| 下周工作计划 | FIELD_TYPE_TEXT |
| 提交时间 | FIELD_TYPE_DATE_TIME |
| 附件 | FIELD_TYPE_ATTACHMENT |
| 本周工作总结 | FIELD_TYPE_TEXT |

### 周报仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 每周提交数 | stackbar | [3, 4] | [9, 5] |
| 累计提交总数 | numberCard | [0, 0] | [3, 4] |
| 各员工累计提交数 | column | [3, 0] | [9, 4] |
