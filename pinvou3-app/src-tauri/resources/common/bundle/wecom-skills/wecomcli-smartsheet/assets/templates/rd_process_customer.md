# 管理产品研发各个流程-客户的数据表模版

## 包含表格模版

- **客户跟进表**：面向销售团队的客户线索管理模版，记录客户来源、跟进阶段、销售对接人及订单总价，支持客户进展看板和销售光荣榜。
- **售后问题跟进**：用于跟踪客户售后问题的全流程，记录问题描述、跟进状态、解决日期及关联客户，并通过仪表盘展示问题来源和高频问题词云。
- **客户满意度调研**：通过表单收集客户对产品和服务的满意度评价，自动分析满意度分布、续费意向及销售人员服务情况，支持关键词词云展示。
- **客户及销售管理**：综合管理客户信息、销售人员及合同，记录客户状态、公司规模、行业及地区，支持销售业绩跟踪和客户动态看板。

## 客户跟进表

### 客户跟进表

| 字段 | 类型 |
| --- | --- |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 客户微信（可添加外部联系人） | FIELD_TYPE_USER |
| 订单总价 | FIELD_TYPE_CURRENCY |
| 客户反馈 | FIELD_TYPE_TEXT |
| 回访日期（一天后） | FIELD_TYPE_FORMULA |
| 对接群（可添加外部群） | FIELD_TYPE_WWGROUP |
| 登记时间 | FIELD_TYPE_DATE_TIME |
| 最新进度 | FIELD_TYPE_SELECT |
| 销售对接人 | FIELD_TYPE_USER |
| 备注 | FIELD_TYPE_TEXT |
| 客户名称-是否重复 | FIELD_TYPE_FORMULA |
| 线索来源 | FIELD_TYPE_SELECT |
| 客户名称 | FIELD_TYPE_TEXT |

### 客户进展看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 客户跟进阶段汇总 | pie | [6, 0] | [3, 3] |
| 线索来源分布 | pie | [9, 0] | [3, 3] |
| 已成功签约客户数 | numberCard | [0, 6] | [3, 3] |
| 高潜客户数 | numberCard | [0, 3] | [3, 3] |
| 成功签约客户明细以及价值求和 | bar | [6, 6] | [3, 3] |
| 高潜客户预估价值 | numberCard | [3, 3] | [3, 3] |
| 当前客户预估价值的求和 | numberCard | [3, 0] | [3, 3] |
| 客户数 | numberCard | [0, 0] | [3, 3] |
| 销售光荣榜 | bar | [9, 6] | [3, 3] |
| 当前已签约成功总价值 | numberCard | [3, 6] | [3, 3] |
| 按销售对接人统计 | bar | [6, 3] | [6, 3] |
| 未反馈数 | numberCard | [6, 0] | [3, 3] |
| 今日待更新反馈 | numberCard | [9, 0] | [3, 3] |
| 客户反馈关键词-待成交 | wordCloud | [4, 3] | [4, 4] |
| 客户反馈关键词-已成交 | wordCloud | [0, 3] | [4, 4] |
| 总客户数 | numberCard | [0, 0] | [3, 3] |
| 已反馈数 | numberCard | [3, 0] | [3, 3] |
| 客户反馈关键词-已流失 | wordCloud | [8, 3] | [4, 4] |

### 意见反馈看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 当前已签约成功总价值 | numberCard | [3, 6] | [3, 3] |
| 高潜客户预估价值 | numberCard | [3, 3] | [3, 3] |
| 客户跟进阶段汇总 | pie | [6, 0] | [3, 3] |
| 客户数 | numberCard | [0, 0] | [3, 3] |
| 销售光荣榜 | bar | [9, 6] | [3, 3] |
| 按销售对接人统计 | bar | [6, 3] | [6, 3] |
| 高潜客户数 | numberCard | [0, 3] | [3, 3] |
| 成功签约客户明细以及价值求和 | bar | [6, 6] | [3, 3] |
| 线索来源分布 | pie | [9, 0] | [3, 3] |
| 当前客户预估价值的求和 | numberCard | [3, 0] | [3, 3] |
| 已成功签约客户数 | numberCard | [0, 6] | [3, 3] |
| 未反馈数 | numberCard | [6, 0] | [3, 3] |
| 今日待更新反馈 | numberCard | [9, 0] | [3, 3] |
| 客户反馈关键词-待成交 | wordCloud | [4, 3] | [4, 4] |
| 客户反馈关键词-已成交 | wordCloud | [0, 3] | [4, 4] |
| 总客户数 | numberCard | [0, 0] | [3, 3] |
| 已反馈数 | numberCard | [3, 0] | [3, 3] |
| 客户反馈关键词-已流失 | wordCloud | [8, 3] | [4, 4] |

## 售后问题跟进

### 问题跟进表

| 字段 | 类型 |
| --- | --- |
| 跟进状态 | FIELD_TYPE_SELECT |
| 反馈日期 | FIELD_TYPE_DATE_TIME |
| 问题跟进人 | FIELD_TYPE_USER |
| 问题截图 | FIELD_TYPE_IMAGE |
| 问题描述 | FIELD_TYPE_TEXT |
| 跟进回复 | FIELD_TYPE_TEXT |
| 所属客户 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 问题跟进群 | FIELD_TYPE_WWGROUP |
| 解决日期 | FIELD_TYPE_DATE_TIME |
| 客户对接负责人 | FIELD_TYPE_LOOKUP |
| 问题录屏 | FIELD_TYPE_ATTACHMENT |
| 反馈人 | FIELD_TYPE_USER |
| 优先级 | FIELD_TYPE_SELECT |
| 问题编号 | FIELD_TYPE_AUTONUMBER |

### 客户信息表

| 字段 | 类型 |
| --- | --- |
| 关联售后问题 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 签约日期 | FIELD_TYPE_DATE_TIME |
| 对接负责人 | FIELD_TYPE_USER |
| 客户编号 | FIELD_TYPE_AUTONUMBER |
| 合同文件 | FIELD_TYPE_ATTACHMENT |
| 需求简述 | FIELD_TYPE_TEXT |
| 问题解决进展 | FIELD_TYPE_FORMULA |
| 行业 | FIELD_TYPE_SELECT |
| 客户名称 | FIELD_TYPE_TEXT |

### 售后问题看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本月 - 反馈问题数 | numberCard | [3, 0] | [2, 3] |
| 总问题数 | numberCard | [0, 0] | [3, 3] |
| 售后问题来源（按客户） | pie | [0, 3] | [3, 3] |
| 本月 - 待解决问题数 | numberCard | [5, 0] | [2, 3] |
| 问题分配情况（按负责人） | bar | [3, 3] | [4, 3] |
| 本月 - 问题跟进情况 | bar | [7, 0] | [5, 3] |
| 高频问题（词云） | wordCloud | [7, 3] | [5, 3] |

## 客户满意度调研

### 客户反馈记录

| 字段 | 类型 |
| --- | --- |
| 服务满意度 | FIELD_TYPE_SELECT |
| 意见反馈 | FIELD_TYPE_TEXT |
| 销售对接人 | FIELD_TYPE_USER |
| 客户微信（可添加外部联系人） | FIELD_TYPE_USER |
| 是否会继续使用 | FIELD_TYPE_SELECT |
| 反馈提交时间 | FIELD_TYPE_DATE_TIME |
| 联系方式 | FIELD_TYPE_PHONE_NUMBER |
| 服务续费日期 | FIELD_TYPE_DATE_TIME |
| 产品满意度 | FIELD_TYPE_SELECT |
| 是否跟进 | FIELD_TYPE_CHECKBOX |
| 对接群（可添加外部群） | FIELD_TYPE_WWGROUP |
| 客户姓名 | FIELD_TYPE_TEXT |

### 满意度仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 反馈「满意」关键词 | wordCloud | [0, 2] | [6, 3] |
| 反馈「非常满意」 | numberCard | [3, 0] | [3, 2] |
| 产品满意度分布 | doughnut | [0, 5] | [3, 4] |
| 表示不会继续使用的客户 | numberCard | [9, 0] | [3, 2] |
| 销售人员与服务满意度情况看板 | stackbar | [6, 5] | [6, 4] |
| 总回收反馈数量 | numberCard | [0, 0] | [3, 2] |
| 服务满意度分布 | doughnut | [3, 5] | [3, 4] |
| 反馈「非常不满意」 | numberCard | [6, 0] | [3, 2] |
| 反馈「不满意」关键词 | wordCloud | [6, 2] | [6, 3] |
| 用户反馈明细 | table | [0, 9] | [12, 3] |

## 客户及销售管理

### 客户管理总表

| 字段 | 类型 |
| --- | --- |
| 预计订单数额 | FIELD_TYPE_CURRENCY |
| 交付员 | FIELD_TYPE_USER |
| 客户名字 | FIELD_TYPE_TEXT |
| 所在地区 | FIELD_TYPE_SELECT |
| 建联时间 | FIELD_TYPE_DATE_TIME |
| 销售人员关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 状态 | FIELD_TYPE_SELECT |
| 合同关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 公司规模 | FIELD_TYPE_SELECT |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 公司名称 | FIELD_TYPE_TEXT |
| 销售员 | FIELD_TYPE_USER |
| 日期 | FIELD_TYPE_DATE_TIME |
| 公司地址 | FIELD_TYPE_LOCATION |
| 部门销售主管 | FIELD_TYPE_LOOKUP |
| 行业 | FIELD_TYPE_SELECT |

### 销售人员表

| 字段 | 类型 |
| --- | --- |
| 部门名称 | FIELD_TYPE_SELECT |
| 工号 | FIELD_TYPE_NUMBER |
| 部门销售主管 | FIELD_TYPE_USER |
| 对接公司 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 对接客户数 | FIELD_TYPE_LOOKUP |
| 销售地区 | FIELD_TYPE_SELECT |
| 销售员 | FIELD_TYPE_USER |
| 对接交付人 | FIELD_TYPE_USER |
| 关联列-1 | FIELD_TYPE_REFERENCE |

### 合同管理

| 字段 | 类型 |
| --- | --- |
| 合同录入 | FIELD_TYPE_USER |
| 合同附件 | FIELD_TYPE_ATTACHMENT |
| 签约人 | FIELD_TYPE_LOOKUP |
| 合同编号 | FIELD_TYPE_TEXT |
| 状态 | FIELD_TYPE_LOOKUP |
| 合同金额 | FIELD_TYPE_LOOKUP |
| 公司名 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 客户动态看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 客户公司规模分布 | pie | [3, 6] | [3, 3] |
| 客户公司行业分布 | pie | [0, 6] | [3, 3] |
| 客户状态分布 | pie | [9, 6] | [3, 3] |
| 客户地区分布 | pie | [6, 6] | [3, 3] |
| 「已建联」客户数 | numberCard | [6, 0] | [2, 2] |
| 「已终止合作」客户数 | numberCard | [8, 0] | [2, 2] |
| 「未触达」客户数 | numberCard | [10, 0] | [2, 2] |
| 各销售的客户状态跟进 | stackbar | [6, 2] | [6, 4] |
| 已成交金额 | numberCard | [0, 2] | [3, 2] |
| 客户状态进展看板 | bar | [0, 4] | [6, 2] |
| 预计成交金额 | numberCard | [3, 2] | [3, 2] |
| 总客户数 | numberCard | [0, 0] | [4, 2] |
| 「合作中」客户数 | numberCard | [4, 0] | [2, 2] |
