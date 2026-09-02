# 财务会计的数据表模版

## 包含表格模版

- **财务管理报表**：综合管理收入、成本、费用明细，自动计算月度利润、毛利率和净利率，支持季度净利润和年度财务数据总览。
- **财务预算**：管理年度和部门预算，记录预算金额、实际支出及审批状态，支持各部门预算使用情况和剩余预算分析。
- **合同管理**：管理合同台账和签约客户信息，记录合同金额、状态、负责人及截止日期，支持合同总金额和企业类型分布统计。
- **公章使用记录**：管理公章使用申请和审批，记录用章事由、用章日期及审批状态，支持公章使用记录总数和审批情况统计。
- **发票管理**：管理采购、销售、服务等各类发票，记录发票类型、金额、开票日期及付款状态，支持每日发票记录趋势分析。
- **部门损益表**：按部门统计收入、成本和费用，自动计算毛利润和净利润，支持不同部门类型的直接成本分布分析。
- **项目收支管理表**：管理项目合同的收入和支出明细，自动计算待收/待支金额，支持客户款项收入情况和支出分布趋势分析。

## 财务管理报表

### 财务看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 3️⃣ 第三季度净利润 | numberCard | [8, 3] | [2, 2] |
| 成本分布 | doughnut | [4, 8] | [4, 2] |
| 4️⃣ 第四季度净利润 | numberCard | [10, 3] | [2, 2] |
| 平均毛利率 | numberCard | [4, 1] | [2, 2] |
| 年度总收入 | numberCard | [0, 1] | [4, 2] |
| 年度总成本 | numberCard | [8, 1] | [2, 2] |
| 平均净利率 | numberCard | [6, 1] | [2, 2] |
| 累计净利润 | numberCard | [0, 3] | [4, 2] |
| 2️⃣ 第二季度净利润 | numberCard | [6, 3] | [2, 2] |
| 费用分布 | doughnut | [8, 8] | [4, 2] |
| 年度总费用 | numberCard | [10, 1] | [2, 2] |
| 月度成本&费用 | stackbar | [8, 5] | [4, 3] |
| 收入分布 | doughnut | [0, 8] | [4, 2] |
| 1️⃣ 第一季度净利润 | numberCard | [4, 3] | [2, 2] |
| 月度利润金额&利润率 | combo | [0, 5] | [8, 3] |

### 利润表

| 字段 | 类型 |
| --- | --- |
| 毛利率 | FIELD_TYPE_FORMULA |
| 净利润 | FIELD_TYPE_FORMULA |
| 月份/日期 | FIELD_TYPE_DATE_TIME |
| 净利润（万） | FIELD_TYPE_FORMULA |
| 毛利润（万） | FIELD_TYPE_FORMULA |
| 净利率 | FIELD_TYPE_FORMULA |
| 当月费用 | FIELD_TYPE_LOOKUP |
| 当月成本 | FIELD_TYPE_LOOKUP |
| 当月收入 | FIELD_TYPE_LOOKUP |
| 毛利润 | FIELD_TYPE_FORMULA |

### 收入明细

| 字段 | 类型 |
| --- | --- |
| 当月累计营业额 | FIELD_TYPE_FORMULA |
| 当月累计营业额（万） | FIELD_TYPE_FORMULA |
| 日期 | FIELD_TYPE_DATE_TIME |
| 收入金额 | FIELD_TYPE_CURRENCY |
| 收入类型 | FIELD_TYPE_SELECT |

### 成本明细

| 字段 | 类型 |
| --- | --- |
| 当月累计成本（万） | FIELD_TYPE_FORMULA |
| 日期 | FIELD_TYPE_DATE_TIME |
| 当月累计成本 | FIELD_TYPE_FORMULA |
| 金额 | FIELD_TYPE_NUMBER |
| 成本类型 | FIELD_TYPE_SELECT |

### 费用明细

| 字段 | 类型 |
| --- | --- |
| 月份/日期 | FIELD_TYPE_DATE_TIME |
| 当月累计费用（万） | FIELD_TYPE_FORMULA |
| 费用类型 | FIELD_TYPE_SELECT |
| 当月累计费用 | FIELD_TYPE_FORMULA |
| 金额 | FIELD_TYPE_CURRENCY |

## 财务预算

### 年度预算表

| 字段 | 类型 |
| --- | --- |
| 实际支出 | FIELD_TYPE_LOOKUP |
| 剩余金额 | FIELD_TYPE_FORMULA |
| 审批备注 | FIELD_TYPE_TEXT |
| 审批状态 | FIELD_TYPE_SELECT |
| 审批人 | FIELD_TYPE_USER |
| 责任部门 | FIELD_TYPE_REFERENCE |
| 最后更新时间 | FIELD_TYPE_MODIFIED_TIME |
| 使用率 | FIELD_TYPE_FORMULA |
| 部门负责人 | FIELD_TYPE_LOOKUP |
| 预算计划书 | FIELD_TYPE_ATTACHMENT |
| 预算金额 | FIELD_TYPE_CURRENCY |

### 支出明细表

| 字段 | 类型 |
| --- | --- |
| 支出部门 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 备注 | FIELD_TYPE_TEXT |
| 支出日期 | FIELD_TYPE_DATE_TIME |
| 支付方式 | FIELD_TYPE_SELECT |
| 创建时间 | FIELD_TYPE_CREATED_TIME |
| 支出凭证 | FIELD_TYPE_ATTACHMENT |
| 支出类型 | FIELD_TYPE_SELECT |
| 支出编号 | FIELD_TYPE_AUTONUMBER |
| 部门负责人 | FIELD_TYPE_LOOKUP |
| 支出金额 | FIELD_TYPE_CURRENCY |

### 部门预算表

| 字段 | 类型 |
| --- | --- |
| 年度总预算 | FIELD_TYPE_LOOKUP |
| 关联支出记录 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 已消费金额 | FIELD_TYPE_LOOKUP |
| 责任部门 | FIELD_TYPE_TEXT |
| 部门负责人 | FIELD_TYPE_USER |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |

### 财务预算看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 预算审批情况 | bar | [8, 0] | [4, 4] |
| 各部门实际支出 | column | [0, 4] | [4, 4] |
| 预算使用情况 | bar | [8, 4] | [4, 4] |
| 各部门年度预算 | doughnut | [4, 0] | [4, 4] |
| 年度总预算 | numberCard | [0, 0] | [4, 4] |
| 各部门剩余预算 | pie | [4, 4] | [4, 4] |

## 合同管理

### 合同台帐

| 字段 | 类型 |
| --- | --- |
| 合同金额 | FIELD_TYPE_CURRENCY |
| 负责人 | FIELD_TYPE_USER |
| 开始日期 | FIELD_TYPE_DATE_TIME |
| 截止日期 | FIELD_TYPE_DATE_TIME |
| 合同名称 | FIELD_TYPE_TEXT |
| 合同状态 | FIELD_TYPE_SELECT |
| 合同扫描件 | FIELD_TYPE_ATTACHMENT |
| 单位 | FIELD_TYPE_TEXT |
| 签约客户 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 合同编号 | FIELD_TYPE_AUTONUMBER |

### 签约客户信息

| 字段 | 类型 |
| --- | --- |
| 对接人 | FIELD_TYPE_SELECT |
| 联系方式 | FIELD_TYPE_PHONE_NUMBER |
| 关联合同 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 公司常驻地 | FIELD_TYPE_SELECT |
| 企业类型 | FIELD_TYPE_SELECT |
| 公司名称 | FIELD_TYPE_TEXT |
| 主营产品/服务 | FIELD_TYPE_TEXT |

### 合同管理看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 签约企业类型 | column | [7, 4] | [5, 4] |
| 合同状态一览 | doughnut | [7, 0] | [5, 4] |
| 合同总金额 | numberCard | [0, 4] | [4, 4] |
| 履行中合同 | numberCard | [4, 0] | [3, 4] |
| （按负责人）合同分布 | bar | [4, 4] | [3, 4] |
| 合同总数 | numberCard | [0, 0] | [4, 4] |

## 公章使用记录

### 公章使用登记

| 字段 | 类型 |
| --- | --- |
| 用章事由 | FIELD_TYPE_TEXT |
| 用章文件 | FIELD_TYPE_ATTACHMENT |
| 最后编辑时间 | FIELD_TYPE_MODIFIED_TIME |
| 审批人 | FIELD_TYPE_LOOKUP |
| 申请时间 | FIELD_TYPE_CREATED_TIME |
| 用章日期 | FIELD_TYPE_DATE_TIME |
| 申请人 | FIELD_TYPE_CREATED_USER |
| 审批回复 | FIELD_TYPE_TEXT |
| 审批状态 | FIELD_TYPE_SELECT |
| 公章名称 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 使用记录编号 | FIELD_TYPE_AUTONUMBER |

### 公章信息

| 字段 | 类型 |
| --- | --- |
| 公章状态 | FIELD_TYPE_SELECT |
| 启用日期 | FIELD_TYPE_DATE_TIME |
| 负责人 | FIELD_TYPE_USER |
| 关联用章记录 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 公章类型 | FIELD_TYPE_SELECT |
| 公章名称 | FIELD_TYPE_TEXT |

### 公章使用看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 审批情况 | doughnut | [8, 0] | [4, 4] |
| 公章状态 | bar | [8, 4] | [4, 4] |
| 公章使用记录总数 | numberCard | [0, 0] | [4, 4] |
| 申请事由一览 | bar | [0, 4] | [4, 4] |
| 负责人处理情况 | column | [4, 4] | [4, 4] |
| 已通过 | numberCard | [4, 0] | [4, 4] |

## 发票管理

### 发票管理仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 按项目和供应商查看 | stackbar | [0, 2] | [6, 3] |
| 发票状态 | stackbar | [6, 2] | [6, 3] |
| 服务发票额 | numberCard | [9, 0] | [3, 2] |
| 总发票额 | numberCard | [0, 0] | [3, 2] |
| 销售发票额 | numberCard | [3, 0] | [3, 2] |
| 每日发票记录 | line | [0, 5] | [12, 2] |
| 采购发票额 | numberCard | [6, 0] | [3, 2] |

### 发票总表

| 字段 | 类型 |
| --- | --- |
| 发票类型 | FIELD_TYPE_SELECT |
| 发票总金额（含税） | FIELD_TYPE_LOOKUP |
| 纳税人识别号 | FIELD_TYPE_LOOKUP |
| 项目名称 | FIELD_TYPE_TEXT |
| 开票日期 | FIELD_TYPE_DATE_TIME |
| 收款账户类型 | FIELD_TYPE_LOOKUP |
| 付款状态 | FIELD_TYPE_SELECT |
| 备注 | FIELD_TYPE_TEXT |
| 客户/供应商ID | FIELD_TYPE_TWOWAYLINKRECORDS |
| 发票编号 | FIELD_TYPE_BARCODE |

### 商品明细

| 字段 | 类型 |
| --- | --- |
| 商品名称 | FIELD_TYPE_TEXT |
| 发票编号 | FIELD_TYPE_BARCODE |
| 数量 | FIELD_TYPE_NUMBER |
| 规格型号 | FIELD_TYPE_TEXT |
| 总金额 | FIELD_TYPE_FORMULA |
| 单价（含税） | FIELD_TYPE_CURRENCY |

### 交易账户

| 字段 | 类型 |
| --- | --- |
| 客户/供应商ID | FIELD_TYPE_TEXT |
| 账户类型 | FIELD_TYPE_SELECT |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 类型 | FIELD_TYPE_SELECT |
| 名称 | FIELD_TYPE_TEXT |
| 纳税人识别号 | FIELD_TYPE_BARCODE |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 联系人 | FIELD_TYPE_TEXT |

## 部门损益表

### 部门损益表

| 字段 | 类型 |
| --- | --- |
| 部门 | FIELD_TYPE_TEXT |
| 毛利润 | FIELD_TYPE_FORMULA |
| 类型 | FIELD_TYPE_LOOKUP |
| 成本类型 | FIELD_TYPE_SELECT |
| 实际净收入 | FIELD_TYPE_CURRENCY |
| 总直接成本 | FIELD_TYPE_CURRENCY |
| 总直接成本（万） | FIELD_TYPE_FORMULA |
| 实际净收入（万） | FIELD_TYPE_FORMULA |
| 收入类型 | FIELD_TYPE_SELECT |
| 毛利润（万） | FIELD_TYPE_FORMULA |
| 分摊费用 | FIELD_TYPE_CURRENCY |
| 净利润（万） | FIELD_TYPE_FORMULA |
| 统计时间-提取年月 | FIELD_TYPE_FORMULA |
| 分摊费用（万） | FIELD_TYPE_FORMULA |
| 净利润 | FIELD_TYPE_FORMULA |
| 统计时间 | FIELD_TYPE_DATE_TIME |

### 部门管理

| 字段 | 类型 |
| --- | --- |
| 统计时间 | FIELD_TYPE_DATE_TIME |
| 部门 | FIELD_TYPE_TEXT |
| 部门功能简述 | FIELD_TYPE_TEXT |
| 部门经理 | FIELD_TYPE_USER |
| 人员数量 | FIELD_TYPE_NUMBER |
| 类型 | FIELD_TYPE_SELECT |

### 部门损益分析（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 总直接成本（万）的求和 | numberCard | [0, 1] | [3, 3] |
| 分摊费用（万）的求和 | numberCard | [3, 1] | [2, 3] |
| 公司总人数 | numberCard | [2, 7] | [2, 3] |
| 人员数量求和 | table | [4, 7] | [8, 3] |
| 现存部门数量 | numberCard | [0, 7] | [2, 3] |
| 实际净收入（万）的求和 | numberCard | [5, 1] | [2, 3] |
| 净利润（万）的求和 | numberCard | [9, 1] | [3, 3] |
| 毛利润（万）的求和 | numberCard | [7, 1] | [2, 3] |
| 不同部门类型的直接成本分布情况（万元） | combo | [0, 4] | [12, 3] |

## 项目收支管理表

### 项目收入管理

| 字段 | 类型 |
| --- | --- |
| 一类 | FIELD_TYPE_SELECT |
| 收入时间-提取年月 | FIELD_TYPE_FORMULA |
| 金额 | FIELD_TYPE_CURRENCY |
| 关联合同 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 收入时间 | FIELD_TYPE_DATE_TIME |
| 二类 | FIELD_TYPE_SELECT |

### 项目支出管理

| 字段 | 类型 |
| --- | --- |
| 一类 | FIELD_TYPE_SELECT |
| 金额 | FIELD_TYPE_CURRENCY |
| 二类 | FIELD_TYPE_SELECT |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 支出时间-提取年月 | FIELD_TYPE_FORMULA |
| 支出时间 | FIELD_TYPE_DATE_TIME |
| 用途 | FIELD_TYPE_TEXT |

### 合同管理

| 字段 | 类型 |
| --- | --- |
| 客户 | FIELD_TYPE_TEXT |
| 款项类型 | FIELD_TYPE_SELECT |
| 最新收支节点 | FIELD_TYPE_DATE_TIME |
| 项目进度 | FIELD_TYPE_SELECT |
| 支出进度 | FIELD_TYPE_FORMULA |
| 收入进度 | FIELD_TYPE_FORMULA |
| 支出金额 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 待支出金额 | FIELD_TYPE_FORMULA |
| 收入金额 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 待收入金额 | FIELD_TYPE_FORMULA |
| 项目负责人 | FIELD_TYPE_USER |
| 客户类型 | FIELD_TYPE_SELECT |
| 合同金额 | FIELD_TYPE_CURRENCY |
| 合同名称 | FIELD_TYPE_TEXT |

### 项目收支情况（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 待支出金额总和 | numberCard | [6, 2] | [6, 2] |
| 待收入金额总和 | numberCard | [6, 0] | [6, 2] |
| 当前总收入 | numberCard | [0, 0] | [6, 2] |
| 支出分布情况 | line | [0, 7] | [12, 3] |
| 客户款项收入情况 | line | [0, 4] | [12, 3] |
| 当前总支出 | numberCard | [0, 2] | [6, 2] |
