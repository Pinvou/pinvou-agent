# 门店管理的数据表模版

## 包含表格模版

- **门店任务管理**：管理门店推广任务的执行进度，记录任务负责人、当前进度、计划完成时间及验收照片，支持任务进度分布和倒计时统计。
- **巡店记录表**：记录巡店发现的问题，包含问题反馈、处理状态及所属门店，支持问题分布统计和巡店时间趋势分析。
- **连锁门店任务管理**：面向连锁门店的大规模任务管理，支持按区域和门店类型统计完成进度，管理验收申请和全国门店列表。
- **连锁门店巡店管理**：管理全国连锁门店的巡店记录，记录巡店评分、待改善问题及整改状态，支持各地区门店整改情况分析。
- **门店问题反馈**：收集和跟踪门店问题，记录问题类型、处理状态及门店信息，支持各门店问题分布和片区问题统计。
- **门店售后问题登记**：管理门店售后问题，记录问题类型、反馈客户、处理状态及处理措施，支持问题类型分布和反馈趋势分析。
- **门店货品库存管理**：全面管理门店货品的采购、入库、出库和库存，记录货品编码、供应商及库存状态，支持库存价值和出入库情况总览。

## 门店任务管理

### 全局看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| ⏰ 大促倒计时 | numberCard | [0, 1] | [3, 2] |
| 【负责人分工】任务进度看板 | stackcolumn | [6, 3] | [6, 4] |
| 【所有】任务进度分布 | doughnut | [0, 3] | [6, 4] |
| 所有任务数 | numberCard | [3, 1] | [3, 2] |
| 已验收 | numberCard | [8, 1] | [2, 2] |
| 【未完成】任务进度明细 | bar | [0, 7] | [12, 4] |
| ❗不合格 | numberCard | [10, 1] | [2, 2] |
| 进行中 | numberCard | [6, 1] | [2, 2] |

### 执行进度

| 字段 | 类型 |
| --- | --- |
| 任务负责人 | FIELD_TYPE_USER |
| 当前进度 | FIELD_TYPE_SELECT |
| 启动时间 | FIELD_TYPE_DATE_TIME |
| 地址 | FIELD_TYPE_LOCATION |
| 验收现场照片 | FIELD_TYPE_IMAGE |
| 店长 | FIELD_TYPE_USER |
| 计划完成时间 | FIELD_TYPE_DATE_TIME |
| 计划耗时（天） | FIELD_TYPE_FORMULA |
| 推广物料类型 | FIELD_TYPE_SELECT |
| 门店名称 | FIELD_TYPE_TEXT |
| 任务描述 | FIELD_TYPE_TEXT |

### 项目倒计时

| 字段 | 类型 |
| --- | --- |
| 启动时间 | FIELD_TYPE_DATE_TIME |
| 计划完成时间 | FIELD_TYPE_DATE_TIME |
| 项目总执行时间 | FIELD_TYPE_FORMULA |
| 倒计时 | FIELD_TYPE_FORMULA |

## 巡店记录表

### 巡店仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 无需处理 | numberCard | [6, 0] | [3, 3] |
| 问题分布 | doughnut | [6, 3] | [6, 3] |
| 巡店时间分布 | line | [0, 6] | [12, 2] |
| 已处理问题 | numberCard | [9, 0] | [3, 3] |
| 所有问题 | numberCard | [0, 0] | [3, 3] |
| 巡店记录分布 | stackcolumn | [0, 3] | [6, 3] |
| 待处理问题 | numberCard | [3, 0] | [3, 3] |

### 巡店记录

| 字段 | 类型 |
| --- | --- |
| 巡店人员 | FIELD_TYPE_CREATED_USER |
| 处理人 | FIELD_TYPE_USER |
| 问题反馈 | FIELD_TYPE_TEXT |
| 处理状态 | FIELD_TYPE_SELECT |
| 所属门店 | FIELD_TYPE_SELECT |
| 巡检照片 | FIELD_TYPE_IMAGE |
| 反馈时间 | FIELD_TYPE_DATE_TIME |
| 是否需要处理 | FIELD_TYPE_SELECT |

## 连锁门店任务管理

### 项目整体进度（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| ⏰ 距离大促活动上市只剩 | numberCard | [0, 1] | [3, 2] |
| ⏳所有门店进度一览 | doughnut | [6, 1] | [6, 4] |
| 各流程进度一览 | stackcolumn | [0, 14] | [12, 5] |
| 各区域进度一览 | stackcolumn | [0, 5] | [12, 5] |
| 各区域完成进度 | bar | [0, 10] | [6, 4] |
| 不同类型门店完成进度 | bar | [6, 10] | [6, 4] |

### 各区域进度【自动计算】

| 字段 | 类型 |
| --- | --- |
| 已完成验收门店数 | FIELD_TYPE_LOOKUP |
| 总门店数 | FIELD_TYPE_LOOKUP |
| 完成度 | FIELD_TYPE_FORMULA |
| 区域划分 | FIELD_TYPE_SELECT |

### 各类型门店进度【自动计算】

| 字段 | 类型 |
| --- | --- |
| 门店类型 | FIELD_TYPE_SELECT |
| 完成度 | FIELD_TYPE_FORMULA |
| 已完成验收门店数 | FIELD_TYPE_LOOKUP |
| 总门店数 | FIELD_TYPE_LOOKUP |

### 任务执行进度

| 字段 | 类型 |
| --- | --- |
| 门店类型 | FIELD_TYPE_SELECT |
| 区域划分 | FIELD_TYPE_SELECT |
| 当前进度（区域负责人更新 | FIELD_TYPE_SELECT |
| 启动时间 | FIELD_TYPE_DATE_TIME |
| 地址 | FIELD_TYPE_LOCATION |
| 门店店长 | FIELD_TYPE_TEXT |
| 【待验收】现场照片 | FIELD_TYPE_LOOKUP |
| 区域主管 | FIELD_TYPE_USER |
| 计划完成时间 | FIELD_TYPE_DATE_TIME |
| 验收不合格原因（主管填 | FIELD_TYPE_TEXT |
| 计划耗时（天） | FIELD_TYPE_FORMULA |
| 推广物料类型 | FIELD_TYPE_SELECT |
| 门店名称 | FIELD_TYPE_TEXT |

### 任务验收申请表

| 字段 | 类型 |
| --- | --- |
| 上报验收日期 | FIELD_TYPE_DATE_TIME |
| 待验收门店 | FIELD_TYPE_REFERENCE |
| 门店现场物料布置拍照 | FIELD_TYPE_IMAGE |
| 验收状态 | FIELD_TYPE_SELECT |

### 项目倒计时【自动计算】

| 字段 | 类型 |
| --- | --- |
| 启动时间 | FIELD_TYPE_DATE_TIME |
| 计划完成时间 | FIELD_TYPE_DATE_TIME |
| 项目总执行时间 | FIELD_TYPE_FORMULA |
| 倒计时 | FIELD_TYPE_FORMULA |

### 全国门店列表

| 字段 | 类型 |
| --- | --- |
| 门店所属区域 | FIELD_TYPE_SELECT |
| 地址 | FIELD_TYPE_LOCATION |
| 门店店长 | FIELD_TYPE_TEXT |
| 区域负责人 | FIELD_TYPE_USER |
| 门店名称 | FIELD_TYPE_TEXT |

## 连锁门店巡店管理

### 巡检记录表

| 字段 | 类型 |
| --- | --- |
| 店铺名称 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 问题处理群 | FIELD_TYPE_WWGROUP |
| 是否需要整改 | FIELD_TYPE_SELECT |
| 巡店日期 | FIELD_TYPE_DATE_TIME |
| 联系电话 | FIELD_TYPE_LOOKUP |
| 巡店考评人 | FIELD_TYPE_USER |
| 位置打卡 | FIELD_TYPE_LOCATION |
| 待改善问题 - 描述 | FIELD_TYPE_TEXT |
| 店铺店长 | FIELD_TYPE_LOOKUP |
| 待改善问题 - 图例 | FIELD_TYPE_IMAGE |
| 巡店评分 | FIELD_TYPE_PROGRESS |
| 巡检记录名 | FIELD_TYPE_FORMULA |
| 店铺区域 | FIELD_TYPE_LOOKUP |
| 整改状态 | FIELD_TYPE_SELECT |

### 全国店铺表

| 字段 | 类型 |
| --- | --- |
| 经营状态 | FIELD_TYPE_SELECT |
| 门店照片 | FIELD_TYPE_IMAGE |
| 员工人数 | FIELD_TYPE_NUMBER |
| 店长名 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 店长联系方式 | FIELD_TYPE_LOOKUP |
| 城市 | FIELD_TYPE_SELECT |
| 门店地址 | FIELD_TYPE_LOCATION |
| 区域 | FIELD_TYPE_SELECT |
| 巡店记录名 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 开业时间 | FIELD_TYPE_DATE_TIME |
| 店铺名称 | FIELD_TYPE_TEXT |

### 店长信息表

| 字段 | 类型 |
| --- | --- |
| 店长联系方式 | FIELD_TYPE_PHONE_NUMBER |
| 负责店铺 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 店长姓名 | FIELD_TYPE_TEXT |

### 巡店情况仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 西南地区 - 各门店详情 | bar | [3, 9] | [3, 3] |
| 未完成整改 | numberCard | [8, 0] | [4, 3] |
| 华南地区 - 各门店详情 | bar | [3, 12] | [3, 3] |
| 华东地区 - 各门店详情 | bar | [3, 6] | [3, 3] |
| 华北地区 - 整改完成情况 | pie | [0, 3] | [3, 3] |
| 华北地区 - 各门店详情 | bar | [3, 3] | [3, 3] |
| 华南地区 - 待整改问题明细 | bar | [6, 12] | [6, 3] |
| 华南地区 - 整改完成情况 | pie | [0, 12] | [3, 3] |
| 华东地区 - 待整改问题明细 | bar | [6, 6] | [6, 3] |
| 已完成整改 | numberCard | [4, 0] | [4, 3] |
| 西南地区 - 整改完成情况 | pie | [0, 9] | [3, 3] |
| 西南地区 - 待整改问题明细 | bar | [6, 9] | [6, 3] |
| 华东地区 - 整改完成情况 | pie | [0, 6] | [3, 3] |
| 华北地区 - 待整改问题明细 | bar | [6, 3] | [6, 3] |
| 待整改问题 | numberCard | [0, 0] | [4, 3] |

## 门店问题反馈

### 门店问题看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各门店问题分布 | bar | [5, 3] | [7, 5] |
| 不同类别问题占比 | doughnut | [0, 3] | [5, 5] |
| （本月）各片区问题一览 | stackcolumn | [7, 0] | [5, 3] |

### 门店问题记录

| 字段 | 类型 |
| --- | --- |
| 片区 | FIELD_TYPE_LOOKUP |
| 店长 | FIELD_TYPE_LOOKUP |
| 发现问题区域 | FIELD_TYPE_SELECT |
| 处理备注 | FIELD_TYPE_TEXT |
| 处理状态 | FIELD_TYPE_SELECT |
| 反馈日期 | FIELD_TYPE_DATE_TIME |
| 处理人 | FIELD_TYPE_USER |
| 问题类型 | FIELD_TYPE_SELECT |
| 问题反馈人 | FIELD_TYPE_USER |
| 问题编号 | FIELD_TYPE_AUTONUMBER |
| 问题截图/录像 | FIELD_TYPE_ATTACHMENT |
| 处理日期 | FIELD_TYPE_DATE_TIME |
| 问题描述 | FIELD_TYPE_TEXT |
| 门店名称 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 门店信息表

| 字段 | 类型 |
| --- | --- |
| 门店名称 | FIELD_TYPE_TEXT |
| 门店地址 | FIELD_TYPE_LOCATION |
| 关联反馈问题 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 区域经理 | FIELD_TYPE_USER |
| 联系电话 | FIELD_TYPE_TEXT |
| 店长 | FIELD_TYPE_USER |
| 所属片区 | FIELD_TYPE_SELECT |
| 开业日期 | FIELD_TYPE_DATE_TIME |
| 经营状态 | FIELD_TYPE_SELECT |
| 城市 | FIELD_TYPE_SELECT |

## 门店售后问题登记

### 售后问题明细表

| 字段 | 类型 |
| --- | --- |
| 登记人 | FIELD_TYPE_USER |
| 处理措施 | FIELD_TYPE_TEXT |
| 反馈时间 | FIELD_TYPE_DATE_TIME |
| 问题类型 | FIELD_TYPE_SELECT |
| 处理完成时间 | FIELD_TYPE_DATE_TIME |
| 反馈客户 | FIELD_TYPE_SELECT |
| 处理状态 | FIELD_TYPE_SELECT |
| 处理负责人 | FIELD_TYPE_USER |
| 问题产品 | FIELD_TYPE_SELECT |
| 问题描述 | FIELD_TYPE_TEXT |
| 问题编号 | FIELD_TYPE_AUTONUMBER |

### 售后问题看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 问题类型分布 | bar | [4, 0] | [4, 4] |
| 问题反馈趋势 | line | [8, 4] | [4, 3] |
| 问题处理状态 | pie | [8, 0] | [4, 4] |
| "疑似变质"相关产品 | column | [0, 4] | [4, 3] |
| 售后问题数 | numberCard | [0, 0] | [4, 4] |
| 反馈客户分布 | doughnut | [4, 4] | [4, 3] |

## 门店货品库存管理

### 出库管理

| 字段 | 类型 |
| --- | --- |
| 确认出库 | FIELD_TYPE_CHECKBOX |
| 货品编码 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 仓管员确认 | FIELD_TYPE_LOOKUP |
| 出库数量 | FIELD_TYPE_NUMBER |
| 出库日期 | FIELD_TYPE_DATE_TIME |
| 出库单号 | FIELD_TYPE_BARCODE |
| 出库用途 | FIELD_TYPE_SELECT |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 出库货品名称 | FIELD_TYPE_TEXT |
| 出库日期-提取年月 | FIELD_TYPE_FORMULA |
| 出库人 | FIELD_TYPE_USER |
| 出库金额 | FIELD_TYPE_FORMULA |
| 出库单位 | FIELD_TYPE_LOOKUP |
| 出库门店/仓库 | FIELD_TYPE_LOCATION |

### 入库管理

| 字段 | 类型 |
| --- | --- |
| 入库总额 | FIELD_TYPE_FORMULA |
| 瑕疵占比 | FIELD_TYPE_FORMULA |
| 申请人 | FIELD_TYPE_USER |
| 采购单号 | FIELD_TYPE_BARCODE |
| 来货是否与采购数量一致 | FIELD_TYPE_FORMULA |
| 实际入库数量 | FIELD_TYPE_NUMBER |
| 入库日期 | FIELD_TYPE_DATE_TIME |
| 瑕疵数量 | FIELD_TYPE_NUMBER |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 入库商品规格 | FIELD_TYPE_LOOKUP |
| 来货数量 | FIELD_TYPE_NUMBER |
| 入库仓库 | FIELD_TYPE_LOCATION |
| 入库货品名称 | FIELD_TYPE_LOOKUP |
| 入库货品编码 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 仓管员 | FIELD_TYPE_LOOKUP |
| 采购数量-求和 | FIELD_TYPE_LOOKUP |
| 入库单位 | FIELD_TYPE_LOOKUP |
| 确认入库 | FIELD_TYPE_CHECKBOX |

### 采购管理

| 字段 | 类型 |
| --- | --- |
| 采购日期-提取年月 | FIELD_TYPE_FORMULA |
| 供应商 | FIELD_TYPE_LOOKUP |
| 采购单号 | FIELD_TYPE_BARCODE |
| 货品单位 | FIELD_TYPE_LOOKUP |
| 采购数量 | FIELD_TYPE_NUMBER |
| 采购日期 | FIELD_TYPE_DATE_TIME |
| 采购单价 | FIELD_TYPE_CURRENCY |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 采购人 | FIELD_TYPE_USER |
| 采购货品 | FIELD_TYPE_TEXT |
| 来货状态 | FIELD_TYPE_SELECT |
| 预计到货仓库 | FIELD_TYPE_LOCATION |
| 仓管员 | FIELD_TYPE_LOOKUP |
| 采购总额 | FIELD_TYPE_FORMULA |
| 物流单号 | FIELD_TYPE_BARCODE |
| 备注 | FIELD_TYPE_TEXT |

### 库存管理

| 字段 | 类型 |
| --- | --- |
| 保质期状态 | FIELD_TYPE_SELECT |
| 基础库存量 | FIELD_TYPE_NUMBER |
| 盘点人 | FIELD_TYPE_USER |
| 出库数量总计 | FIELD_TYPE_LOOKUP |
| 出库批次 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 入库批次 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 当前库存价值 | FIELD_TYPE_FORMULA |
| 间隔天数 | FIELD_TYPE_FORMULA |
| 库存安全值-Min | FIELD_TYPE_NUMBER |
| 实际入库数量总计 | FIELD_TYPE_LOOKUP |
| 货架位置 | FIELD_TYPE_TEXT |
| 库存状态 | FIELD_TYPE_FORMULA |
| 是否已超盘点周期 | FIELD_TYPE_FORMULA |
| 货品编码 | FIELD_TYPE_TEXT |
| 盘点周期（天） | FIELD_TYPE_NUMBER |
| 存储仓库 | FIELD_TYPE_LOOKUP |
| 最后盘点日期 | FIELD_TYPE_DATE_TIME |
| 库存安全值-Max | FIELD_TYPE_NUMBER |
| 货品名称 | FIELD_TYPE_LOOKUP |
| 当前可用库存 | FIELD_TYPE_FORMULA |

### 货品总表

| 字段 | 类型 |
| --- | --- |
| 货品单位 | FIELD_TYPE_TEXT |
| 货品种类 | FIELD_TYPE_SELECT |
| 存储仓库 | FIELD_TYPE_LOCATION |
| 备注 | FIELD_TYPE_TEXT |
| 货品名称 | FIELD_TYPE_TEXT |
| 供应商 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 总库存金额 | FIELD_TYPE_FORMULA |
| 所属产品行业分类 | FIELD_TYPE_SELECT |
| 货品图片 | FIELD_TYPE_IMAGE |
| 最新销售单价（元） | FIELD_TYPE_NUMBER |
| 负责仓管员 | FIELD_TYPE_USER |
| 规格型号 | FIELD_TYPE_TEXT |
| 最新进货单价（元） | FIELD_TYPE_NUMBER |
| 货品编码 | FIELD_TYPE_TEXT |

### 供应商花名册

| 字段 | 类型 |
| --- | --- |
| 供应商编号 | FIELD_TYPE_TEXT |
| 联系人 | FIELD_TYPE_TEXT |
| 货品 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 平均来货瑕疵率 | FIELD_TYPE_LOOKUP |
| 供应商名称 | FIELD_TYPE_TEXT |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |

### 货品库存管理仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 累计损失总额（¥） | numberCard | [6, 11] | [3, 3] |
| 各仓库货品存储情况 | combo | [8, 5] | [4, 5] |
| 累计出库货品及数量 | bar | [4, 1] | [4, 4] |
| 累计入库货品及数量 | bar | [0, 1] | [4, 4] |
| 当前存货价值 | numberCard | [9, 11] | [3, 3] |
| 当前货品可用库存量情况 | column | [8, 1] | [4, 4] |
| 各货品出货总额 | stackcolumn | [6, 14] | [6, 5] |
| 累计瑕疵货品情况 | line | [0, 5] | [4, 5] |
| 各货品采购成本分布 | doughnut | [0, 14] | [6, 5] |
| 货品出库用途分布 | combo | [4, 5] | [4, 5] |
| 当前库存价值总计 | bar | [0, 19] | [12, 4] |
| 累计销售总额（¥） | numberCard | [3, 11] | [3, 3] |
| 累计采购总额（¥） | numberCard | [0, 11] | [3, 3] |
