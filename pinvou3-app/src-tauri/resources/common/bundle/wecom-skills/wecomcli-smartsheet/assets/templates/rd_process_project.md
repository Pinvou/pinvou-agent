# 管理产品研发各个流程-项目的数据表模版

## 包含表格模版

- **项目管理**：面向研发团队的项目任务管理模版，支持记录项目任务的负责人、状态、截止时间，并通过仪表盘展示项目总数、进行中、已完成及逾期情况。
- **产品功能需求池**：用于管理产品功能需求的全生命周期，记录需求描述、优先级、负责人及开发状态，并通过词云图和任务分工图直观呈现需求分布。
- **需求收集表单**：通过表单收集内外部需求，自动汇总需求状态、优先级分布及负责人分工，适合产品团队快速收集和评估用户反馈。
- **人力甘特图**：以甘特图视角管理研发人力资源，记录每位研发人员的任务分配、开始/结束日期及状态，支持人力状态统计和需求总览。

## 项目管理

### 进展统计（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 逾期项目数 | numberCard | [6, 0] | [3, 3] |
| 项目负责人分布 | stackbar | [6, 3] | [6, 5] |
| 进行中项目数 | numberCard | [3, 0] | [3, 3] |
| 已完成项目数 | numberCard | [9, 0] | [3, 3] |
| 项目总数 | numberCard | [0, 0] | [3, 3] |
| 项目状态分布 | doughnut | [0, 3] | [6, 5] |

### 项目任务

| 字段 | 类型 |
| --- | --- |
| 项目 | FIELD_TYPE_TEXT |
| 自动编号 | FIELD_TYPE_AUTONUMBER |
| 预计完成时间 | FIELD_TYPE_DATE_TIME |
| 倒数日 | FIELD_TYPE_FORMULA |
| 状态 | FIELD_TYPE_SELECT |
| 负责人 | FIELD_TYPE_USER |
| 备注 | FIELD_TYPE_TEXT |
| 开始时间 | FIELD_TYPE_DATE_TIME |

## 产品功能需求池

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 任务分工 | stackbar | [6, 3] | [6, 5] |
| 已评估需求数 | numberCard | [3, 0] | [3, 3] |
| 开发中需求数 | numberCard | [6, 0] | [3, 3] |
| 需求总数 | numberCard | [0, 0] | [3, 3] |
| 用户需求词云图 | wordCloud | [0, 3] | [6, 5] |
| 暂不考虑 | numberCard | [9, 0] | [3, 3] |

### 需求池

| 字段 | 类型 |
| --- | --- |
| 负责人总数 | FIELD_TYPE_FORMULA |
| 功能名称 | FIELD_TYPE_TEXT |
| 需求分类 | FIELD_TYPE_SELECT |
| 优先级 | FIELD_TYPE_SELECT |
| 结束时间 | FIELD_TYPE_DATE_TIME |
| 预计交付时间 | FIELD_TYPE_DATE_TIME |
| 需求状态 | FIELD_TYPE_SELECT |
| 负责人 | FIELD_TYPE_USER |
| 需求描述 | FIELD_TYPE_TEXT |
| 提出时间 | FIELD_TYPE_CREATED_TIME |

## 需求收集表单

### 需求收集

| 字段 | 类型 |
| --- | --- |
| 功能名称 | FIELD_TYPE_TEXT |
| 需求提出人 | FIELD_TYPE_CREATED_USER |
| 优先级 | FIELD_TYPE_SELECT |
| 需求状态 | FIELD_TYPE_SELECT |
| 需求负责人 | FIELD_TYPE_USER |
| 需求描述 | FIELD_TYPE_TEXT |
| 提出时间 | FIELD_TYPE_CREATED_TIME |

### 需求统计仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 需求词云图 | wordCloud | [0, 3] | [6, 5] |
| 暂不考虑 | numberCard | [9, 0] | [3, 3] |
| 需求评估分工 | stackbar | [6, 3] | [6, 5] |
| 已评估需求数 | numberCard | [3, 0] | [3, 3] |
| 开发中需求数 | numberCard | [6, 0] | [3, 3] |
| 需求总数 | numberCard | [0, 0] | [3, 3] |

## 人力甘特图

### 人力表

| 字段 | 类型 |
| --- | --- |
| 研发人员 | FIELD_TYPE_USER |
| 开始日期 | FIELD_TYPE_DATE_TIME |
| 结束日期 | FIELD_TYPE_DATE_TIME |
| 优先级 | FIELD_TYPE_SELECT |
| 状态 | FIELD_TYPE_SELECT |
| 需求 | FIELD_TYPE_TEXT |

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 总人力数 | numberCard | [9, 5] | [3, 3] |
| 人力状态统计 | doughnut | [0, 0] | [6, 5] |
| 需求总览（按优先级） | bar | [6, 0] | [6, 5] |
| 历史需求 | table | [0, 5] | [9, 3] |
