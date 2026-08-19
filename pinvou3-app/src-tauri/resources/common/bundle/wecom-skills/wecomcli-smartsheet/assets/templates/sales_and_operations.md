# 销售经营的数据表模版

## 包含表格模版

- **经营分析（仪表盘）**：汇总多门店线上线下经营数据，展示总营业额、各门店收入占比及城市营业额分布，支持收入波动趋势分析。
- **销售CRM系统**：完整的销售 CRM 系统，管理客户跟进、合同、销售业绩及周报，支持销售光荣榜、小组业绩 PK 和目标达成率统计。
- **CRM系统（简易版）**：轻量级客户管理模版，记录客户状态、行业、地区及对接人，支持客户跟进状态分布和地区/行业分析。
- **业绩分析看板**：多维度销售业绩分析，展示总销售额、各渠道销售额、逐日累计销售趋势及销售排行榜，支持产品词云分析。
- **订单管理**：管理月度订单明细，记录商品名称、客户、数量、单价及订单状态，支持销售业绩排名和订单金额统计。
- **销售业绩管理**：精细化销售业绩管理，记录个人目标、每日成单记录及团队目标，支持月度业绩排行榜和今日业绩达成度统计。
- **业绩追踪**：实时追踪销售人员当月和今日业绩，支持个人和小组业绩排名对比，适合销售团队日常业绩监控。
- **销售日报**：门店销售额日报管理，记录各门店各渠道目标和实际销售额，支持目标达成情况统计和线上线下销售额对比。
- **经营分析简表**：简洁的多门店经营分析模版，记录每日收入和支出，自动计算净利润，支持各门店收入占比和利润趋势分析。
- **会员信息登记**：管理会员基本信息，记录生日、口味偏好、消费频次及注册渠道，支持会员总数统计和注册趋势分析。
- **订单跟进**：跟踪订单从下单到发货的全流程，记录订单状态、紧急度、配送地址及应发货时间，支持待发货订单明细统计。

## 经营分析（仪表盘）

### 经营管理仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 总营业额 | numberCard | [0, 1] | [3, 3] |
| 城市营业额 | stackbar | [6, 4] | [6, 4] |
| 线上线下收入 | percentbar | [0, 8] | [5, 4] |
| 线上营业额 | numberCard | [6, 1] | [3, 3] |
| 收入波动 | smoothline | [0, 4] | [6, 4] |
| 各门店收入占比 | pie | [9, 1] | [3, 3] |
| 分店营业额一览表 | bar | [5, 8] | [4, 4] |
| 线下营业额 | numberCard | [3, 1] | [3, 3] |
| 店铺营业情况 | pie | [9, 8] | [3, 4] |

### 经营数据明细

| 字段 | 类型 |
| --- | --- |
| 城市 | FIELD_TYPE_LOOKUP |
| 门店线下收入 | FIELD_TYPE_CURRENCY |
| 项目负责人 | FIELD_TYPE_USER |
| 总收入 | FIELD_TYPE_FORMULA |
| 日期 | FIELD_TYPE_DATE_TIME |
| 店铺名称 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 填写人 | FIELD_TYPE_USER |
| 日销售记录名称 | FIELD_TYPE_TEXT |
| 店铺名称-表单输入 | FIELD_TYPE_SELECT |
| 门店线上收入 | FIELD_TYPE_CURRENCY |

### 门店信息

| 字段 | 类型 |
| --- | --- |
| 门店照片 | FIELD_TYPE_IMAGE |
| 店长 | FIELD_TYPE_TEXT |
| 门店编号 | FIELD_TYPE_AUTONUMBER |
| 开业时间 | FIELD_TYPE_DATE_TIME |
| 门店地址 | FIELD_TYPE_LOCATION |
| 经营状态 | FIELD_TYPE_SELECT |
| 城市 | FIELD_TYPE_SELECT |
| 联系电话 | FIELD_TYPE_TEXT |
| 区域经理 | FIELD_TYPE_USER |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 门店名称 | FIELD_TYPE_TEXT |
| 总收入-求和 | FIELD_TYPE_LOOKUP |
| 区域 | FIELD_TYPE_SELECT |
| 日销售记录 (关联) | FIELD_TYPE_REFERENCE |

## 销售CRM系统

### 业绩进展总看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 销售一组业绩进度 | table | [0, 14] | [6, 3] |
| 客户跟进阶段汇总 | pie | [0, 9] | [4, 5] |
| 🌟销售光荣榜 | bar | [0, 4] | [4, 5] |
| 小组业绩pk | stackbar | [8, 4] | [4, 5] |
| 销售二组业绩进度 | bar | [6, 14] | [6, 3] |
| 签约客户详情 | bar | [8, 9] | [4, 5] |
| 各销售业绩完成度 | column | [4, 4] | [4, 5] |
| 线索来源分布 | pie | [4, 9] | [4, 5] |

### 客户跟进

| 字段 | 类型 |
| --- | --- |
| 成交意向 | FIELD_TYPE_SELECT |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 客户微信（可添加外部联系人） | FIELD_TYPE_USER |
| 节点4 | FIELD_TYPE_DATE_TIME |
| 节点3 | FIELD_TYPE_DATE_TIME |
| 客户跟进记录2 | FIELD_TYPE_TEXT |
| 订单总价 | FIELD_TYPE_NUMBER |
| 节点5-合同到期 | FIELD_TYPE_DATE_TIME |
| 客户跟进记录 | FIELD_TYPE_TEXT |
| 节点1-回访日期（一天后） | FIELD_TYPE_FORMULA |
| 成交日期 | FIELD_TYPE_DATE_TIME |
| 对接群（可添加外部群） | FIELD_TYPE_WWGROUP |
| 登记时间 | FIELD_TYPE_CREATED_TIME |
| 最新进度 | FIELD_TYPE_SELECT |
| 节点2-交付日期 | FIELD_TYPE_DATE_TIME |
| 收款日期 | FIELD_TYPE_DATE_TIME |
| 销售对接人 | FIELD_TYPE_USER |
| 线索来源 | FIELD_TYPE_SELECT |
| 客户名称 | FIELD_TYPE_TEXT |

### 合同管理

| 字段 | 类型 |
| --- | --- |
| 订单金额 | FIELD_TYPE_NUMBER |
| 合同到期日期 | FIELD_TYPE_DATE_TIME |
| 合同编号 | FIELD_TYPE_TEXT |
| 登记日期 | FIELD_TYPE_DATE_TIME |
| 合同开始日期 | FIELD_TYPE_DATE_TIME |
| 销售对接人 | FIELD_TYPE_USER |
| 客户名称 | FIELD_TYPE_TEXT |
| 对接群（可添加外部群） | FIELD_TYPE_WWGROUP |

### 销售业绩

| 字段 | 类型 |
| --- | --- |
| 当前总业绩 | FIELD_TYPE_FORMULA |
| 销售 | FIELD_TYPE_USER |
| 小组 | FIELD_TYPE_SELECT |
| 部门 | FIELD_TYPE_SELECT |
| 业绩完成度 | FIELD_TYPE_FORMULA |
| 业绩目标 | FIELD_TYPE_NUMBER |

### 周报月报

| 字段 | 类型 |
| --- | --- |
| 销售 | FIELD_TYPE_USER |
| 填报日期 | FIELD_TYPE_DATE_TIME |
| 月报文件 | FIELD_TYPE_ATTACHMENT |

### 目标达成率

| 字段 | 类型 |
| --- | --- |
| 业绩达成度 | FIELD_TYPE_FORMULA |
| 销售一组业绩达成度 | FIELD_TYPE_FORMULA |
| 销售一组业绩目标 | FIELD_TYPE_FORMULA |
| 销售二组业绩达成度 | FIELD_TYPE_FORMULA |
| 销售二组业绩目标 | FIELD_TYPE_FORMULA |
| 部门业绩目标 | FIELD_TYPE_FORMULA |

## CRM系统（简易版）

### CRM-客户管理总表

| 字段 | 类型 |
| --- | --- |
| 销售员 | FIELD_TYPE_USER |
| 状态 | FIELD_TYPE_SELECT |
| 公司名称 | FIELD_TYPE_TEXT |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 行业 | FIELD_TYPE_SELECT |
| 备注 | FIELD_TYPE_TEXT |
| 所在地区 | FIELD_TYPE_SELECT |
| 交付员 | FIELD_TYPE_USER |
| 对接群 | FIELD_TYPE_WWGROUP |
| 公司对接人 | FIELD_TYPE_TEXT |

### 客户跟进情况仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 客户所在地区分布 | bar | [0, 3] | [4, 5] |
| 已建联的客户数 | numberCard | [6, 0] | [2, 3] |
| 客户所在行业分布饼图 | pie | [8, 3] | [4, 5] |
| 客户数 | numberCard | [0, 0] | [4, 3] |
| 客户跟进状态分布饼图 | pie | [4, 3] | [4, 5] |
| 未触达的客户数 | numberCard | [8, 0] | [2, 3] |
| 合作中的客户数 | numberCard | [4, 0] | [2, 3] |
| 暂停合作的客户数 | numberCard | [10, 0] | [2, 3] |

## 业绩分析看板

### 销售统计看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 已交付金额 | numberCard | [0, 2] | [3, 2] |
| 直播总额 | numberCard | [6, 2] | [3, 2] |
| 📊 销售排行榜 | stackbar | [0, 12] | [6, 4] |
| 📍 总销售额 | numberCard | [0, 0] | [6, 2] |
| 逐日累计 销售总数 & 销售总额 | smoothline | [0, 8] | [12, 4] |
| 销售渠道分布 | pie | [6, 12] | [6, 4] |
| 老客复购总额 | numberCard | [9, 2] | [3, 2] |
| 商品词云图 | wordCloud | [9, 4] | [3, 4] |
| 按产品 逐日累计销售额 | smoothline | [0, 4] | [9, 4] |
| 企业团购总额 | numberCard | [6, 0] | [3, 2] |
| 待交付金额 | numberCard | [3, 2] | [3, 2] |
| 线下自拓总额 | numberCard | [9, 0] | [3, 2] |

### 订单明细

| 字段 | 类型 |
| --- | --- |
| 订单编号 | FIELD_TYPE_AUTONUMBER |
| 🌟逐日累计销售额 | FIELD_TYPE_FORMULA |
| 产品型号 | FIELD_TYPE_SELECT |
| 数量 | FIELD_TYPE_NUMBER |
| 🌟分产品-逐日累计销售额 | FIELD_TYPE_FORMULA |
| 单价 | FIELD_TYPE_LOOKUP |
| 订单金额 | FIELD_TYPE_FORMULA |
| 🌟逐日累计销售额（万） | FIELD_TYPE_FORMULA |
| 订单创建日期 | FIELD_TYPE_DATE_TIME |
| 发货时间 | FIELD_TYPE_DATE_TIME |
| 跟进销售 | FIELD_TYPE_USER |
| 交货状态 | FIELD_TYPE_SELECT |
| 销售渠道 | FIELD_TYPE_SELECT |
| 🌟逐日累计销售量 | FIELD_TYPE_FORMULA |

### 商品列表

| 字段 | 类型 |
| --- | --- |
| 产品型号 | FIELD_TYPE_TEXT |
| 单价 | FIELD_TYPE_CURRENCY |

## 订单管理

### 3月订单仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 3月业绩 | numberCard | [6, 4] | [3, 3] |
| 3月订单金额 | bar | [8, 7] | [4, 5] |
| 3月销售业绩排名 | column | [0, 7] | [4, 5] |
| 3月业绩 | numberCard | [0, 4] | [3, 3] |
| 3月业绩 | numberCard | [3, 4] | [3, 3] |
| 3月订单总额 | numberCard | [0, 1] | [6, 3] |
| 3月订购数量 | bar | [4, 7] | [4, 5] |
| 3月业绩 | numberCard | [9, 4] | [3, 3] |
| 3月订单状态 | pie | [6, 1] | [6, 3] |

### 订单明细

| 字段 | 类型 |
| --- | --- |
| 单价 | FIELD_TYPE_CURRENCY |
| 下单日期 | FIELD_TYPE_DATE_TIME |
| 订单总额 | FIELD_TYPE_FORMULA |
| 客户名称 | FIELD_TYPE_TEXT |
| 数量 | FIELD_TYPE_NUMBER |
| 预期发货日期 | FIELD_TYPE_DATE_TIME |
| 商品名称 | FIELD_TYPE_SELECT |
| 销售人员 | FIELD_TYPE_USER |
| 订单状态 | FIELD_TYPE_SELECT |
| 订单号 | FIELD_TYPE_AUTONUMBER |

## 销售业绩管理

### 销售业绩排行榜（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| ✨12月业绩排行榜 | bar | [4, 1] | [4, 4] |
| 今日各销售业绩达成度 | column | [8, 5] | [4, 4] |
| 12月各销售业绩进度 | column | [8, 1] | [4, 4] |
| 各小组业绩排行榜 | stackbar | [4, 9] | [4, 4] |
| ✨今日业绩排行榜 | bar | [4, 5] | [4, 4] |

### 个人目标及进度

| 字段 | 类型 |
| --- | --- |
| 业绩目标 | FIELD_TYPE_NUMBER |
| 当前总业绩 | FIELD_TYPE_FORMULA |
| 12月业绩达成度 | FIELD_TYPE_FORMULA |
| 日期 | FIELD_TYPE_DATE_TIME |
| 所属销售小组 | FIELD_TYPE_SELECT |
| 平均每日需完成业绩目标 | FIELD_TYPE_FORMULA |
| 今日业绩是否达标 | FIELD_TYPE_FORMULA |
| 月工作时长(天) | FIELD_TYPE_NUMBER |
| 今日业绩达成度 | FIELD_TYPE_FORMULA |
| 销售 | FIELD_TYPE_USER |

### 每日成单记录

| 字段 | 类型 |
| --- | --- |
| 订单销售额 | FIELD_TYPE_NUMBER |
| 成单日期 | FIELD_TYPE_DATE_TIME |
| 订单售出产品 | FIELD_TYPE_TEXT |
| 销售 | FIELD_TYPE_USER |
| 订单号 | FIELD_TYPE_TEXT |
| 月份 | FIELD_TYPE_SELECT |

### 团队目标及进度

| 字段 | 类型 |
| --- | --- |
| 业绩达成度 | FIELD_TYPE_FORMULA |
| 12月销售目标（所有销售业绩目标之和） | FIELD_TYPE_FORMULA |

### 周报月报

| 字段 | 类型 |
| --- | --- |
| 销售 | FIELD_TYPE_USER |
| 填报日期 | FIELD_TYPE_DATE_TIME |
| 月报文件 | FIELD_TYPE_ATTACHMENT |
| 备注 | FIELD_TYPE_TEXT |

## 业绩追踪

### 业绩仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 今日业绩排名 - 小组 | bar | [8, 6] | [4, 3] |
| 各销售当月业绩 | numberCard | [6, 1] | [3, 2] |
| 当月业绩排名 - 个人 | bar | [0, 3] | [6, 3] |
| 各销售当月业绩 | numberCard | [9, 1] | [3, 2] |
| 当月销售业绩 | numberCard | [0, 1] | [3, 2] |
| 当月业绩排名 - 小组 | pie | [6, 3] | [6, 3] |
| 各销售当月业绩 | numberCard | [3, 1] | [3, 2] |
| 今日销售业绩 | numberCard | [0, 6] | [4, 3] |
| 今日业绩排名 - 个人 | bar | [4, 6] | [4, 3] |

### 业绩明细

| 字段 | 类型 |
| --- | --- |
| 销售人员 | FIELD_TYPE_USER |
| 所属小组 | FIELD_TYPE_SELECT |
| 该人员累计销售额（公式） | FIELD_TYPE_FORMULA |
| 订单金额 | FIELD_TYPE_CURRENCY |
| 成单日期 | FIELD_TYPE_DATE_TIME |
| 成单年月 | FIELD_TYPE_FORMULA |
| 订单号 | FIELD_TYPE_TEXT |

## 销售日报

### 门店销售额日报表

| 字段 | 类型 |
| --- | --- |
| 目标销售额 | FIELD_TYPE_NUMBER |
| 销售渠道 | FIELD_TYPE_SELECT |
| 订单数 | FIELD_TYPE_NUMBER |
| 门店名称 | FIELD_TYPE_SELECT |
| 实际销售额 | FIELD_TYPE_NUMBER |
| 备注 | FIELD_TYPE_TEXT |
| 上报日期 | FIELD_TYPE_DATE_TIME |
| 目标达成情况 | FIELD_TYPE_FORMULA |
| 负责人 | FIELD_TYPE_USER |

### 销售额仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各店铺实际销售额 | column | [8, 3] | [4, 4] |
| 各店铺目标销售额 | column | [8, 0] | [4, 3] |
| （按渠道）销售达成情况 | bar | [0, 3] | [4, 4] |
| 线上销售额 | numberCard | [4, 3] | [2, 4] |
| 线下销售额 | numberCard | [6, 3] | [2, 4] |
| 实际销售额 | numberCard | [4, 0] | [4, 3] |
| 目标销售额 | numberCard | [0, 0] | [4, 3] |

## 经营分析简表

### 经营分析仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 利润支出统计（按日期） | stackcolumn | [6, 6] | [6, 4] |
| 总收入分布（按门店） | pie | [6, 2] | [6, 4] |
| 收入支出统计（按门店） | bar | [0, 6] | [6, 4] |
| 总利润趋势（按门店） | smoothline | [0, 2] | [6, 4] |

### 经营明细

| 字段 | 类型 |
| --- | --- |
| 支出 | FIELD_TYPE_CURRENCY |
| 门店名称 | FIELD_TYPE_SELECT |
| 收入 | FIELD_TYPE_CURRENCY |
| 净利润 | FIELD_TYPE_FORMULA |
| 记录日期 | FIELD_TYPE_DATE_TIME |

## 会员信息登记

### 会员信息表

| 字段 | 类型 |
| --- | --- |
| 生日 | FIELD_TYPE_DATE_TIME |
| 口味偏好 | FIELD_TYPE_SELECT |
| 所在城市 | FIELD_TYPE_SELECT |
| 了解到产品的渠道 | FIELD_TYPE_SELECT |
| 手机号码 | FIELD_TYPE_PHONE_NUMBER |
| 微信昵称 | FIELD_TYPE_TEXT |
| 年龄 | FIELD_TYPE_FORMULA |
| 会员时长 | FIELD_TYPE_FORMULA |
| 消费频次 | FIELD_TYPE_SELECT |
| 性别 | FIELD_TYPE_SELECT |
| 会员 | FIELD_TYPE_USER |
| 注册日期 | FIELD_TYPE_DATE_TIME |
| 会员ID | FIELD_TYPE_AUTONUMBER |

### 会员信息看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 会员总数 | numberCard | [0, 0] | [2, 3] |
| 本月新注册会员 | numberCard | [2, 0] | [2, 3] |
| 了解到产品的渠道 | doughnut | [4, 0] | [4, 3] |
| 消费频次 | bar | [4, 3] | [4, 4] |
| 口味偏好 | doughnut | [0, 3] | [4, 4] |
| 会员注册趋势 | line | [8, 3] | [4, 4] |
| 会员所在城市 | pie | [8, 0] | [4, 3] |

## 订单跟进

### 订单跟进

| 字段 | 类型 |
| --- | --- |
| 商品单价 | FIELD_TYPE_NUMBER |
| 购买数量 | FIELD_TYPE_NUMBER |
| 商品名称 | FIELD_TYPE_TEXT |
| 订单跟进人 | FIELD_TYPE_USER |
| 备注 | FIELD_TYPE_TEXT |
| 订单编号 | FIELD_TYPE_TEXT |
| 下单时间 | FIELD_TYPE_DATE_TIME |
| 紧急度 | FIELD_TYPE_SELECT |
| 客户名称 | FIELD_TYPE_TEXT |
| 商品规格 | FIELD_TYPE_TEXT |
| 订单金额 | FIELD_TYPE_NUMBER |
| 客户联系方式 | FIELD_TYPE_TEXT |
| 配送地址 | FIELD_TYPE_TEXT |
| 应发货时间 | FIELD_TYPE_DATE_TIME |
| 订单状态 | FIELD_TYPE_SELECT |

### 订单仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 订单发货状态 | pie | [0, 3] | [4, 5] |
| 待发货订单明细 | bar | [4, 3] | [8, 5] |
| 订单总金额 | numberCard | [0, 0] | [4, 3] |
