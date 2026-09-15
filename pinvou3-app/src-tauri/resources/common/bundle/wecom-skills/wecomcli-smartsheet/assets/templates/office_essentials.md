# 办公必备的数据表模版

## 包含表格模版

- **费用报销单**：支持员工提交费用报销申请，经部门和财务双重审批，自动统计各部门报销金额、费用类别分布及待打款单数。
- **报销登记与审批**：简化版报销流程管理，记录报销类型、金额、审批状态，支持多审批人待审批单量统计和报销费用类型分析。
- **信息收集表**：通用信息收集模版，支持收集姓名、部门、日期、图片、附件等多类型数据，并统计提交总数和部门分布。
- **物品领用表**：管理办公物资的申领和审批流程，记录物资库存、申领记录及审批状态，支持部门申领数量统计和库存预警。
- **办公用品采购**：管理办公用品的采购申请、领用记录和库存，支持采购和领用的双重审批流程，并通过仪表盘展示库存和采购分布。
- **资料公示**：用于公示企业办公地点信息，记录各办公点的地址、联系方式、接口人及照片，方便员工查阅。
- **假勤管理**：管理员工请假申请和审批，自动计算请假天数和剩余假期，支持假单审批状态统计和请假类型分布分析。

## 费用报销单

### 报销单统计表

| 字段 | 类型 |
| --- | --- |
| 提单时间 | FIELD_TYPE_CREATED_TIME |
| 报销事宜 | FIELD_TYPE_TEXT |
| 是否已打款 | FIELD_TYPE_CHECKBOX |
| 财务审批人 | FIELD_TYPE_LOOKUP |
| 备注 | FIELD_TYPE_TEXT |
| 费用类别 | FIELD_TYPE_REFERENCE |
| 部门审批人 | FIELD_TYPE_LOOKUP |
| 报销费用 | FIELD_TYPE_CURRENCY |
| 财务审批结果 | FIELD_TYPE_SELECT |
| 部门审批结果 | FIELD_TYPE_SELECT |
| 所在部门 | FIELD_TYPE_REFERENCE |
| 申请人 | FIELD_TYPE_USER |
| 报销材料 | FIELD_TYPE_ATTACHMENT |

### 费用报销情况总览（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 财务待审批单数 | numberCard | [4, 3] | [4, 3] |
| 累计费用报销单量-按部门 | bar | [4, 0] | [4, 3] |
| 累计报销金额-按费用支出类别 | pie | [8, 0] | [4, 3] |
| 部门待审批单数 | numberCard | [0, 3] | [4, 3] |
| 累计费用报销单量-按申请人 | bar | [0, 0] | [4, 3] |
| 待打款审批单数 | numberCard | [8, 3] | [4, 3] |

### 部门审批流

| 字段 | 类型 |
| --- | --- |
| 部门审批人 | FIELD_TYPE_USER |
| 关联 | FIELD_TYPE_REFERENCE |
| 所在部门 | FIELD_TYPE_TEXT |

### 财务审批流

| 字段 | 类型 |
| --- | --- |
| 财务审批人 | FIELD_TYPE_USER |
| 类别标准 | FIELD_TYPE_TEXT |
| 财务审批所需材料 | FIELD_TYPE_TEXT |
| 费用类别 | FIELD_TYPE_TEXT |
| 关联 | FIELD_TYPE_REFERENCE |

## 报销登记与审批

### 员工报销登记

| 字段 | 类型 |
| --- | --- |
| 备注 | FIELD_TYPE_TEXT |
| 支付时间 | FIELD_TYPE_DATE_TIME |
| 申请人 | FIELD_TYPE_USER |
| 应支付金额 | FIELD_TYPE_FORMULA |
| 申请部门 | FIELD_TYPE_SELECT |
| 报销凭证（发票等） | FIELD_TYPE_ATTACHMENT |
| 审批状态 | FIELD_TYPE_SELECT |
| 报销类型 | FIELD_TYPE_REFERENCE |
| 实际支付金额 | FIELD_TYPE_CURRENCY |
| 报销单号 | FIELD_TYPE_AUTONUMBER |
| 报销金额 | FIELD_TYPE_CURRENCY |
| 审批人 | FIELD_TYPE_LOOKUP |
| 创建时间 | FIELD_TYPE_CREATED_TIME |
| 报销事宜 | FIELD_TYPE_TEXT |

### 支出费用类型

| 字段 | 类型 |
| --- | --- |
| 支出类型 | FIELD_TYPE_TEXT |
| 单笔限额 | FIELD_TYPE_CURRENCY |
| 跟进财务 | FIELD_TYPE_USER |
| 备注信息 | FIELD_TYPE_TEXT |

### 报销登记一览图（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 审批人A待审批单量 | numberCard | [3, 0] | [3, 2] |
| 审批人B待审批单量 | numberCard | [6, 0] | [3, 2] |
| 累计报销费用-按支出费用类型 | pie | [8, 2] | [4, 3] |
| 累计报销单量-按申请人 | bar | [0, 2] | [4, 3] |
| 累计报销单量-按部门 | bar | [4, 2] | [4, 3] |
| 审批人C待审批单量 | numberCard | [9, 0] | [3, 2] |
| 当前待审批单量 | numberCard | [0, 0] | [3, 2] |

## 信息收集表

### 信息收集

| 字段 | 类型 |
| --- | --- |
| 问题描述 | FIELD_TYPE_TEXT |
| 日期 | FIELD_TYPE_DATE_TIME |
| 文件 | FIELD_TYPE_ATTACHMENT |
| 电话 | FIELD_TYPE_TEXT |
| 数字 | FIELD_TYPE_NUMBER |
| 图片 | FIELD_TYPE_IMAGE |
| 部门 | FIELD_TYPE_SELECT |
| 姓名 | FIELD_TYPE_TEXT |

### 收集情况统计（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 提交总数 | numberCard | [0, 0] | [4, 4] |
| 部门分布 | column | [0, 4] | [6, 4] |
| 问题描述 | bar | [4, 0] | [8, 4] |
| 提交人分布 | column | [6, 4] | [6, 4] |

## 物品领用表

### 物资申领记录

| 字段 | 类型 |
| --- | --- |
| 申领人 | FIELD_TYPE_USER |
| 申领日期 | FIELD_TYPE_DATE_TIME |
| 部门审批状态 | FIELD_TYPE_SELECT |
| 备注 | FIELD_TYPE_TEXT |
| 是否已领取 | FIELD_TYPE_CHECKBOX |
| 部门审批人 | FIELD_TYPE_LOOKUP |
| 申请用途 | FIELD_TYPE_TEXT |
| 行政审批人 | FIELD_TYPE_LOOKUP |
| 申领记录编号 | FIELD_TYPE_AUTONUMBER |
| 申领部门 | FIELD_TYPE_REFERENCE |
| 申请数量 | FIELD_TYPE_NUMBER |
| 行政审批状态 | FIELD_TYPE_SELECT |
| 物资名称 | FIELD_TYPE_REFERENCE |

### 物资清单

| 字段 | 类型 |
| --- | --- |
| 物资编号 | FIELD_TYPE_BARCODE |
| 申领记录 | FIELD_TYPE_REFERENCE |
| 当前库存 | FIELD_TYPE_FORMULA |
| 库存总量 | FIELD_TYPE_NUMBER |
| 物资名称 | FIELD_TYPE_TEXT |
| 物资照片 | FIELD_TYPE_IMAGE |
| 物资价值（元） | FIELD_TYPE_CURRENCY |
| 行政审批人 | FIELD_TYPE_USER |
| 已发放数量 | FIELD_TYPE_LOOKUP |

### 物资管理概览（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 当前行政待处理申领需求数 | numberCard | [8, 0] | [4, 3] |
| 审批通过物资领用情况 | doughnut | [4, 3] | [4, 3] |
| 物资当前库存 | bar | [0, 0] | [4, 3] |
| 当前部门待处理申领需求数 | numberCard | [4, 0] | [4, 3] |
| 各部门申领物资数量 | table | [8, 3] | [4, 3] |
| 今日各种类物资申领数量 | bar | [0, 3] | [4, 3] |

### 部门物资申领审批流

| 字段 | 类型 |
| --- | --- |
| 备注 | FIELD_TYPE_TEXT |
| 关联 | FIELD_TYPE_REFERENCE |
| 部门名称 | FIELD_TYPE_TEXT |
| 部门审批人 | FIELD_TYPE_USER |

## 办公用品采购

### 办公用品管理数据看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 申请数 | numberCard | [6, 1] | [3, 3] |
| 采购数量统计 | bar | [4, 8] | [4, 6] |
| 待审批数 | numberCard | [3, 1] | [3, 3] |
| 待审批数 | numberCard | [9, 1] | [3, 3] |
| 领用数量统计 | bar | [0, 8] | [4, 6] |
| 剩余库存统计 | bar | [8, 8] | [4, 6] |
| 领用部门分布 | doughnut | [0, 4] | [3, 4] |
| 申请数 | numberCard | [0, 1] | [3, 3] |
| 领用物品分类 | pie | [3, 4] | [3, 4] |
| 采购部门分布 | doughnut | [6, 4] | [3, 4] |
| 采购物品分类 | doughnut | [9, 4] | [3, 4] |

### 办公用品采购记录表

| 字段 | 类型 |
| --- | --- |
| 审批单标识 | FIELD_TYPE_FORMULA |
| 所在部门 | FIELD_TYPE_SELECT |
| 物品分类 | FIELD_TYPE_SELECT |
| 申请时间 | FIELD_TYPE_CREATED_TIME |
| 申请采购数量 | FIELD_TYPE_NUMBER |
| 申请理由 | FIELD_TYPE_TEXT |
| 物品名称 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 剩余库存 | FIELD_TYPE_LOOKUP |
| 采购申请人 | FIELD_TYPE_CREATED_USER |
| 批准采购数量 | FIELD_TYPE_FORMULA |
| 审批状态 | FIELD_TYPE_SELECT |

### 办公用品领用记录表

| 字段 | 类型 |
| --- | --- |
| 所在部门 | FIELD_TYPE_SELECT |
| 审批单标识 | FIELD_TYPE_FORMULA |
| 申请时间 | FIELD_TYPE_CREATED_TIME |
| 剩余库存 | FIELD_TYPE_LOOKUP |
| 批准领用数量 | FIELD_TYPE_FORMULA |
| 物品分类 | FIELD_TYPE_LOOKUP |
| 申请领用数量 | FIELD_TYPE_NUMBER |
| 申请理由 | FIELD_TYPE_TEXT |
| 物品名称 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 领用人 | FIELD_TYPE_CREATED_USER |
| 审批状态 | FIELD_TYPE_SELECT |

### 办公用品库存

| 字段 | 类型 |
| --- | --- |
| 物品分类 | FIELD_TYPE_SELECT |
| 采购记录 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 领用记录 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 已领用数量总和 | FIELD_TYPE_LOOKUP |
| 已采购数量总和 | FIELD_TYPE_LOOKUP |
| 物品名称 | FIELD_TYPE_TEXT |
| 初始库存 | FIELD_TYPE_NUMBER |
| 剩余库存 | FIELD_TYPE_FORMULA |

## 资料公示

### 智能表1

| 字段 | 类型 |
| --- | --- |
| 地址 | FIELD_TYPE_TEXT |
| 电话 | FIELD_TYPE_PHONE_NUMBER |
| 地区/城市 | FIELD_TYPE_SELECT |
| 邮编 | FIELD_TYPE_TEXT |
| 接口人 | FIELD_TYPE_USER |
| 办公点照片 | FIELD_TYPE_IMAGE |
| 办公点描述 | FIELD_TYPE_TEXT |
| 办公地点 | FIELD_TYPE_TEXT |

## 假勤管理

### 请假明细表

| 字段 | 类型 |
| --- | --- |
| 开始时间 | FIELD_TYPE_DATE_TIME |
| 审批状态 | FIELD_TYPE_SELECT |
| 结束时间 | FIELD_TYPE_DATE_TIME |
| 员工姓名 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 申请人 | FIELD_TYPE_CREATED_USER |
| 请假天数 | FIELD_TYPE_FORMULA |
| 审批人 | FIELD_TYPE_USER |
| 证明材料 | FIELD_TYPE_ATTACHMENT |
| 提交时间 | FIELD_TYPE_DATE_TIME |
| 请假类型 | FIELD_TYPE_SELECT |
| 假单申请编号 | FIELD_TYPE_AUTONUMBER |

### 员工信息表

| 字段 | 类型 |
| --- | --- |
| 假期总天数 | FIELD_TYPE_NUMBER |
| 剩余假期 | FIELD_TYPE_FORMULA |
| 员工 | FIELD_TYPE_USER |
| 部门 | FIELD_TYPE_SELECT |
| 累计休假天数 | FIELD_TYPE_LOOKUP |
| 员工姓名 | FIELD_TYPE_FORMULA |
| 关联假勤记录 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 假勤管理看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 待审批假单 | numberCard | [4, 0] | [4, 3] |
| 假单审批状态 | doughnut | [8, 0] | [4, 3] |
| 假单提交趋势 | line | [8, 3] | [4, 4] |
| 请假天数分布 | column | [4, 3] | [4, 4] |
| 总假单数 | numberCard | [0, 0] | [4, 3] |
| 请假类型分布 | bar | [0, 3] | [4, 4] |
