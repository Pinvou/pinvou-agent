# 管理产品研发各个流程-运维的数据表模版

## 包含表格模版

- **运维问题跟进**：用于收集和跟踪运维工单，记录问题类型、关联系统、紧急程度及处理进度，支持本月工单统计和高频问题词云分析。
- **设备管理台账**：管理企业设备的全生命周期，记录设备采购、领用、维保信息，并通过仪表盘展示设备状态分布、维保开支及成本情况。

## 运维问题跟进

### 运维问题收集

| 字段 | 类型 |
| --- | --- |
| 反馈人 | FIELD_TYPE_USER |
| 反馈日期 | FIELD_TYPE_DATE_TIME |
| 问题截图 | FIELD_TYPE_IMAGE |
| 关联系统 | FIELD_TYPE_SELECT |
| 处理用时 | FIELD_TYPE_FORMULA |
| 问题处理进度 | FIELD_TYPE_FORMULA |
| 跟进人 | FIELD_TYPE_USER |
| 问题类型 | FIELD_TYPE_SELECT |
| 工单状态 | FIELD_TYPE_SELECT |
| 跟进备注 | FIELD_TYPE_TEXT |
| 解决日期 | FIELD_TYPE_DATE_TIME |
| 工单编号 | FIELD_TYPE_AUTONUMBER |
| 紧急程度 | FIELD_TYPE_SELECT |
| 问题描述 | FIELD_TYPE_TEXT |

### 本月问题看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本月工单处理状态 | bar | [0, 3] | [4, 3] |
| 本月工单跟进情况（按人） | bar | [9, 3] | [3, 3] |
| 本月各类问题占比 | doughnut | [4, 0] | [5, 6] |
| 本月待处理工单数 | numberCard | [2, 0] | [2, 3] |
| 本月工单总数 | numberCard | [0, 0] | [2, 3] |
| 本月高频问题（词云） | wordCloud | [9, 0] | [3, 3] |
| 待处理工单数 | numberCard | [2, 0] | [2, 3] |
| 工单总数 | numberCard | [0, 0] | [2, 3] |
| 高频问题（词云） | wordCloud | [9, 0] | [3, 3] |
| 工单处理状态 | bar | [0, 3] | [4, 3] |
| （按人）工单跟进情况 | bar | [9, 3] | [3, 3] |
| 各类问题占比 | doughnut | [4, 0] | [5, 6] |

### 问题总看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本月工单跟进情况（按人） | bar | [9, 3] | [3, 3] |
| 本月各类问题占比 | doughnut | [4, 0] | [5, 6] |
| 本月待处理工单数 | numberCard | [2, 0] | [2, 3] |
| 本月工单总数 | numberCard | [0, 0] | [2, 3] |
| 本月高频问题（词云） | wordCloud | [9, 0] | [3, 3] |
| 本月工单处理状态 | bar | [0, 3] | [4, 3] |
| 各类问题占比 | doughnut | [4, 0] | [5, 6] |
| 待处理工单数 | numberCard | [2, 0] | [2, 3] |
| 工单总数 | numberCard | [0, 0] | [2, 3] |
| 高频问题（词云） | wordCloud | [9, 0] | [3, 3] |
| 工单处理状态 | bar | [0, 3] | [4, 3] |
| （按人）工单跟进情况 | bar | [9, 3] | [3, 3] |

## 设备管理台账

### 设备明细表

| 字段 | 类型 |
| --- | --- |
| 设备负责人 | FIELD_TYPE_USER |
| 采购日期 | FIELD_TYPE_DATE_TIME |
| 领用人员 | FIELD_TYPE_USER |
| 维修金额 | FIELD_TYPE_FORMULA |
| 保修到期日 | FIELD_TYPE_DATE_TIME |
| 设备状态 | FIELD_TYPE_SELECT |
| 设备编号 | FIELD_TYPE_BARCODE |
| 采购金额 | FIELD_TYPE_CURRENCY |
| 设备类别 | FIELD_TYPE_SELECT |
| 维保记录 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 设备名称 | FIELD_TYPE_TEXT |

### 维保记录表

| 字段 | 类型 |
| --- | --- |
| 设备编号 | FIELD_TYPE_LOOKUP |
| 维护类型 | FIELD_TYPE_SELECT |
| 维护费用 | FIELD_TYPE_CURRENCY |
| 维护人 | FIELD_TYPE_USER |
| 维护日期 | FIELD_TYPE_DATE_TIME |
| 维护内容 | FIELD_TYPE_TEXT |
| 维护结果 | FIELD_TYPE_TEXT |
| 关联设备 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 设备管理看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 维护开支分布 | bar | [8, 3] | [4, 3] |
| 使用中设备数 | numberCard | [2, 0] | [2, 3] |
| 设备成本情况 | bar | [4, 3] | [4, 3] |
| 设备类型分布 | pie | [0, 3] | [4, 3] |
| 维修中设备数 | numberCard | [4, 0] | [2, 3] |
| 维保总开支（元） | numberCard | [8, 0] | [4, 3] |
| 设备总数 | numberCard | [0, 0] | [2, 3] |
| 已报废设备数 | numberCard | [6, 0] | [2, 3] |
