# 项目管理的数据表模版

## 包含表格模版

- **任务管理**：通用任务管理模版，记录任务描述、负责人、状态及截止时间，支持任务状态分布和负责人分工统计。
- **问题跟进**：用于跟踪项目中出现的问题，记录问题描述、紧急程度、跟进人及处理截止时间，支持超期未处理问题预警。
- **通用项目管理**：综合管理多个项目及其子任务，支持项目状态、任务优先级、负责人分布等多维度统计，并整合部门周报管理。
- **工单跟踪管理**：管理咨询、维修、安装、保养等各类工单，记录工单类型、紧急程度及处理状态，支持工单词云和平均处理天数统计。
- **项目研发流程图**：以甘特图形式展示研发各阶段流程，记录责任部门、参与部门、开始/完成时间，统计各阶段和各部门参与周期。
- **项目管理简表**：轻量级项目任务管理模版，仅记录任务负责人、状态和时间，适合小团队快速上手使用。
- **智能表格公式场景案例**：收录智能表格常用公式的实际应用场景，涵盖日期、数字、逻辑、文本、列表函数及 VLOOKUP、SUMIF、COUNTIF 等高级用法，是学习公式的参考手册。
- **设计项目管理**：面向设计团队的需求管理模版，记录需求类型、优先级、承接人及交付时间，支持逾期预警和人员工作量统计。
- **立项申请表**：通过表单收集项目立项申请信息，包括项目背景、预算、实施计划及领导审批，规范项目启动流程。

## 任务管理

### 任务仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 进行中任务数 | numberCard | [3, 1] | [3, 3] |
| 已完成任务数 | numberCard | [9, 1] | [3, 3] |
| 任务总数 | numberCard | [0, 1] | [3, 3] |
| 任务状态分布 | doughnut | [0, 4] | [6, 5] |
| 逾期任务数 | numberCard | [6, 1] | [3, 3] |
| 任务分工 | stackbar | [6, 4] | [6, 5] |

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

## 问题跟进

### 跟进仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 处理中 | numberCard | [3, 1] | [3, 3] |
| ✅ 已解决 | numberCard | [9, 1] | [3, 3] |
| 总问题数 | numberCard | [0, 1] | [3, 3] |
| 状态分布 | stackcolumn | [6, 4] | [3, 4] |
| 紧急问题处理进度 | bar | [0, 4] | [6, 4] |
| ❗超期未处理 | numberCard | [6, 1] | [3, 3] |
| 工作量分布 | stackbar | [9, 4] | [3, 4] |

### 问题记录

| 字段 | 类型 |
| --- | --- |
| 记录人 | FIELD_TYPE_CREATED_USER |
| 问题描述 | FIELD_TYPE_TEXT |
| 处理截止时间 | FIELD_TYPE_DATE_TIME |
| 倒数日 | FIELD_TYPE_FORMULA |
| 问题处理时长 | FIELD_TYPE_FORMULA |
| 问题编号 | FIELD_TYPE_AUTONUMBER |
| 状态 | FIELD_TYPE_SELECT |
| 跟进人 | FIELD_TYPE_USER |
| 紧急程度 | FIELD_TYPE_SELECT |
| 开始时间 | FIELD_TYPE_DATE_TIME |
| 问题创建时间 | FIELD_TYPE_CREATED_TIME |

## 通用项目管理

### 项目仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 已完成项目数 | numberCard | [4, 1] | [2, 3] |
| 任务优先级 | stackcolumn | [6, 5] | [6, 3] |
| 待办任务总数 | numberCard | [0, 5] | [2, 3] |
| 任务完成状态 | stackcolumn | [0, 8] | [6, 4] |
| 本周周报提交数 | numberCard | [4, 5] | [2, 3] |
| 任务负责人分布 | bar | [6, 8] | [6, 4] |
| 已完成任务数 | numberCard | [2, 5] | [2, 3] |
| 进行中项目数 | numberCard | [2, 1] | [2, 3] |
| 项目总数 | numberCard | [0, 1] | [2, 3] |
| 项目和任务分布 | stackbar | [6, 1] | [6, 3] |

### 项目管理

| 字段 | 类型 |
| --- | --- |
| 关联 | FIELD_TYPE_REFERENCE |
| 项目状态 | FIELD_TYPE_SELECT |
| 项目总负责人 | FIELD_TYPE_USER |
| 项目名称 | FIELD_TYPE_SELECT |
| 目标 | FIELD_TYPE_TEXT |
| 项目子任务 | FIELD_TYPE_REFERENCE |
| 关联 1 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 项目子任务管理

| 字段 | 类型 |
| --- | --- |
| 优先级 | FIELD_TYPE_SELECT |
| 实际完成时间 | FIELD_TYPE_DATE_TIME |
| 负责人 | FIELD_TYPE_USER |
| 所属项目 | FIELD_TYPE_SELECT |
| 任务状态 | FIELD_TYPE_SELECT |
| 讨论群 | FIELD_TYPE_WWGROUP |
| 关联的项目信息 | FIELD_TYPE_REFERENCE |
| 所属部门 | FIELD_TYPE_SELECT |
| 任务描述 | FIELD_TYPE_TEXT |
| 任务名称 | FIELD_TYPE_TEXT |
| 启动时间 | FIELD_TYPE_DATE_TIME |
| 截止时间 | FIELD_TYPE_DATE_TIME |

### 部门周报

| 字段 | 类型 |
| --- | --- |
| 提交人 | FIELD_TYPE_USER |
| 所属项目 | FIELD_TYPE_SELECT |
| 汇报时间 | FIELD_TYPE_DATE_TIME |
| 负责人 | FIELD_TYPE_USER |
| 周报内容 | FIELD_TYPE_TEXT |

### 项目成员

| 字段 | 类型 |
| --- | --- |
| 负责的项目名称 | FIELD_TYPE_TEXT |
| 项目总负责人 | FIELD_TYPE_USER |
| 项目目标 | FIELD_TYPE_TWOWAYLINKRECORDS |

## 工单跟踪管理

### 进度管理看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各类型占比 | doughnut | [0, 8] | [8, 2] |
| 按 紧急程度 查看 3 月工单创建量 | smoothline | [8, 1] | [4, 4] |
| 咨询类工单数 | numberCard | [2, 6] | [2, 2] |
| ❗️高优工单 | numberCard | [6, 1] | [2, 2] |
| 平均处理天数 | numberCard | [2, 1] | [2, 2] |
| 按 处理状态和重要紧急程度 查看 | stackcolumn | [0, 3] | [8, 2] |
| 工单问题词云图 | wordCloud | [8, 6] | [4, 4] |
| 维修类工单数 | numberCard | [6, 6] | [2, 2] |
| 工单总数 | numberCard | [0, 1] | [2, 2] |
| ❗️待完成&处理中数量 | numberCard | [4, 1] | [2, 2] |
| 安装类工单数 | numberCard | [0, 6] | [2, 2] |
| 保养类工单数 | numberCard | [4, 6] | [2, 2] |

### 工单汇总

| 字段 | 类型 |
| --- | --- |
| 工单编号 | FIELD_TYPE_AUTONUMBER |
| 问题描述 | FIELD_TYPE_TEXT |
| 工单类型 | FIELD_TYPE_SELECT |
| 客户订单编号 | FIELD_TYPE_TEXT |
| 紧急程度 | FIELD_TYPE_SELECT |
| 工单状态 | FIELD_TYPE_SELECT |
| 创建时间 | FIELD_TYPE_DATE_TIME |
| 开始处理时间 | FIELD_TYPE_DATE_TIME |
| 完成时间 | FIELD_TYPE_DATE_TIME |
| 处理天数 | FIELD_TYPE_FORMULA |

## 项目研发流程图

### 研发流程看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各部门参与周期 (天) | doughnut | [8, 0] | [4, 5] |
| 各研发阶段所需周期 (天) | stackbar | [3, 0] | [5, 5] |

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

## 项目管理简表

### 任务列表

| 字段 | 类型 |
| --- | --- |
| 负责人 | FIELD_TYPE_USER |
| 结束时间 | FIELD_TYPE_DATE_TIME |
| 状态 | FIELD_TYPE_SELECT |
| 开始时间 | FIELD_TYPE_DATE_TIME |
| 任务描述 | FIELD_TYPE_TEXT |

### 任务仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 任务数（按负责人分布） | column | [0, 3] | [6, 6] |
| 任务数（按状态分布） | pie | [6, 3] | [6, 6] |

## 智能表格公式场景案例

### 💡 目录

| 字段 | 类型 |
| --- | --- |
| 场景对应工作表 | FIELD_TYPE_TEXT |
| 图片 | FIELD_TYPE_IMAGE |
| 函数 | FIELD_TYPE_SELECT |
| 场景名 | FIELD_TYPE_TEXT |
| 详细说明 | FIELD_TYPE_TEXT |

### 日期函数

| 字段 | 类型 |
| --- | --- |
| DATEDIF(月) | FIELD_TYPE_FORMULA |
| TODATE(文本转日期） | FIELD_TYPE_FORMULA |
| MONTH(月份) | FIELD_TYPE_FORMULA |
| DATEDIF(日） | FIELD_TYPE_FORMULA |
| 日期 | FIELD_TYPE_DATE_TIME |
| SECOND（秒） | FIELD_TYPE_FORMULA |
| WEEKNUM(周数) | FIELD_TYPE_FORMULA |
| MINUTE（分钟） | FIELD_TYPE_FORMULA |
| YEAR(年份) | FIELD_TYPE_FORMULA |
| HOUR（小时） | FIELD_TYPE_FORMULA |
| TODAY | FIELD_TYPE_FORMULA |
| DATEVALUE（日期转数字） | FIELD_TYPE_FORMULA |
| FIELD_TYPE_DATE_TIME | FIELD_TYPE_FORMULA |
| DAY（日） | FIELD_TYPE_FORMULA |

### 数字函数

| 字段 | 类型 |
| --- | --- |
| +(加法) | FIELD_TYPE_FORMULA |
| CEILING(向上舍入） | FIELD_TYPE_FORMULA |
| \*(乘法) | FIELD_TYPE_FORMULA |
| AVERAGE | FIELD_TYPE_FORMULA |
| /(除法) | FIELD_TYPE_FORMULA |
| MIN | FIELD_TYPE_FORMULA |
| EXP(e的n次幂) | FIELD_TYPE_FORMULA |
| POWER(幂计算） | FIELD_TYPE_FORMULA |
| SUM | FIELD_TYPE_FORMULA |
| 数字1 | FIELD_TYPE_NUMBER |
| RAND(随机数) | FIELD_TYPE_FORMULA |
| MAX | FIELD_TYPE_FORMULA |
| ABS(绝对值) | FIELD_TYPE_FORMULA |
| 数字 2 | FIELD_TYPE_NUMBER |
| ROUND(小数位数） | FIELD_TYPE_FORMULA |
| -(减法) | FIELD_TYPE_FORMULA |
| ^(幂运算) | FIELD_TYPE_FORMULA |
| SQRT(平方根) | FIELD_TYPE_FORMULA |

### 仪表盘1（仪表盘）

### 逻辑函数

| 字段 | 类型 |
| --- | --- |
| AND(且) | FIELD_TYPE_FORMULA |
| TRUE | FIELD_TYPE_FORMULA |
| 城市 | FIELD_TYPE_SELECT |
| IF | FIELD_TYPE_FORMULA |
| ISERROR(是否报错) | FIELD_TYPE_FORMULA |
| ISBLANK(是否为空) | FIELD_TYPE_FORMULA |
| 报错 | FIELD_TYPE_FORMULA |
| IFS | FIELD_TYPE_FORMULA |
| OR(或) | FIELD_TYPE_FORMULA |
| 且或组合 | FIELD_TYPE_FORMULA |
| NOT（取反） | FIELD_TYPE_FORMULA |
| IFERROR(报错) | FIELD_TYPE_FORMULA |
| FALSE | FIELD_TYPE_FORMULA |
| IFBLANK(为空) | FIELD_TYPE_FORMULA |
| SWITCH | FIELD_TYPE_FORMULA |

### 文本函数

| 字段 | 类型 |
| --- | --- |
| LEN(文本长度) | FIELD_TYPE_FORMULA |
| REPLACE(替换) | FIELD_TYPE_FORMULA |
| &(拼接符) | FIELD_TYPE_FORMULA |
| 产品名称 | FIELD_TYPE_TEXT |
| SPLIT(分割) | FIELD_TYPE_FORMULA |
| FIND | FIELD_TYPE_FORMULA |
| SUNSTITUTE(替换) | FIELD_TYPE_FORMULA |
| CONTAINTEXT(文本包含) | FIELD_TYPE_FORMULA |
| 功能名称 | FIELD_TYPE_SELECT |
| SEARCH(查询文本位置) | FIELD_TYPE_FORMULA |
| CHAR(换行符) | FIELD_TYPE_FORMULA |
| CONCAT(拼接) | FIELD_TYPE_FORMULA |

### 列表函数

| 字段 | 类型 |
| --- | --- |
| LISTCOMBINE(列表打平) | FIELD_TYPE_FORMULA |
| AT(取第二个) | FIELD_TYPE_FORMULA |
| CONTAINSALL(都包含) | FIELD_TYPE_FORMULA |
| 单选 | FIELD_TYPE_SELECT |
| 表.列 | FIELD_TYPE_FORMULA |
| 表.列（返回整列内容） | FIELD_TYPE_FORMULA |
| LIST(生成列表) | FIELD_TYPE_FORMULA |
| CONTAINSONLY(只包含) | FIELD_TYPE_FORMULA |
| LISTJOIN(列表拼接) | FIELD_TYPE_FORMULA |
| CONTAINS(列表包含) | FIELD_TYPE_FORMULA |
| 多选 | FIELD_TYPE_SELECT |
| FIRST(取第一个) | FIELD_TYPE_FORMULA |
| LAST(取最后一个) | FIELD_TYPE_FORMULA |

### 标记重复值

| 字段 | 类型 |
| --- | --- |
| 分门店统计商品重复次数 | FIELD_TYPE_FORMULA |
| 统计去重后的门店数 | FIELD_TYPE_FORMULA |
| 门店名称是否重复 | FIELD_TYPE_FORMULA |
| 商品重复次数（大于2） | FIELD_TYPE_FORMULA |
| 门店和商品都重复 | FIELD_TYPE_FORMULA |
| 判断重复-首个显示重复值 | FIELD_TYPE_FORMULA |
| 门店名称 | FIELD_TYPE_SELECT |
| 商品名称 | FIELD_TYPE_TEXT |
| 商品名称-是否重复-仅保留首值 | FIELD_TYPE_FORMULA |
| 商品名称-是否重复 | FIELD_TYPE_FORMULA |
| 自动编号 | FIELD_TYPE_AUTONUMBER |

### 计算销售业绩排名

| 字段 | 类型 |
| --- | --- |
| 门店 | FIELD_TYPE_SELECT |
| 全公司销售排名 | FIELD_TYPE_FORMULA |
| 分割线 | FIELD_TYPE_TEXT |
| 销量 | FIELD_TYPE_NUMBER |
| 姓名 | FIELD_TYPE_TEXT |
| 门店内排名 | FIELD_TYPE_FORMULA |

### 对销量进行累加

| 字段 | 类型 |
| --- | --- |
| 按月销量累加 | FIELD_TYPE_FORMULA |
| 销量 | FIELD_TYPE_NUMBER |
| 日期-月 | FIELD_TYPE_FORMULA |
| 按月累计求和 | FIELD_TYPE_FORMULA |
| 按日销量累加 | FIELD_TYPE_FORMULA |
| 销售日期 | FIELD_TYPE_DATE_TIME |

### 小时分钟计算

| 字段 | 类型 |
| --- | --- |
| 时间2 | FIELD_TYPE_DATE_TIME |
| 时间间隔-小时 | FIELD_TYPE_FORMULA |
| 时间间隔-分钟 | FIELD_TYPE_FORMULA |
| 间隔小时分钟 | FIELD_TYPE_FORMULA |
| 时间1 | FIELD_TYPE_DATE_TIME |

### 计算工作日天数

| 字段 | 类型 |
| --- | --- |
| 开始日期 | FIELD_TYPE_DATE_TIME |
| 项目工作日天数（排除双休） | FIELD_TYPE_FORMULA |
| 结束日期 | FIELD_TYPE_DATE_TIME |
| 项目耗费天数 | FIELD_TYPE_FORMULA |
| 项目耗费工作日（排除双休、节假日、调休） | FIELD_TYPE_FORMULA |

### 上一行减下一行

| 字段 | 类型 |
| --- | --- |
| 收入 | FIELD_TYPE_NUMBER |
| 分隔线 | FIELD_TYPE_TEXT |
| 日期 | FIELD_TYPE_DATE_TIME |
| 剩余金额 | FIELD_TYPE_FORMULA |
| 自动编号 | FIELD_TYPE_AUTONUMBER |
| 剩余库存 | FIELD_TYPE_FORMULA |
| 消耗 | FIELD_TYPE_NUMBER |
| 支出 | FIELD_TYPE_NUMBER |

### 数据透视表一

| 字段 | 类型 |
| --- | --- |
| 销量 | FIELD_TYPE_NUMBER |
| 日环比 | FIELD_TYPE_FORMULA |
| 日期-天 | FIELD_TYPE_DATE_TIME |
| 月同比 | FIELD_TYPE_FORMULA |
| 上月同天 | FIELD_TYPE_FORMULA |

### 数据透视表二

| 字段 | 类型 |
| --- | --- |
| 月环比 | FIELD_TYPE_FORMULA |
| 上月销量 | FIELD_TYPE_FORMULA |
| 月度 | FIELD_TYPE_TEXT |
| 月总销量 | FIELD_TYPE_FORMULA |
| 月度 1 | FIELD_TYPE_TEXT |

### VLOOKUP表一

| 字段 | 类型 |
| --- | --- |
| 邮箱 | FIELD_TYPE_EMAIL |
| 电话号码 | FIELD_TYPE_PHONE_NUMBER |
| 入职日期 | FIELD_TYPE_DATE_TIME |
| 部门 | FIELD_TYPE_SELECT |
| 姓名 | FIELD_TYPE_USER |

### VLOOKUP表二

| 字段 | 类型 |
| --- | --- |
| 所属部门计数 | FIELD_TYPE_FORMULA |
| 所属部门-公式 | FIELD_TYPE_FORMULA |
| 负责人 | FIELD_TYPE_USER |
| 工龄（天） | FIELD_TYPE_FORMULA |
| 所属部门 | FIELD_TYPE_LOOKUP |
| 项目名称 | FIELD_TYPE_TEXT |

### 函数TEXT常见用法

| 字段 | 类型 |
| --- | --- |
| FIELD_TYPE_TEXT(日期HH:MM-分钟) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(日期HH:MM:SS-秒) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(日期yy-年) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(数字补位) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(百分号) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(日期DD-日) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(日期M-月份) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(日期MM-月份) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(日期yyyy-年） | FIELD_TYPE_FORMULA |
| 日期 | FIELD_TYPE_DATE_TIME |
| FIELD_TYPE_TEXT(日期DDD-周) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(数字千位分隔符） | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(数字占位) | FIELD_TYPE_FORMULA |
| 数字 | FIELD_TYPE_NUMBER |
| FIELD_TYPE_TEXT(日期HH-小时) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(日期DDDD-星期) | FIELD_TYPE_FORMULA |
| FIELD_TYPE_TEXT(日期D-日) | FIELD_TYPE_FORMULA |

### 函数SUMIF常见用法

| 字段 | 类型 |
| --- | --- |
| FILTER实现SUMIF | FIELD_TYPE_FORMULA |
| 销量过百的总销量 | FIELD_TYPE_FORMULA |
| 销量 | FIELD_TYPE_NUMBER |
| 姓名 | FIELD_TYPE_TEXT |
| 姓张或销量过百的总销量 | FIELD_TYPE_FORMULA |
| 销量在60-100的总销量 | FIELD_TYPE_FORMULA |

### 函数COUNTIF常见用法

| 字段 | 类型 |
| --- | --- |
| FILTER实现COUNTIF | FIELD_TYPE_FORMULA |
| 分数过百的人数 | FIELD_TYPE_FORMULA |
| 姓名 | FIELD_TYPE_TEXT |
| FILTER多条件 | FIELD_TYPE_FORMULA |
| 分数 | FIELD_TYPE_NUMBER |
| 人名姓张的人数 | FIELD_TYPE_FORMULA |
| 分数大于60小于100人数 | FIELD_TYPE_FORMULA |

### 收集表表格题拆分

| 字段 | 类型 |
| --- | --- |
| 表格题 | FIELD_TYPE_TEXT |
| 姓名 | FIELD_TYPE_FORMULA |
| 入职日期 | FIELD_TYPE_FORMULA |
| 年龄 | FIELD_TYPE_FORMULA |

### 节假日表

| 字段 | 类型 |
| --- | --- |
| 节假日名称 | FIELD_TYPE_SELECT |
| 日期 | FIELD_TYPE_DATE_TIME |

## 设计项目管理

### 设计需求总览（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 已完成需求总计 | numberCard | [3, 1] | [3, 3] |
| 需求方分布 | doughnut | [6, 4] | [6, 4] |
| 未完成需求状态 | bar | [4, 8] | [8, 3] |
| 预估周期延长需求总计 | numberCard | [9, 1] | [3, 3] |
| 人员逾期情况 | bar | [0, 15] | [6, 4] |
| 已承接需求总计 | numberCard | [0, 1] | [3, 3] |
| 人员预估周期延长情况 | bar | [6, 15] | [6, 4] |
| 逾期交付需求总计 | numberCard | [6, 1] | [3, 3] |
| 未完成需求总计 | numberCard | [0, 8] | [4, 3] |
| 各设计师已承接需求的数量分布 | stackcolumn | [6, 12] | [6, 3] |
| 各设计师已承接需求的周期总计 | pie | [0, 12] | [6, 3] |
| 已承接的任务类型分布 | doughnut | [0, 4] | [6, 4] |

### 需求承接

| 字段 | 类型 |
| --- | --- |
| 计划执行周期（只计算工作日） | FIELD_TYPE_FORMULA |
| 需求项目 | FIELD_TYPE_TEXT |
| 具体对接人 | FIELD_TYPE_USER |
| 优先级 | FIELD_TYPE_SELECT |
| 计划开始时间 | FIELD_TYPE_DATE_TIME |
| 需求类型 | FIELD_TYPE_SELECT |
| 需求方 | FIELD_TYPE_SELECT |
| 需求承接人 | FIELD_TYPE_USER |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 计划交付时间 | FIELD_TYPE_DATE_TIME |

### 需求进度管理

| 字段 | 类型 |
| --- | --- |
| 逾期原因及解决方案 | FIELD_TYPE_TEXT |
| 实际开始时间 | FIELD_TYPE_DATE_TIME |
| 周期延长原因及解决方案 | FIELD_TYPE_TEXT |
| 计划开始时间 | FIELD_TYPE_LOOKUP |
| 计划交付时间 | FIELD_TYPE_LOOKUP |
| 当前状态 | FIELD_TYPE_SELECT |
| 优先级 | FIELD_TYPE_LOOKUP |
| 实际执行周期是否延长 | FIELD_TYPE_FORMULA |
| 实际交付时间 | FIELD_TYPE_DATE_TIME |
| 需求承接人 | FIELD_TYPE_USER |
| 备注 | FIELD_TYPE_TEXT |
| 需求项目 | FIELD_TYPE_TEXT |
| 是否逾期 | FIELD_TYPE_FORMULA |

## 立项申请表

### 立项申请表

| 字段 | 类型 |
| --- | --- |
| 您所在的部门是？ | FIELD_TYPE_SELECT |
| 项目预计启动于？ | FIELD_TYPE_DATE_TIME |
| 请提交领导同意的签字文件。 | FIELD_TYPE_IMAGE |
| 请选择填写本表单的日期 | FIELD_TYPE_DATE_TIME |
| 您的姓名是？ | FIELD_TYPE_USER |
| 该项目的类型属于？ | FIELD_TYPE_SELECT |
| 请提供项目详细的实施计划。 | FIELD_TYPE_ATTACHMENT |
| 项目预计结束于？ | FIELD_TYPE_DATE_TIME |
| 是否已通过上级领导同意 | FIELD_TYPE_SELECT |
| 请概述该项目设立的背景及预期达到的效果。 | FIELD_TYPE_TEXT |
| 该项目预算为？ | FIELD_TYPE_NUMBER |
| 项目名称 | FIELD_TYPE_TEXT |
