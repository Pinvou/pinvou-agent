# 采购物流的数据表模版

## 包含表格模版

- **供应商管理**：综合评估和管理供应商，记录供应商类型、价格优势、交付速度及历史合作信息，支持供应商评价分布和品类分布统计。
- **物流跟进表**：跟踪货物物流状态，记录发货日期、预计到达时间、物流服务商及是否延迟，支持在途/已签收物流单统计。
- **采购申请表**：管理采购申请和审批流程，记录物品名称、需求数量、申请部门及审批状态，支持各类别采购申请占比统计。
- **企业采购管理**：管理企业物品采购全流程，记录物品库存、采购状态、供应商及需求数量，支持库存总价和采购状态统计。
- **采购订单管理**：管理采购订单和物品库存，记录采购单价、最新采购状态及供应商信息，支持各品类采购总价分布分析。
- **采购询价比价**：管理多供应商询价和比价，记录报价、货期、起订量及采购意见，支持各物品比价表展示和供应商库管理。

## 供应商管理

### 供应商管理

| 字段 | 类型 |
| --- | --- |
| 供应商类型 | FIELD_TYPE_SELECT |
| 最后更新时间日期 | FIELD_TYPE_MODIFIED_TIME |
| 优势说明 | FIELD_TYPE_TEXT |
| 总体得分 | FIELD_TYPE_FORMULA |
| 交付速度 | FIELD_TYPE_SELECT |
| 供应商名称 | FIELD_TYPE_TEXT |
| 供应商联系方式 | FIELD_TYPE_LOOKUP |
| 供应商历史合作信息 | FIELD_TYPE_REFERENCE |
| 总体评价 | FIELD_TYPE_SELECT |
| 供应商报价 | FIELD_TYPE_REFERENCE |
| 价格优势 | FIELD_TYPE_SELECT |
| 供应商联系人 | FIELD_TYPE_LOOKUP |
| 供应商具体信息 | FIELD_TYPE_REFERENCE |

### 供应商看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各品类供应商分布 | doughnut | [4, 0] | [4, 4] |
| 供应商评价分布 | doughnut | [8, 0] | [4, 4] |
| 供应商总数 | numberCard | [0, 0] | [4, 4] |
| 供应商价格情况 | bar | [0, 4] | [4, 4] |
| 供应商交付速度情况 | bar | [4, 4] | [4, 4] |
| 供应商常驻地分布 | bar | [8, 4] | [4, 4] |

### 供应商联系信息

| 字段 | 类型 |
| --- | --- |
| 供应商联系人 | FIELD_TYPE_TEXT |
| 供应商编号 | FIELD_TYPE_TEXT |
| 供应商类型 | FIELD_TYPE_SELECT |
| 公司常驻地 | FIELD_TYPE_SELECT |
| 供应商名称 | FIELD_TYPE_TEXT |
| 供应商联系方式 | FIELD_TYPE_PHONE_NUMBER |
| 主营产品/服务 | FIELD_TYPE_TEXT |

### 供应商报价情况

| 字段 | 类型 |
| --- | --- |
| 供应商类型 | FIELD_TYPE_LOOKUP |
| 平均报价（元） | FIELD_TYPE_NUMBER |
| 主营产品/服务 | FIELD_TYPE_LOOKUP |
| 备注 | FIELD_TYPE_TEXT |
| 报价单位 | FIELD_TYPE_TEXT |
| 供应商名称 | FIELD_TYPE_REFERENCE |
| 供应商编号 | FIELD_TYPE_LOOKUP |

## 物流跟进表

### 物流跟踪明细

| 字段 | 类型 |
| --- | --- |
| 物流状态 | FIELD_TYPE_SELECT |
| 预计到达时间 | FIELD_TYPE_DATE_TIME |
| 是否延迟 | FIELD_TYPE_FORMULA |
| 货物价值 | FIELD_TYPE_CURRENCY |
| 发货日期 | FIELD_TYPE_DATE_TIME |
| 延迟原因 | FIELD_TYPE_TEXT |
| 数量 (pcs) | FIELD_TYPE_TEXT |
| 单位 | FIELD_TYPE_TEXT |
| 关联供应商 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 实际到达时间 | FIELD_TYPE_DATE_TIME |
| 货运单号 | FIELD_TYPE_BARCODE |
| 物流服务商 | FIELD_TYPE_SELECT |
| 货物信息 | FIELD_TYPE_TEXT |

### 供应商信息表

| 字段 | 类型 |
| --- | --- |
| 合作评级 | FIELD_TYPE_SELECT |
| 主营产品/服务 | FIELD_TYPE_TEXT |
| 供应商名称 | FIELD_TYPE_TEXT |
| 实际货品延迟率 | FIELD_TYPE_FORMULA |
| 联系人 | FIELD_TYPE_TEXT |
| 供应商类型 | FIELD_TYPE_SELECT |
| 公司所在地 | FIELD_TYPE_SELECT |
| 供应商联系方式 | FIELD_TYPE_PHONE_NUMBER |
| 关联货运单 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 物流跟进看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 物流单的服务商分布 | doughnut | [8, 3] | [4, 4] |
| 货运总额 | numberCard | [0, 3] | [4, 4] |
| 有延迟物流单 | numberCard | [4, 0] | [4, 3] |
| 在途物流单 | numberCard | [0, 0] | [4, 3] |
| 物流单的供应商分布 | bar | [4, 3] | [4, 4] |
| 已签收物流单 | numberCard | [8, 0] | [4, 3] |

## 采购申请表

### 采购申请明细

| 字段 | 类型 |
| --- | --- |
| 申请人 | FIELD_TYPE_USER |
| 申请日期 | FIELD_TYPE_CREATED_TIME |
| 单位 | FIELD_TYPE_TEXT |
| 申请单号 | FIELD_TYPE_AUTONUMBER |
| 审批状态 | FIELD_TYPE_SELECT |
| 需求数量 | FIELD_TYPE_NUMBER |
| 审批人 | FIELD_TYPE_USER |
| 采购类型 | FIELD_TYPE_SELECT |
| 规格型号 | FIELD_TYPE_TEXT |
| 申请部门 | FIELD_TYPE_SELECT |
| 物品名称 | FIELD_TYPE_TEXT |

### 采购申请看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各类别采购申请占比 | doughnut | [4, 3] | [4, 4] |
| 待审批采购 | numberCard | [4, 0] | [4, 3] |
| 采购中数量 | numberCard | [8, 0] | [4, 3] |
| 采购申请总数 | numberCard | [0, 0] | [4, 3] |
| 采购申请分布（按部门） | bar | [0, 3] | [4, 4] |
| 采购审批状态 | column | [8, 3] | [4, 4] |

## 企业采购管理

### 采购管理统计图（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 当前库存总数 | numberCard | [0, 0] | [2, 2] |
| 本次新增需采购数量 | numberCard | [2, 0] | [2, 2] |
| 物品类型统计图 | bar | [0, 5] | [4, 3] |
| 当前采购状态统计 | pie | [0, 2] | [4, 3] |
| 各品类采购总价分布 | stackbar | [4, 3] | [8, 5] |
| 本次新采购总价 | numberCard | [8, 0] | [4, 3] |
| 当前库存总价 | numberCard | [4, 0] | [4, 3] |

### 物品管理

| 字段 | 类型 |
| --- | --- |
| 规格 | FIELD_TYPE_TEXT |
| 最近采购时间 | FIELD_TYPE_DATE_TIME |
| 本次需求数量 | FIELD_TYPE_NUMBER |
| 本次需求总价（元） | FIELD_TYPE_FORMULA |
| 采购负责人 | FIELD_TYPE_USER |
| 库存总价（元） | FIELD_TYPE_FORMULA |
| 供应商 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 单位 | FIELD_TYPE_TEXT |
| 采购单价（元） | FIELD_TYPE_NUMBER |
| 物品类型 | FIELD_TYPE_SELECT |
| 采购状态 | FIELD_TYPE_SELECT |
| 物品图片 | FIELD_TYPE_IMAGE |
| 物资需求方 | FIELD_TYPE_USER |
| 每月消耗数量 | FIELD_TYPE_NUMBER |
| 当前库存 | FIELD_TYPE_NUMBER |
| 物品名称 | FIELD_TYPE_TEXT |

### 供应商管理

| 字段 | 类型 |
| --- | --- |
| 联系人 | FIELD_TYPE_TEXT |
| 供应商负责人 | FIELD_TYPE_USER |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 联系地址 | FIELD_TYPE_TEXT |
| 供应商名称 | FIELD_TYPE_TEXT |

## 采购订单管理

### 采购管理统计图（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本次新采购总价 | numberCard | [8, 0] | [4, 3] |
| 当前库存总价 | numberCard | [4, 0] | [4, 3] |
| 当前库存总数 | numberCard | [0, 0] | [2, 2] |
| 本次新增需采购数量 | numberCard | [2, 0] | [2, 2] |
| 物品类型统计图 | bar | [0, 5] | [4, 3] |
| 当前采购状态统计 | pie | [0, 2] | [4, 3] |
| 各品类采购总价分布 | stackbar | [4, 3] | [8, 5] |

### 物品管理

| 字段 | 类型 |
| --- | --- |
| 规格 | FIELD_TYPE_TEXT |
| 最近采购时间 | FIELD_TYPE_DATE_TIME |
| 本次需求数量 | FIELD_TYPE_NUMBER |
| 本次需求总价（元） | FIELD_TYPE_FORMULA |
| 采购负责人 | FIELD_TYPE_USER |
| 库存总价（元） | FIELD_TYPE_FORMULA |
| 供应商 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 单位 | FIELD_TYPE_TEXT |
| 采购单价（元） | FIELD_TYPE_NUMBER |
| 物品类型 | FIELD_TYPE_SELECT |
| 最新采购状态 | FIELD_TYPE_SELECT |
| 物品图片 | FIELD_TYPE_IMAGE |
| 物资需求方 | FIELD_TYPE_USER |
| 每月消耗数量 | FIELD_TYPE_NUMBER |
| 当前库存 | FIELD_TYPE_NUMBER |
| 物品名称 | FIELD_TYPE_TEXT |

### 供应商管理

| 字段 | 类型 |
| --- | --- |
| 联系人 | FIELD_TYPE_TEXT |
| 供应商负责人 | FIELD_TYPE_USER |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 联系地址 | FIELD_TYPE_TEXT |
| 供应商名称 | FIELD_TYPE_TEXT |

## 采购询价比价

### 需询价物品清单

| 字段 | 类型 |
| --- | --- |
| 采购员 | FIELD_TYPE_USER |
| 交期要求 | FIELD_TYPE_DATE_TIME |
| 本次需求数量 | FIELD_TYPE_NUMBER |
| 本次总预算（元） | FIELD_TYPE_FORMULA |
| 关联 | FIELD_TYPE_REFERENCE |
| 单位 | FIELD_TYPE_TEXT |
| 单价预算（元） | FIELD_TYPE_NUMBER |
| 最终选定供应商 | FIELD_TYPE_TEXT |
| 物品类型 | FIELD_TYPE_SELECT |
| 采购状态 | FIELD_TYPE_SELECT |
| 规格/型号 | FIELD_TYPE_TEXT |
| 物品名称 | FIELD_TYPE_SELECT |

### 询价比价总看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 办公椅比价表 | bar | [8, 3] | [4, 3] |
| 茶叶比价表 | bar | [4, 3] | [4, 3] |
| 定制礼盒比价表（已订货 | table | [0, 3] | [4, 3] |

### 询价单-定制礼盒

| 字段 | 类型 |
| --- | --- |
| 供应商对接人(可填微信用户) | FIELD_TYPE_USER |
| 询价单号 | FIELD_TYPE_AUTONUMBER |
| 起订量 | FIELD_TYPE_NUMBER |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 采购员 | FIELD_TYPE_USER |
| 样品照片 | FIELD_TYPE_IMAGE |
| 供应商名称 | FIELD_TYPE_TEXT |
| 是否选购（采购员填 | FIELD_TYPE_CHECKBOX |
| 报价(单价) | FIELD_TYPE_CURRENCY |
| 其他备注（供应商填 | FIELD_TYPE_TEXT |
| 采购意见（采购员填 | FIELD_TYPE_TEXT |
| 报价时间 | FIELD_TYPE_CREATED_TIME |
| 货期(天) | FIELD_TYPE_TEXT |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |

### 询价单-茶叶

| 字段 | 类型 |
| --- | --- |
| 供应商对接人(可填微信用户) | FIELD_TYPE_USER |
| 是否选购（采购员填 | FIELD_TYPE_CHECKBOX |
| 询价单号 | FIELD_TYPE_AUTONUMBER |
| 起订量 | FIELD_TYPE_NUMBER |
| 样品照片 | FIELD_TYPE_IMAGE |
| 供应商名称 | FIELD_TYPE_TEXT |
| 报价(单价) | FIELD_TYPE_CURRENCY |
| 其他备注（供应商填 | FIELD_TYPE_TEXT |
| 采购意见（采购员填 | FIELD_TYPE_TEXT |
| 报价时间 | FIELD_TYPE_CREATED_TIME |
| 采购员 | FIELD_TYPE_USER |
| 货期(天) | FIELD_TYPE_TEXT |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |

### 询价单-办公椅

| 字段 | 类型 |
| --- | --- |
| 供应商对接人(可填微信用户) | FIELD_TYPE_USER |
| 询价单号 | FIELD_TYPE_AUTONUMBER |
| 采购员 | FIELD_TYPE_USER |
| 起订量 | FIELD_TYPE_NUMBER |
| 样品照片 | FIELD_TYPE_IMAGE |
| 供应商名称 | FIELD_TYPE_TEXT |
| 报价(单价) | FIELD_TYPE_CURRENCY |
| 型号 | FIELD_TYPE_SELECT |
| 其他备注（供应商填 | FIELD_TYPE_TEXT |
| 采购意见（采购员填 | FIELD_TYPE_TEXT |
| 报价时间 | FIELD_TYPE_CREATED_TIME |
| 货期(天) | FIELD_TYPE_TEXT |
| 是否选购（采购员填 | FIELD_TYPE_CHECKBOX |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |

### 供应商库

| 字段 | 类型 |
| --- | --- |
| 供应商 | FIELD_TYPE_TEXT |
| 对接群（可添加外部群聊 | FIELD_TYPE_WWGROUP |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 联系人（可填微信用户 | FIELD_TYPE_USER |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 售卖品类 | FIELD_TYPE_SELECT |
