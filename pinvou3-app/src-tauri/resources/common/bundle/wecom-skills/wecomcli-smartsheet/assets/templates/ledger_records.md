# 台账记录的数据表模版

## 包含表格模版

- **设备台账**：管理企业设备基本信息和历史维修记录，记录设备类型、购买日期、保修年限及当前状态，支持设备总数和平均保修年限统计。
- **退换货台账表**：记录退换货申请和原订单信息，管理处理状态和处理人，支持退货数、换货数及按产品统计的退换货明细分析。
- **发货明细登记**：管理发货单明细，记录货品名称、客户、发货数量、物流状态及金额，支持发货单总金额和货品类型分布统计。
- **销售业务台账**：记录销售订单明细，包含商品名称、客户、数量、单价及收款情况，支持月度订单总额和销售业绩排名统计。

## 设备台账

### 设备基本信息

| 字段 | 类型 |
| --- | --- |
| 购买日期 | FIELD_TYPE_DATE_TIME |
| 保修年限 | FIELD_TYPE_NUMBER |
| 最后编辑人 | FIELD_TYPE_MODIFIED_USER |
| 购置渠道 | FIELD_TYPE_TEXT |
| 设备全名 | FIELD_TYPE_TEXT |
| 现状 | FIELD_TYPE_SELECT |
| 保修截止 | FIELD_TYPE_DATE_TIME |
| 设备类型 | FIELD_TYPE_SELECT |
| 当前设备位置 | FIELD_TYPE_TEXT |
| 设备编号 | FIELD_TYPE_BARCODE |
| 历史维护记录 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 历史维修记录

| 字段 | 类型 |
| --- | --- |
| 维护内容 | FIELD_TYPE_TEXT |
| 设备全名 | FIELD_TYPE_LOOKUP |
| 设备编号 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 维护结果 | FIELD_TYPE_SELECT |
| 责任人 | FIELD_TYPE_TEXT |
| 维护编号 | FIELD_TYPE_TEXT |
| 维护日期 | FIELD_TYPE_TEXT |
| 维护完成照片 | FIELD_TYPE_IMAGE |

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 正常设备数 | numberCard | [6, 0] | [3, 2] |
| 生产设备的平均保修年限 | numberCard | [9, 2] | [3, 2] |
| 维修中设备数 | numberCard | [9, 0] | [3, 2] |
| 设备总数 | numberCard | [2, 0] | [4, 2] |
| 运输设备的平均保修年限 | numberCard | [6, 2] | [3, 2] |
| 机床的平均保修年限 | numberCard | [2, 2] | [4, 2] |

## 退换货台账表

### 退换货记录

| 字段 | 类型 |
| --- | --- |
| 处理人 | FIELD_TYPE_USER |
| 订单编号 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 申请时间 | FIELD_TYPE_DATE_TIME |
| 处理状态 | FIELD_TYPE_SELECT |
| 处理时间 | FIELD_TYPE_DATE_TIME |
| 类型 | FIELD_TYPE_SELECT |
| 订单金额 | FIELD_TYPE_LOOKUP |
| 原因 | FIELD_TYPE_SELECT |
| 产品名称 | FIELD_TYPE_LOOKUP |
| 退换货单号 | FIELD_TYPE_BARCODE |

### 原订单信息

| 字段 | 类型 |
| --- | --- |
| 关联退换货记录 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 图片 | FIELD_TYPE_IMAGE |
| 订单编号 | FIELD_TYPE_BARCODE |
| 订单状态 | FIELD_TYPE_SELECT |
| 跟单员 | FIELD_TYPE_USER |
| 产品名称 | FIELD_TYPE_TEXT |
| 客户 ID | FIELD_TYPE_TEXT |
| 订单金额 | FIELD_TYPE_CURRENCY |
| 下单日期 | FIELD_TYPE_DATE_TIME |
| 数量 | FIELD_TYPE_NUMBER |

### 统计看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 按产品统计 | bar | [0, 4] | [4, 5] |
| 换货数 | numberCard | [8, 1] | [4, 3] |
| 退换货明细 | bar | [4, 4] | [8, 5] |
| 退货数 | numberCard | [4, 1] | [4, 3] |
| 退换货记录数 | numberCard | [0, 1] | [4, 3] |

## 发货明细登记

### 发货单明细表

| 字段 | 类型 |
| --- | --- |
| 货品名称 | FIELD_TYPE_TEXT |
| SKU id | FIELD_TYPE_BARCODE |
| 物流状态 | FIELD_TYPE_SELECT |
| 规格 | FIELD_TYPE_TEXT |
| 签收日期 | FIELD_TYPE_DATE_TIME |
| 发货日期 | FIELD_TYPE_DATE_TIME |
| 含税单价 | FIELD_TYPE_CURRENCY |
| 发货负责人 | FIELD_TYPE_USER |
| 客户名称 | FIELD_TYPE_SELECT |
| 单位 | FIELD_TYPE_TEXT |
| 发货数量 | FIELD_TYPE_NUMBER |
| 总金额 | FIELD_TYPE_FORMULA |
| 货品类型 | FIELD_TYPE_SELECT |
| 发货单号 | FIELD_TYPE_AUTONUMBER |

### 发货信息看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 发货单物流状态 | pie | [8, 0] | [4, 3] |
| 货品类型分布 | doughnut | [8, 3] | [4, 4] |
| 运输中 | numberCard | [4, 0] | [4, 3] |
| 发货单数量 | numberCard | [0, 0] | [4, 3] |
| 发货单总金额 | numberCard | [0, 3] | [4, 4] |
| 发货金额（按客户） | bar | [4, 3] | [4, 4] |

## 销售业务台账

### 订单明细

| 字段 | 类型 |
| --- | --- |
| 单价 | FIELD_TYPE_CURRENCY |
| 收款情况 | FIELD_TYPE_SELECT |
| 下单日期 | FIELD_TYPE_CREATED_TIME |
| 订单总额 | FIELD_TYPE_FORMULA |
| 客户名称 | FIELD_TYPE_TEXT |
| 数量 | FIELD_TYPE_NUMBER |
| 收款截图 | FIELD_TYPE_IMAGE |
| 商品名称 | FIELD_TYPE_SELECT |
| 销售人员 | FIELD_TYPE_USER |
| 跟进状态 | FIELD_TYPE_SELECT |
| 订单号 | FIELD_TYPE_TEXT |

### 2月订单仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 2月订单总额 | numberCard | [0, 1] | [6, 3] |
| 2月订单金额 | bar | [8, 7] | [4, 5] |
| 2月订购数量 | bar | [4, 7] | [4, 5] |
| 2月销售业绩排名 | column | [0, 7] | [4, 5] |
| 2月业绩 | numberCard | [3, 4] | [3, 3] |
| 2月订单状态 | pie | [6, 1] | [6, 3] |
| 2月业绩 | numberCard | [6, 4] | [3, 3] |
| 2月业绩 | numberCard | [9, 4] | [3, 3] |
| 2月业绩 | numberCard | [0, 4] | [3, 3] |
