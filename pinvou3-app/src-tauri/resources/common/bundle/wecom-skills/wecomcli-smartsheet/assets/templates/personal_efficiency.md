# 个人效率的数据表模版

## 包含表格模版

- **待办清单 To-Do List**：个人待办事项管理，记录任务内容、重要紧急程度、提醒时间及完成状态，支持待办总数和任务紧急程度分布统计。
- **月度计划看板**：以周为单位管理月度计划，记录计划详情、类型标签及完成情况，适合个人月度目标的可视化管理。
- **个人待办管理**：精细化个人任务管理，记录任务类型、优先级、预计/实际完成时间，自动计算剩余时间，支持任务完成情况和优先级分析。

## 待办清单 To-Do List

### To-Do

| 字段 | 类型 |
| --- | --- |
| 提醒人 | FIELD_TYPE_USER |
| 重要紧急程度 | FIELD_TYPE_SELECT |
| 是否完成 | FIELD_TYPE_CHECKBOX |
| 备注 | FIELD_TYPE_TEXT |
| 提醒时间 | FIELD_TYPE_DATE_TIME |
| 任务 | FIELD_TYPE_TEXT |

### ✅待办统计（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 重要紧急任务数 | numberCard | [4, 0] | [4, 3] |
| 待办状态 | pie | [6, 3] | [6, 5] |
| 待办总数 | numberCard | [0, 0] | [4, 3] |
| 任务紧急程度 | bar | [0, 3] | [6, 5] |
| 已完成任务数 | numberCard | [8, 0] | [4, 3] |

## 月度计划看板

### 月度计划看板

| 字段 | 类型 |
| --- | --- |
| 计划详情 | FIELD_TYPE_TEXT |
| 是否完成计划 | FIELD_TYPE_CHECKBOX |
| 周 | FIELD_TYPE_SELECT |
| 类型标签 | FIELD_TYPE_SELECT |
| 日期 | FIELD_TYPE_DATE_TIME |

## 个人待办管理

### 个人待办进度表

| 字段 | 类型 |
| --- | --- |
| 已完成 | FIELD_TYPE_CHECKBOX |
| 备注 | FIELD_TYPE_TEXT |
| 实际完成时间 | FIELD_TYPE_DATE_TIME |
| 剩余可用时间 | FIELD_TYPE_FORMULA |
| 预计完成时间 | FIELD_TYPE_DATE_TIME |
| 优先级 | FIELD_TYPE_SELECT |
| 剩余时间情况 | FIELD_TYPE_FORMULA |
| 任务类型 | FIELD_TYPE_SELECT |
| 任务创建时间 | FIELD_TYPE_DATE_TIME |
| 任务内容 | FIELD_TYPE_TEXT |

### 任务进展统计图（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 重要且紧急任务数 | numberCard | [2, 0] | [2, 3] |
| 待办任务总数 | numberCard | [0, 0] | [2, 3] |
| 待办任务优先级柱状图 | stackbar | [4, 0] | [4, 4] |
| 任务完成剩余时间情况 | combo | [8, 0] | [4, 4] |
| 任务类型以及完成情况条形图 | column | [4, 4] | [8, 4] |
| 待办优先级饼图 | pie | [0, 3] | [4, 5] |
