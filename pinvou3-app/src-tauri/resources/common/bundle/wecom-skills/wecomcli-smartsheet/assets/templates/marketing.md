# 市场营销的数据表模版

## 包含表格模版

- **广告投放管理**：管理广告计划、素材和投放记录，统计总展示量、点击量、转化率及广告消耗，支持各平台和素材类型的效果对比分析。
- **营销活动策划**：管理年度营销活动策划和任务拆解，记录活动类型、预算、负责人及任务状态，支持季度活动分布和任务优先级统计。
- **内容选题管理**：管理内容选题从登记到发布的全流程，记录目标用户、发布渠道、KPI 及达成情况，支持选题类型分布和人员任务量统计。

## 广告投放管理

### 投放数据仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 总点击量 | numberCard | [3, 2] | [3, 2] |
| 投放条数 | numberCard | [6, 4] | [2, 2] |
| 平均点击率 | numberCard | [3, 4] | [3, 2] |
| 【各类型素材】平均点击率vs转化率 | smoothline | [8, 2] | [4, 3] |
| 按投放时间统计 | smoothline | [8, 8] | [4, 4] |
| 平均转化率 | numberCard | [0, 4] | [3, 2] |
| 总购买量 | numberCard | [6, 2] | [2, 2] |
| 素材数据明细 | bar | [0, 6] | [8, 6] |
| 【各平台】平均点击率vs转化率 | smoothline | [8, 5] | [4, 3] |
| 总广告展示量 | numberCard | [0, 2] | [3, 2] |
| 总广告消耗 | numberCard | [0, 0] | [8, 2] |

### 投放记录总表

| 字段 | 类型 |
| --- | --- |
| 素材ID | FIELD_TYPE_TWOWAYLINKRECORDS |
| 点击量 | FIELD_TYPE_LOOKUP |
| 素材类型 | FIELD_TYPE_LOOKUP |
| 投放记录ID | FIELD_TYPE_AUTONUMBER |
| 素材标题 | FIELD_TYPE_LOOKUP |
| 当日消耗 | FIELD_TYPE_LOOKUP |
| 🔴点击率 | FIELD_TYPE_LOOKUP |
| 投放平台 | FIELD_TYPE_LOOKUP |
| 展示量 | FIELD_TYPE_LOOKUP |
| 🟡转化率 | FIELD_TYPE_LOOKUP |
| 出价方式 | FIELD_TYPE_LOOKUP |
| 【关联依据】广告计划ID | FIELD_TYPE_REFERENCE |
| 投放日期 | FIELD_TYPE_DATE_TIME |

### 效果分析

| 字段 | 类型 |
| --- | --- |
| 分析时间 | FIELD_TYPE_CREATED_TIME |
| 总点击量 | FIELD_TYPE_NUMBER |
| 【关联依据】广告计划ID | FIELD_TYPE_REFERENCE |
| 当日消耗 | FIELD_TYPE_CURRENCY |
| 🔴点击率 | FIELD_TYPE_FORMULA |
| 购买量 | FIELD_TYPE_NUMBER |
| 总展示量 | FIELD_TYPE_NUMBER |
| 素材标题 | FIELD_TYPE_LOOKUP |
| 🟡转化率 | FIELD_TYPE_FORMULA |

### 广告计划

| 字段 | 类型 |
| --- | --- |
| 总预算 | FIELD_TYPE_CURRENCY |
| 开始日期 | FIELD_TYPE_DATE_TIME |
| 目标受众 | FIELD_TYPE_TEXT |
| 计划名称 | FIELD_TYPE_TEXT |
| 结束日期 | FIELD_TYPE_DATE_TIME |
| 出价方式 | FIELD_TYPE_SELECT |
| 投放平台 | FIELD_TYPE_SELECT |
| 【关联依据】广告计划ID | FIELD_TYPE_TEXT |

### 广告素材

| 字段 | 类型 |
| --- | --- |
| 内容描述 | FIELD_TYPE_TEXT |
| 素材ID | FIELD_TYPE_AUTONUMBER |
| 状态 | FIELD_TYPE_SELECT |
| 素材类型 | FIELD_TYPE_SELECT |
| 素材标题 | FIELD_TYPE_TEXT |
| 尺寸规格 | FIELD_TYPE_TEXT |
| 素材链接 | FIELD_TYPE_URL |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |

## 营销活动策划

### 年度活动策划

| 字段 | 类型 |
| --- | --- |
| 活动目标（简要） | FIELD_TYPE_TEXT |
| 活动类型 | FIELD_TYPE_SELECT |
| 预算金额 | FIELD_TYPE_CURRENCY |
| 活动开始时间 | FIELD_TYPE_DATE_TIME |
| 负责人员 | FIELD_TYPE_USER |
| 活动简介 | FIELD_TYPE_TEXT |
| 活动结束时间 | FIELD_TYPE_DATE_TIME |
| 活动季度 | FIELD_TYPE_FORMULA |
| 活动名称 | FIELD_TYPE_SELECT |

### 活动任务管理

| 字段 | 类型 |
| --- | --- |
| 任务详情 | FIELD_TYPE_TEXT |
| 活动名称 | FIELD_TYPE_SELECT |
| 负责人 | FIELD_TYPE_TEXT |
| 任务开始时间 | FIELD_TYPE_DATE_TIME |
| 任务结束时间 | FIELD_TYPE_DATE_TIME |
| 任务状态 | FIELD_TYPE_SELECT |
| 任务名称 | FIELD_TYPE_TEXT |
| 任务优先级 | FIELD_TYPE_SELECT |

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 按活动类型查看分布情况 | doughnut | [8, 0] | [4, 4] |
| 按任务优先级统计 | stackbar | [0, 4] | [4, 4] |
| 季度活动一览表 | bar | [8, 4] | [4, 4] |
| 图表 | stackbar | [0, 8] | [4, 3] |
| 策划活动总数 | numberCard | [0, 0] | [4, 4] |
| 活动细分任务数 | numberCard | [4, 0] | [4, 4] |
| 图表 | stackbar | [4, 8] | [4, 3] |
| 活动目标 | wordCloud | [4, 4] | [4, 4] |

## 内容选题管理

### 选题登记

| 字段 | 类型 |
| --- | --- |
| 目标用户群体 | FIELD_TYPE_SELECT |
| 主题建议 | FIELD_TYPE_TEXT |
| 目标痛点/需求 | FIELD_TYPE_TEXT |
| 风险预警 | FIELD_TYPE_TEXT |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 所属类别 | FIELD_TYPE_SELECT |
| 内容展示渠道 | FIELD_TYPE_SELECT |
| 内容展示形式 | FIELD_TYPE_SELECT |
| 是否需要外部协作方 | FIELD_TYPE_SELECT |
| 登记时间 | FIELD_TYPE_DATE_TIME |
| 选题状态 | FIELD_TYPE_SELECT |
| 预期KPI | FIELD_TYPE_TEXT |
| 如需外部协作方，计划预算为 | FIELD_TYPE_CURRENCY |
| 内容主题 | FIELD_TYPE_TEXT |

### 内容管理

| 字段 | 类型 |
| --- | --- |
| 计划结束时间 | FIELD_TYPE_DATE_TIME |
| 内容展示形式 | FIELD_TYPE_LOOKUP |
| 经验沉淀/复盘 | FIELD_TYPE_TEXT |
| 目标是否达成 | FIELD_TYPE_SELECT |
| 主责及协作成员 | FIELD_TYPE_USER |
| 如需外部协作方，计划预算为 | FIELD_TYPE_LOOKUP |
| 实际达成KPI数据 | FIELD_TYPE_TEXT |
| 当前状态 | FIELD_TYPE_SELECT |
| 预期KPI | FIELD_TYPE_LOOKUP |
| 内容类型 | FIELD_TYPE_LOOKUP |
| 发布是否逾期 | FIELD_TYPE_FORMULA |
| 实际发布及推流日期 | FIELD_TYPE_DATE_TIME |
| 发布平台 | FIELD_TYPE_LOOKUP |
| 计划发布并推流日期 | FIELD_TYPE_DATE_TIME |
| 备选发布及推流日期 | FIELD_TYPE_DATE_TIME |
| 优先级 | FIELD_TYPE_SELECT |
| 具体交付物料清单 | FIELD_TYPE_ATTACHMENT |
| 内容主题 | FIELD_TYPE_TEXT |

### 内容选题数据总览（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 内容发布成本总计 | numberCard | [0, 9] | [7, 4] |
| 目标达成率≥100%的内容类型 | column | [0, 13] | [3, 4] |
| 已通过选题总计 | numberCard | [4, 1] | [4, 3] |
| 待评估选题总计 | numberCard | [8, 1] | [4, 3] |
| 团队成员任务量统计 | bar | [0, 17] | [7, 4] |
| 已通过的内容形式 | bar | [8, 4] | [4, 4] |
| 在各渠道发布内容后目标达成情况 | combo | [7, 13] | [5, 4] |
| 已通过的选题类型 | doughnut | [0, 4] | [8, 4] |
| 成本投入分布 | doughnut | [7, 9] | [5, 4] |
| 选题池子总计 | numberCard | [0, 1] | [4, 3] |
| 目标达成率≥100%的内容展示形式 | pie | [3, 13] | [4, 4] |
| 人员目标达成情况 | bar | [7, 17] | [5, 4] |
