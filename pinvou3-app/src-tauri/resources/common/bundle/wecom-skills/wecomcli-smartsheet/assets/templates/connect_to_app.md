# 链接应用中的数据的数据表模版

## 包含表格模版

- **审批仪表盘**：将企业微信审批数据同步至智能表格，自动统计审批单数量、申请人分布、部门提交趋势及审批状态，实现审批流程的可视化管理。
- **考勤分析仪表盘**：对接企业微信考勤数据，自动汇总员工每月打卡情况，包含迟到、早退、旷工、缺卡、请假等异常统计，支持多维度考勤分析看板。
- **经营收款仪表盘**：整合对外收款、微信小店、抖音、支付宝、小鹅通等多渠道收款数据，统一展示各渠道实收金额、订单总数及销售排行。
- **门店基础数据**：提供法定节假日日历及中国行政区划数据，作为其他门店管理模版的基础数据支撑，用于日期计算和地区筛选。

## 审批仪表盘

### 审批仪表盘（示例）（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 部门提交数量分布 | doughnut | [8, 1] | [4, 3] |
| 总采购金额 | numberCard | [0, 1] | [4, 3] |
| 上月审批单总数 | numberCard | [2, 6] | [2, 2] |
| 上月提交数量趋势 | smoothline | [4, 6] | [4, 2] |
| 本月部门提交数量分布 | doughnut | [8, 4] | [4, 2] |
| 申请部门分布 | doughnut | [0, 8] | [4, 3] |
| 上月部门提交数量分布 | doughnut | [8, 6] | [4, 2] |
| 本月审批单总数 | numberCard | [2, 4] | [2, 2] |
| 审批状态分布 | column | [4, 8] | [4, 3] |
| 审批单总数 | numberCard | [0, 4] | [2, 4] |
| 申请人明细 | bar | [4, 1] | [4, 3] |
| 本月提交数量趋势 | smoothline | [4, 4] | [4, 2] |
| 申请人分布 | bar | [8, 8] | [4, 3] |

### 审批明细（示例）

| 字段 | 类型 |
| --- | --- |
| 审批单编号 | FIELD_TYPE_TEXT |
| 审批单链接 | FIELD_TYPE_TEXT |
| 提交时间 | FIELD_TYPE_DATE_TIME |
| 完成时间 | FIELD_TYPE_DATE_TIME |
| 申请人 | FIELD_TYPE_USER |
| 申请人部门 | FIELD_TYPE_SELECT |
| 申请人账号 | FIELD_TYPE_TEXT |
| 申请事由 | FIELD_TYPE_TEXT |
| 期望交付日期 | FIELD_TYPE_DATE_TIME |
| 采购明细-物品名称 | FIELD_TYPE_TEXT |
| 采购明细-型号或规格 | FIELD_TYPE_TEXT |
| 采购明细-数量 | FIELD_TYPE_NUMBER |
| 采购金额（元） | FIELD_TYPE_CURRENCY |
| 采购明细-备注 | FIELD_TYPE_TEXT |
| 附件 | FIELD_TYPE_TEXT |
| 提交方式 | FIELD_TYPE_SELECT |
| 当前审批状态 | FIELD_TYPE_SELECT |
| 审批流程 | FIELD_TYPE_TEXT |

## 考勤分析仪表盘

### 考勤分析仪表盘（示例）（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 每月早退人数对比（示例） | column | [3, 10] | [3, 3] |
| 累计请假情况分布（示例） | bar | [6, 17] | [6, 3] |
| 累计打卡异常排名（示例） | column | [0, 13] | [12, 3] |
| 当月正常人数（示例） | numberCard | [0, 1] | [3, 4] |
| 每月缺卡人数对比（示例） | column | [9, 10] | [3, 3] |
| 当月旷工人数（示例） | numberCard | [10, 1] | [2, 2] |
| 每月正常人数（示例） | line | [0, 7] | [6, 3] |
| 当月早退人数（示例） | numberCard | [6, 3] | [2, 2] |
| 当月异常人数（示例） | numberCard | [3, 1] | [3, 4] |
| 每月旷工人数对比（示例） | column | [6, 10] | [3, 3] |
| 每月异常人数（示例） | line | [6, 7] | [6, 3] |
| 当月缺卡人数（示例） | numberCard | [8, 1] | [2, 2] |
| 累计加班情况分布（小时）（示例） | pie | [0, 17] | [6, 3] |
| 每月迟到人数对比（示例） | column | [0, 10] | [3, 3] |
| 当月迟到人数（示例） | numberCard | [6, 1] | [2, 2] |
| 当月设备异常人数（示例） | numberCard | [10, 3] | [2, 2] |
| 上月异常人数（示例） | numberCard | [6, 5] | [6, 2] |
| 上月正常人数（示例） | numberCard | [0, 5] | [6, 2] |
| 当月地点异常人数（示例） | numberCard | [8, 3] | [2, 2] |

### 每月打卡概况（示例）

| 字段 | 类型 |
| --- | --- |
| 工作日加班计为加班费(小时) | FIELD_TYPE_NUMBER |
| 招聘类型 | FIELD_TYPE_TEXT |
| 实际工作时长(小时) | FIELD_TYPE_NUMBER |
| 节假日加班计为加班费(小时) | FIELD_TYPE_NUMBER |
| 员工状态 | FIELD_TYPE_SELECT |
| 直属上级 | FIELD_TYPE_TEXT |
| 标准工作时长(小时) | FIELD_TYPE_NUMBER |
| 陪产假(天) | FIELD_TYPE_NUMBER |
| 节假日加班计为调休(小时) | FIELD_TYPE_NUMBER |
| 异常天数(天) | FIELD_TYPE_NUMBER |
| 入职日期 | FIELD_TYPE_DATE_TIME |
| 工作日加班时长(小时) | FIELD_TYPE_NUMBER |
| 进度 | FIELD_TYPE_PROGRESS |
| 当月第一天 | FIELD_TYPE_DATE_TIME |
| 休息天数(天) | FIELD_TYPE_NUMBER |
| 工作日加班计为调休(小时) | FIELD_TYPE_NUMBER |
| 地址 | FIELD_TYPE_TEXT |
| 补卡次数(次) | FIELD_TYPE_NUMBER |
| 别名 | FIELD_TYPE_TEXT |
| 外勤次数(次) | FIELD_TYPE_NUMBER |
| 产假(天) | FIELD_TYPE_NUMBER |
| 职务 | FIELD_TYPE_TEXT |
| 早退时长(分钟) | FIELD_TYPE_NUMBER |
| 调休假(小时) | FIELD_TYPE_NUMBER |
| 姓名 | FIELD_TYPE_TEXT |
| 年假(天) | FIELD_TYPE_NUMBER |
| 设备异常(次) | FIELD_TYPE_NUMBER |
| 员工类型 | FIELD_TYPE_SELECT |
| 月份 | FIELD_TYPE_SELECT |
| 审批打卡次数(次) | FIELD_TYPE_NUMBER |
| 旷工时长(分钟) | FIELD_TYPE_NUMBER |
| 节假日加班时长(小时) | FIELD_TYPE_NUMBER |
| 迟到时长(分钟) | FIELD_TYPE_NUMBER |
| 性别 | FIELD_TYPE_SELECT |
| 离职日期 | FIELD_TYPE_DATE_TIME |
| 迟到次数(次) | FIELD_TYPE_NUMBER |
| 出差(天) | FIELD_TYPE_NUMBER |
| 工号 | FIELD_TYPE_TEXT |
| 异常合计(次) | FIELD_TYPE_NUMBER |
| 职位 | FIELD_TYPE_TEXT |
| 部门 | FIELD_TYPE_TEXT |
| 账号 | FIELD_TYPE_TEXT |
| 办公地点 | FIELD_TYPE_SELECT |
| 病假(小时) | FIELD_TYPE_NUMBER |
| 早退次数(次) | FIELD_TYPE_NUMBER |
| 所属规则 | FIELD_TYPE_SELECT |
| 休息日加班计为调休(小时) | FIELD_TYPE_NUMBER |
| 外出(小时) | FIELD_TYPE_NUMBER |
| 事假(小时) | FIELD_TYPE_NUMBER |
| 缺卡次数(次) | FIELD_TYPE_NUMBER |
| 休息日加班计为加班费(小时) | FIELD_TYPE_NUMBER |
| 地点异常(次) | FIELD_TYPE_NUMBER |
| 加班时长(小时) | FIELD_TYPE_NUMBER |
| 休息日加班时长(小时) | FIELD_TYPE_NUMBER |
| 婚假(天) | FIELD_TYPE_NUMBER |
| 旷工次数(次) | FIELD_TYPE_NUMBER |
| 其他(天) | FIELD_TYPE_NUMBER |
| 应出勤天数(天) | FIELD_TYPE_NUMBER |

## 经营收款仪表盘

### 收款仪表盘（示例）（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 对外收款-销售收款排名 | bar | [0, 3] | [4, 3] |
| 对外收款-客户付款排名 | bar | [8, 3] | [4, 3] |
| 微信小店实收金额 | numberCard | [0, 7] | [3, 2] |
| 小鹅通实收金额 | numberCard | [9, 7] | [3, 2] |
| 支付宝实收金额 | numberCard | [6, 7] | [3, 2] |
| 对外收款-总计 | numberCard | [4, 1] | [8, 2] |
| 订单总数 | numberCard | [0, 1] | [4, 2] |
| 抖音实收金额 | numberCard | [3, 7] | [3, 2] |
| 本月销售光荣榜 | bar | [0, 9] | [12, 4] |
| 对外收款-部门付款排名 | bar | [4, 3] | [4, 3] |

### 对外收款明细（示例）

| 字段 | 类型 |
| --- | --- |
| 关联单（退款记录关联单为收款记录、收款记录关联单为退款记录） | FIELD_TYPE_TEXT |
| 交易状态 | FIELD_TYPE_SELECT |
| 交易单号 | FIELD_TYPE_TEXT |
| 商户单号 | FIELD_TYPE_TEXT |
| 转账时间 | FIELD_TYPE_DATE_TIME |
| 交易时间 | FIELD_TYPE_DATE_TIME |
| 客户 | FIELD_TYPE_USER |
| 金额 | FIELD_TYPE_CURRENCY |
| 成员所在部门 | FIELD_TYPE_SELECT |
| 收款方式 | FIELD_TYPE_SELECT |
| 收款账户 | FIELD_TYPE_NUMBER |
| 收款说明 | FIELD_TYPE_TEXT |
| 客户备注 | FIELD_TYPE_TEXT |
| 商品信息（商品图册下单的会代入商品信息&数量） | FIELD_TYPE_SELECT |
| 关联退款记录 | FIELD_TYPE_REFERENCE |
| 联系人姓名 | FIELD_TYPE_TEXT |
| 手机号 | FIELD_TYPE_TEXT |
| 联系地址 | FIELD_TYPE_TEXT |
| 退款备注 | FIELD_TYPE_TEXT |
| 关联单交易时间 | FIELD_TYPE_DATE_TIME |
| 成员 | FIELD_TYPE_USER |

### 微信小店收款明细

| 字段 | 类型 |
| --- | --- |
| 带货账号类型 | FIELD_TYPE_TEXT |
| 商品实际价格(总共) | FIELD_TYPE_NUMBER |
| 文本 2 | FIELD_TYPE_TEXT |
| 订单完成结算时间 | FIELD_TYPE_URL |
| 商品已退款金额 | FIELD_TYPE_NUMBER |
| 商品属性 | FIELD_TYPE_TEXT |
| 支付方式 | FIELD_TYPE_SELECT |
| 是否预售 | FIELD_TYPE_SELECT |
| 快递单号 | FIELD_TYPE_TEXT |
| 订单实际收款金额 | FIELD_TYPE_NUMBER |
| 跨店优惠 | FIELD_TYPE_NUMBER |
| 订单状态 | FIELD_TYPE_SELECT |
| 商品名称 | FIELD_TYPE_TEXT |
| 物流公司 | FIELD_TYPE_SELECT |
| 带货方式 | FIELD_TYPE_TEXT |
| 带货佣金率 | FIELD_TYPE_TEXT |
| 订单发货时间 | FIELD_TYPE_DATE_TIME |
| 商品数量 | FIELD_TYPE_NUMBER |
| 商品实际价格(单件) | FIELD_TYPE_NUMBER |
| 技术服务费（将以人气卡形式返还） | FIELD_TYPE_URL |
| 省 | FIELD_TYPE_SELECT |
| 买家备注 | FIELD_TYPE_URL |
| 商品发货 | FIELD_TYPE_SELECT |
| 区 | FIELD_TYPE_SELECT |
| 收件人手机 | FIELD_TYPE_TEXT |
| 订单实际支付金额 | FIELD_TYPE_NUMBER |
| 商品编码(自定义) | FIELD_TYPE_SELECT |
| 带货费用 | FIELD_TYPE_NUMBER |
| 定制信息 | FIELD_TYPE_URL |
| 市 | FIELD_TYPE_SELECT |
| 带货费用渠道 | FIELD_TYPE_TEXT |
| 商品价格(单件) | FIELD_TYPE_NUMBER |
| 运费险预计投保费用 | FIELD_TYPE_NUMBER |
| 商家备注 | FIELD_TYPE_URL |
| 商品售后 | FIELD_TYPE_SELECT |
| SKU编码(自定义) | FIELD_TYPE_SELECT |
| 带货费用类型 | FIELD_TYPE_TEXT |
| 收件人地址 | FIELD_TYPE_TEXT |
| 发货方式 | FIELD_TYPE_SELECT |
| 礼物单号 | FIELD_TYPE_TEXT |
| 订单下单时间 | FIELD_TYPE_DATE_TIME |
| 带货账号昵称 | FIELD_TYPE_TEXT |
| 文本 | FIELD_TYPE_TEXT |
| 商品平均运费 | FIELD_TYPE_NUMBER |
| 支付时间 | FIELD_TYPE_DATE_TIME |
| 商品改价 | FIELD_TYPE_NUMBER |
| 收件人姓名 | FIELD_TYPE_TEXT |
| 商品总价 | FIELD_TYPE_NUMBER |
| 订单确认收货时间 | FIELD_TYPE_DATE_TIME |
| 定制预览图 | FIELD_TYPE_URL |
| 商品优惠 | FIELD_TYPE_NUMBER |
| 积分抵扣 | FIELD_TYPE_NUMBER |
| 订单运费 | FIELD_TYPE_NUMBER |
| 商品编码(平台) | FIELD_TYPE_SELECT |
| 交易单号 | FIELD_TYPE_TEXT |
| 技术服务费 | FIELD_TYPE_NUMBER |

### 抖音收款明细

| 字段 | 类型 |
| --- | --- |
| 达人UID | FIELD_TYPE_TEXT |
| 职人UID | FIELD_TYPE_TEXT |
| 商品ID | FIELD_TYPE_TEXT |
| 收款账号 | FIELD_TYPE_TEXT |
| 达人昵称 | FIELD_TYPE_TEXT |
| 支付手续费（已含在软件服务费中） | FIELD_TYPE_CURRENCY |
| 平台撮合佣金 | FIELD_TYPE_CURRENCY |
| 商品类目(游玩类目展示的是上品时的主POI类目) | FIELD_TYPE_SELECT |
| 收款门店 | FIELD_TYPE_TEXT |
| 订单标签 | FIELD_TYPE_TEXT |
| 软件服务费费率特殊情况说明 | FIELD_TYPE_TEXT |
| 核销人ID | FIELD_TYPE_TEXT |
| 核销门店城市 | FIELD_TYPE_TEXT |
| 分账时间 | FIELD_TYPE_TEXT |
| 核销门店省份 | FIELD_TYPE_TEXT |
| 核销门店ID | FIELD_TYPE_TEXT |
| 券售卖金额 | FIELD_TYPE_CURRENCY |
| 核销人昵称 | FIELD_TYPE_USER |
| 服务商补贴(元) | FIELD_TYPE_CURRENCY |
| 结算时间 | FIELD_TYPE_TEXT |
| 支付手续费费率 | FIELD_TYPE_PERCENTAGE |
| 撮合经纪服务费 | FIELD_TYPE_TEXT |
| 结算状态 | FIELD_TYPE_TEXT |
| 职人抖音号 | FIELD_TYPE_TEXT |
| 分期免息手续费 | FIELD_TYPE_TEXT |
| 消费者UID | FIELD_TYPE_TEXT |
| 软件服务费 | FIELD_TYPE_CURRENCY |
| 各类服务费率基数（=订单实收-代商家出资补贴-平台补贴（不参与抽佣）） | FIELD_TYPE_CURRENCY |
| 关联单号 | FIELD_TYPE_NUMBER |
| 服务商佣金 | FIELD_TYPE_TEXT |
| 核销人账号 | FIELD_TYPE_NUMBER |
| 商家补贴金额 | FIELD_TYPE_CURRENCY |
| 达人佣金比例 | FIELD_TYPE_PERCENTAGE |
| 自动提现发起时间 | FIELD_TYPE_TEXT |
| 备注 | FIELD_TYPE_TEXT |
| 订单商品 | FIELD_TYPE_SELECT |
| 达人佣金 | FIELD_TYPE_CURRENCY |
| 平台撮合佣金费率 | FIELD_TYPE_PERCENTAGE |
| 职人激励佣金 | FIELD_TYPE_TEXT |
| 售卖渠道 | FIELD_TYPE_SELECT |
| 自动提现结束时间 | FIELD_TYPE_TEXT |
| 商家应得 | FIELD_TYPE_CURRENCY |
| 职人昵称 | FIELD_TYPE_TEXT |
| 平台补贴（不参与抽佣） | FIELD_TYPE_CURRENCY |
| 预付抵扣金额（元） | FIELD_TYPE_TEXT |
| 职人激励佣金比例 | FIELD_TYPE_TEXT |
| 冻结金额 | FIELD_TYPE_TEXT |
| 服务商佣金比例 | FIELD_TYPE_TEXT |
| 服务商费率类型 | FIELD_TYPE_TEXT |
| 内容渠道 | FIELD_TYPE_TEXT |
| 抖音支付优惠金额 | FIELD_TYPE_CURRENCY |
| 核销渠道 | FIELD_TYPE_SELECT |
| 收款主体 | FIELD_TYPE_TEXT |
| 分期免息手续费费率 | FIELD_TYPE_TEXT |
| 订单实收金额 | FIELD_TYPE_CURRENCY |
| 用户实付金额 | FIELD_TYPE_CURRENCY |
| 服务商名称 | FIELD_TYPE_TEXT |
| 软件服务费费率 | FIELD_TYPE_PERCENTAGE |
| 商品类型 | FIELD_TYPE_TEXT |
| 核销门店 | FIELD_TYPE_TEXT |
| 核销时间 | FIELD_TYPE_DATE_TIME |
| 券码 | FIELD_TYPE_NUMBER |
| 保险费用 | FIELD_TYPE_TEXT |
| 订单编号 | FIELD_TYPE_NUMBER |
| 核销ID | FIELD_TYPE_NUMBER |
| 订单支付时间 | FIELD_TYPE_TEXT |
| 平台补贴金额 | FIELD_TYPE_CURRENCY |

### 支付宝收款明细

| 字段 | 类型 |
| --- | --- |
| 业务类型 | FIELD_TYPE_SELECT |
| 支出金额（-元） | FIELD_TYPE_CURRENCY |
| 账务流水号 | FIELD_TYPE_TEXT |
| 业务流水号 | FIELD_TYPE_TEXT |
| 账户余额（元） | FIELD_TYPE_CURRENCY |
| 对方账号 | FIELD_TYPE_TEXT |
| 商户订单号 | FIELD_TYPE_TEXT |
| 收入金额（+元） | FIELD_TYPE_CURRENCY |
| 发生时间 | FIELD_TYPE_DATE_TIME |
| 商品名称 | FIELD_TYPE_SELECT |
| 交易渠道 | FIELD_TYPE_SELECT |
| 备注 | FIELD_TYPE_TEXT |

### 小鹅通收款明细

| 字段 | 类型 |
| --- | --- |
| 订单实收金额 | FIELD_TYPE_CURRENCY |
| 商品ID | FIELD_TYPE_TEXT |
| 订单状态 | FIELD_TYPE_SELECT |
| 用户UNION_ID | FIELD_TYPE_TEXT |
| 买家手机号 | FIELD_TYPE_TEXT |
| 用户ID | FIELD_TYPE_TEXT |
| 支付时间 | FIELD_TYPE_DATE_TIME |
| 支付方式 | FIELD_TYPE_SELECT |
| 用户地址 | FIELD_TYPE_TEXT |
| 订单类型 | FIELD_TYPE_SELECT |
| 商品名称 | FIELD_TYPE_TEXT |
| 序号 | FIELD_TYPE_TEXT |
| 内部订单号 | FIELD_TYPE_TEXT |
| 结算时间 | FIELD_TYPE_DATE_TIME |
| 买家昵称 | FIELD_TYPE_TEXT |
| 真实姓名 | FIELD_TYPE_TEXT |

### 收款汇总表

| 字段 | 类型 |
| --- | --- |
| 当月收入累计求和 | FIELD_TYPE_FORMULA |
| 渠道 | FIELD_TYPE_SELECT |

## 门店基础数据

### 2026年法定节假日

| 字段 | 类型 |
| --- | --- |
| 日期 | FIELD_TYPE_DATE_TIME |
| 节假日类型 | FIELD_TYPE_SELECT |
| 节假日名称 | FIELD_TYPE_SELECT |

### 中国行政区划分数据

| 字段 | 类型 |
| --- | --- |
| 行政区代码 | FIELD_TYPE_NUMBER |
| 区县 | FIELD_TYPE_TEXT |
| 省份 | FIELD_TYPE_SELECT |
| 城市 | FIELD_TYPE_TEXT |
