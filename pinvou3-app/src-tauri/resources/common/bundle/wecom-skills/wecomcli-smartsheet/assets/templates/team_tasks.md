# 团队任务的数据表模版

## 包含表格模版

- **工作计划表**：团队工作计划管理模版，记录任务描述、负责人、优先级及完成情况，支持任务状态看板和负责人工作量统计。
- **待办清单**：轻量级待办事项管理，记录任务类型、优先级、截止时间及完成状态，支持待办关键词词云和分工完成情况统计。
- **工作计划表（多视图）**：支持多视图展示的工作计划模版，记录工作事项、责任人、进度及部门，并统计各部门未完成事项。
- **团队周会**：用于记录团队周会内容，包含本周工作进度、存在问题、下周计划及所需支持，方便会议记录归档。
- **季度任务拆解**：将季度目标拆解为具体任务，记录优先级、负责人、工作进度及难点，支持季度任务总览和预计完成时间趋势分析。
- **运营工作计划**：面向连锁门店运营团队，管理大区和门店的季度运营重点、销售目标及专项任务，支持各大区目标销售额对比。
- **工作量统计**：统计员工值班工时，记录值班地点、开始/结束时间及工时，支持月度值班时长排名和各仓库值班情况分析。
- **任务管理**：通用任务管理模版，记录任务描述、负责人、状态及截止时间，支持任务状态分布和逾期任务预警。
- **日报**：简洁的日报提交模版，记录日报内容和进度，统计今日提交日报人数，适合团队日常工作汇报。

## 工作计划表

### 工作计划表

| 字段 | 类型 |
| --- | --- |
| 项目进度 | FIELD_TYPE_PROGRESS |
| 开始日期 | FIELD_TYPE_DATE_TIME |
| 讨论群 | FIELD_TYPE_WWGROUP |
| 实际完成日期 | FIELD_TYPE_DATE_TIME |
| 任务状态 | FIELD_TYPE_SELECT |
| 项目进展描述 | FIELD_TYPE_TEXT |
| 是否按时交付 | FIELD_TYPE_FORMULA |
| 任务负责人 | FIELD_TYPE_USER |
| 预计所需天数 | FIELD_TYPE_FORMULA |
| 预计完成日期 | FIELD_TYPE_DATE_TIME |
| 紧急重要度 | FIELD_TYPE_SELECT |
| 任务描述 | FIELD_TYPE_TEXT |

### 任务看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 项目进度表 | column | [4, 3] | [4, 4] |
| 负责人看板 | bar | [8, 3] | [4, 4] |
| 已完成任务数 | numberCard | [6, 0] | [3, 3] |
| 未完成任务数 | numberCard | [3, 0] | [3, 3] |
| 项目状态一览 | pie | [9, 0] | [3, 3] |
| 优先级分布 | stackbar | [0, 3] | [4, 4] |
| 任务总数 | numberCard | [0, 0] | [3, 3] |

## 待办清单

### 待办清单

| 字段 | 类型 |
| --- | --- |
| 是否完成 | FIELD_TYPE_CHECKBOX |
| 跟进备注 | FIELD_TYPE_TEXT |
| 负责人 | FIELD_TYPE_USER |
| 截止时间 | FIELD_TYPE_DATE_TIME |
| 优先级 | FIELD_TYPE_SELECT |
| 剩余时间情况 | FIELD_TYPE_FORMULA |
| 任务类型 | FIELD_TYPE_SELECT |
| 创建时间 | FIELD_TYPE_CREATED_TIME |
| 待办事项 | FIELD_TYPE_TEXT |

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 任务类型及完成情况 | stackcolumn | [8, 4] | [4, 4] |
| 分工及完成情况 | stackbar | [0, 4] | [4, 4] |
| 待办事项关键词 | wordCloud | [4, 4] | [4, 4] |
| 高优未完成数量 | numberCard | [6, 1] | [3, 3] |
| 已完成数量 | numberCard | [9, 1] | [3, 3] |
| 待办总数 | numberCard | [0, 1] | [3, 3] |
| 未完成数量 | numberCard | [3, 1] | [3, 3] |

## 工作计划表（多视图）

### 工作计划仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 已完成 | numberCard | [0, 0] | [3, 3] |
| 计划状态分布 | pie | [0, 3] | [6, 5] |
| 未开展/延期 | numberCard | [6, 0] | [3, 3] |
| 已暂停 | numberCard | [9, 0] | [3, 3] |
| 进行中 | numberCard | [3, 0] | [3, 3] |
| 各部门未完成事项记录 | table | [6, 3] | [6, 5] |

### 工作计划表

| 字段 | 类型 |
| --- | --- |
| 开始时间 | FIELD_TYPE_DATE_TIME |
| 工作目标 | FIELD_TYPE_TEXT |
| 进展状态 | FIELD_TYPE_SELECT |
| 责任人 | FIELD_TYPE_USER |
| 进度 | FIELD_TYPE_PROGRESS |
| 计划完成时间 | FIELD_TYPE_DATE_TIME |
| 部门 | FIELD_TYPE_SELECT |
| 工作进展描述 | FIELD_TYPE_TEXT |
| 困难及需要支持 | FIELD_TYPE_TEXT |
| 优先级 | FIELD_TYPE_SELECT |
| 工作事项 | FIELD_TYPE_TEXT |

## 团队周会

### 智能表1

| 字段 | 类型 |
| --- | --- |
| 周会日期 | FIELD_TYPE_DATE_TIME |
| 下周计划 | FIELD_TYPE_TEXT |
| 所属项目 | FIELD_TYPE_SELECT |
| 需要的支持 | FIELD_TYPE_TEXT |
| 汇报人 | FIELD_TYPE_USER |
| 本周工作进度 | FIELD_TYPE_PROGRESS |
| 存在的问题/风险 | FIELD_TYPE_TEXT |
| 汇报主题 | FIELD_TYPE_FORMULA |

## 季度任务拆解

### 季度任务仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 项目进度 | column | [0, 3] | [12, 4] |
| Q2高优任务数 | numberCard | [3, 0] | [2, 3] |
| Q2任务总数 | numberCard | [0, 0] | [3, 3] |
| 项目预计完成时间 | smoothline | [0, 7] | [12, 3] |
| 优先级分布 | stackbar | [7, 0] | [5, 3] |
| 已完成任务数 | numberCard | [5, 0] | [2, 3] |

### 季度任务

| 字段 | 类型 |
| --- | --- |
| 完成时间 | FIELD_TYPE_DATE_TIME |
| 优先级 | FIELD_TYPE_SELECT |
| 讨论群 | FIELD_TYPE_WWGROUP |
| 工作进度 | FIELD_TYPE_PROGRESS |
| 工作进展 | FIELD_TYPE_TEXT |
| 开始时间 | FIELD_TYPE_DATE_TIME |
| 负责人 | FIELD_TYPE_USER |
| Q2目标（按月度管理） | FIELD_TYPE_TEXT |
| 状态 | FIELD_TYPE_SELECT |
| 难度与解决方案 | FIELD_TYPE_TEXT |
| 第X季度工作任务 | FIELD_TYPE_TEXT |

## 运营工作计划

### 【大区】运营重点

| 字段 | 类型 |
| --- | --- |
| 大区负责人 | FIELD_TYPE_LOOKUP |
| 客群增长目标 | FIELD_TYPE_PROGRESS |
| 涉及店长 | FIELD_TYPE_LOOKUP |
| 三季度运营重点 | FIELD_TYPE_LOOKUP |
| 运营指导文件 | FIELD_TYPE_ATTACHMENT |
| 涉及门店 | FIELD_TYPE_REFERENCE |
| 大区沟通群 | FIELD_TYPE_WWGROUP |
| 三季度销售目标 (元) | FIELD_TYPE_LOOKUP |
| 大区名称 | FIELD_TYPE_SELECT |

### 【门店】运营重点

| 字段 | 类型 |
| --- | --- |
| 预计完成时间 | FIELD_TYPE_DATE_TIME |
| 联系电话 | FIELD_TYPE_TEXT |
| 门店名称 | FIELD_TYPE_TEXT |
| 经营状态 | FIELD_TYPE_SELECT |
| 三季度目标销售额 | FIELD_TYPE_NUMBER |
| 三季度运营重点 | FIELD_TYPE_SELECT |
| 预计开始时间 | FIELD_TYPE_DATE_TIME |
| 城市 | FIELD_TYPE_SELECT |
| 门店地址 | FIELD_TYPE_LOCATION |
| 专项任务2 | FIELD_TYPE_TEXT |
| 店长 | FIELD_TYPE_USER |
| 所属片区 | FIELD_TYPE_SELECT |
| 专项任务1 | FIELD_TYPE_TEXT |
| 开业日期 | FIELD_TYPE_DATE_TIME |
| 大区负责人 | FIELD_TYPE_USER |

### 三季度运营重点看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 三季度目标销售额 | numberCard | [0, 0] | [4, 4] |
| 各门店三季度目标销售额 | bar | [0, 4] | [6, 4] |
| 各大区客群增长目标 | bar | [6, 4] | [6, 4] |
| 各门店三季度运营目标 | bar | [8, 0] | [4, 4] |
| 各大区三季度目标销售额 | doughnut | [4, 0] | [4, 4] |

## 工作量统计

### 汇总仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 3月值班时长 | numberCard | [4, 4] | [2, 3] |
| 3月值班时长 | numberCard | [10, 4] | [2, 3] |
| 3月值班时长 | numberCard | [0, 4] | [2, 3] |
| 3月值班人员表 | bar | [8, 1] | [4, 3] |
| 3月值班时长 | numberCard | [6, 4] | [2, 3] |
| 3月值班时长 | numberCard | [2, 4] | [2, 3] |
| 3月各仓库值班时长 | bar | [0, 7] | [6, 3] |
| 3月总值班时长 | numberCard | [0, 1] | [4, 3] |
| 3月总值班次数 | numberCard | [4, 1] | [4, 3] |
| 3月值班时长 | numberCard | [8, 4] | [2, 3] |
| 3月值班时长排名 | bar | [6, 7] | [6, 3] |

### 工时明细

| 字段 | 类型 |
| --- | --- |
| 值班地点 | FIELD_TYPE_SELECT |
| 值班时长（分钟） | FIELD_TYPE_FORMULA |
| 值班日期 | FIELD_TYPE_DATE_TIME |
| 值班结束时间 | FIELD_TYPE_DATE_TIME |
| 值班人员 | FIELD_TYPE_USER |
| 值班工时 | FIELD_TYPE_FORMULA |
| 工号 | FIELD_TYPE_TEXT |
| 值班开始时间 | FIELD_TYPE_DATE_TIME |

### 工时计算

| 字段 | 类型 |
| --- | --- |
| 所属片区 | FIELD_TYPE_SELECT |
| 值班人员 | FIELD_TYPE_USER |
| 3月值班次数 | FIELD_TYPE_LOOKUP |
| 工号 | FIELD_TYPE_TEXT |
| 3月值班时长 | FIELD_TYPE_LOOKUP |

## 任务管理

### 任务仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 任务总数 | numberCard | [0, 1] | [3, 3] |
| 任务状态分布 | doughnut | [0, 4] | [6, 5] |
| 逾期任务数 | numberCard | [6, 1] | [3, 3] |
| 任务分工 | stackbar | [6, 4] | [6, 5] |
| 进行中任务数 | numberCard | [3, 1] | [3, 3] |
| 已完成任务数 | numberCard | [9, 1] | [3, 3] |

### 任务列表

| 字段 | 类型 |
| --- | --- |
| 任务描述 | FIELD_TYPE_TEXT |
| 预计完成时间 | FIELD_TYPE_DATE_TIME |
| 倒数日 | FIELD_TYPE_FORMULA |
| 状态 | FIELD_TYPE_SELECT |
| 负责人 | FIELD_TYPE_USER |
| 备注 | FIELD_TYPE_TEXT |
| 开始时间 | FIELD_TYPE_DATE_TIME |

## 日报

### 日报

| 字段 | 类型 |
| --- | --- |
| 提交时间 | FIELD_TYPE_DATE_TIME |
| 提交人 | FIELD_TYPE_CREATED_USER |
| 进度 | FIELD_TYPE_PROGRESS |
| 日报内容 | FIELD_TYPE_TEXT |

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 今日提交日报人数 | numberCard | [0, 0] | [2, 2] |
