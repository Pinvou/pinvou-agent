# 管理产品研发各个流程-研发的数据表模版

## 包含表格模版

- **项目研发流程图**：以流程图形式管理研发各阶段进展，记录每个研发流程的责任部门、参与部门、开始/完成时间及成果，并统计各阶段所需周期。
- **走查问题跟进**：用于记录和跟踪产品走查中发现的问题，支持按问题类型、优先级、进展状态分类管理，并通过仪表盘展示待修复和已修复数量趋势。
- **BUG跟进表**：专为研发团队设计的 BUG 管理模版，记录 BUG 类型、等级、所属功能、提出人及修复版本，支持 BUG 状态看板和类型分布分析。

## 项目研发流程图

### 研发流程看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各研发阶段所需周期 (天) | stackbar | [3, 0] | [5, 5] |
| 各部门参与周期 (天) | doughnut | [8, 0] | [4, 5] |

### 项目研发流程

| 字段 | 类型 |
| --- | --- |
| 开始时间 | FIELD_TYPE_DATE_TIME |
| 创建人 | FIELD_TYPE_CREATED_USER |
| 成果 | FIELD_TYPE_TEXT |
| 责任部门 | FIELD_TYPE_SELECT |
| 研发阶段 | FIELD_TYPE_SELECT |
| 周期 | FIELD_TYPE_FORMULA |
| 完成时间 | FIELD_TYPE_DATE_TIME |
| 责任人 | FIELD_TYPE_USER |
| 参与部门 | FIELD_TYPE_SELECT |
| 研发流程 | FIELD_TYPE_TEXT |

## 走查问题跟进

### 跟进统计（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各类问题占比 | pie | [4, 3] | [4, 5] |
| 总走查数 | numberCard | [0, 0] | [4, 3] |
| 走查提出时间 | line | [8, 3] | [4, 5] |
| 待修复 | numberCard | [4, 0] | [4, 3] |
| 已修复 | numberCard | [8, 0] | [4, 3] |
| 处理状态 | bar | [0, 3] | [4, 5] |

### 走查问题

| 字段 | 类型 |
| --- | --- |
| 备注 | FIELD_TYPE_TEXT |
| 反馈日期 | FIELD_TYPE_DATE_TIME |
| 讨论群 | FIELD_TYPE_WWGROUP |
| 预计/实际修复日期 | FIELD_TYPE_DATE_TIME |
| 问题类型 | FIELD_TYPE_SELECT |
| 进展状态 | FIELD_TYPE_SELECT |
| 优先级 | FIELD_TYPE_SELECT |
| 反馈人 | FIELD_TYPE_USER |
| 跟进人 | FIELD_TYPE_USER |
| 问题描述 | FIELD_TYPE_TEXT |

## BUG跟进表

### BUG跟进明细

| 字段 | 类型 |
| --- | --- |
| BUG编号 | FIELD_TYPE_AUTONUMBER |
| BUG类型 | FIELD_TYPE_SELECT |
| 所属功能 | FIELD_TYPE_TEXT |
| BUG描述 | FIELD_TYPE_TEXT |
| BUG等级 | FIELD_TYPE_SELECT |
| 设备 | FIELD_TYPE_SELECT |
| 提出人 | FIELD_TYPE_USER |
| 设备系统 | FIELD_TYPE_SELECT |
| 跟进人 | FIELD_TYPE_USER |
| 状态 | FIELD_TYPE_SELECT |
| 提出时间 | FIELD_TYPE_DATE_TIME |
| 预计修复时间 | FIELD_TYPE_DATE_TIME |
| 修复版本 | FIELD_TYPE_SELECT |
| BUG截图 | FIELD_TYPE_IMAGE |
| 备注 | FIELD_TYPE_TEXT |

### BUG跟进看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| BUG来源分布 | column | [4, 3] | [4, 4] |
| BUG等级分布 | bar | [8, 3] | [4, 4] |
| BUG类型分布 | bar | [0, 3] | [4, 4] |
| BUG总数 | numberCard | [0, 0] | [4, 3] |
| BUG状态一览 | doughnut | [8, 0] | [4, 3] |
| 待修复BUG数 | numberCard | [4, 0] | [4, 3] |
