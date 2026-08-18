# 生产制造的数据表模版

## 包含表格模版

- **车间生产日报**：记录各批次各工序的每日实际产量和预期产量，自动计算完成度，支持计划与实际产量对比分析。
- **车间现场巡检**：管理车间每日巡检记录，记录检查地点、问题类别及整改进度，支持本月问题总数和每日问题数统计。
- **设备维护点检**：记录设备点检结果和整改进度，支持合格/不合格点检记录统计和各设备点检结果汇总。
- **生产计划表**：管理生产工单，记录物料编号、产品品类、计划产量、生产车间及交付日期，支持各车间任务分布和计划产量统计。
- **生产进度管理**：多工序生产进度管理，记录各批次各工序的每日产量，自动计算生产总进度，支持批次进度和工序完成度分析。
- **异常问题记录**：记录车间异常问题，包含异常类型、发现车间、处理状态及处理时长，支持问题类型分布和平均处理时长统计。
- **生产研发管理**：管理研发流程各阶段，记录责任部门、参与部门、开始/完成时间及成果，统计各研发阶段和各部门参与周期。
- **样品登记表**：管理样品检测全流程，记录样品名称、送检单位、检测结果及有效期，支持样品状态和检测结果分布统计。
- **不合格品统计**：通过质检任务下发和每日不良上报，自动计算订单不良率，支持每日不良原因走势和各订单不良率分析。
- **来料质检记录**：管理来料质检记录，关联 BOM 物料清单和供应商信息，支持质检结果统计和供应商供货质量分析。

## 车间生产日报

### 产能盘点（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 平均生产进度 | numberCard | [8, 0] | [4, 3] |
| 【分批次】计划&实际产量 | bar | [0, 3] | [6, 3] |
| 计划总产量 | numberCard | [0, 0] | [4, 3] |
| 【分工序】平均生产进度 | smoothline | [0, 6] | [6, 3] |
| 【分批次】【分工序】生产进度 | stackcolumn | [6, 6] | [6, 3] |
| 【分工序】计划&实际产量 | bar | [6, 3] | [6, 3] |
| 实际总产量 | numberCard | [4, 0] | [4, 3] |

### 生产日报

| 字段 | 类型 |
| --- | --- |
| 生产批次 | FIELD_TYPE_TEXT |
| 工序 | FIELD_TYPE_SELECT |
| 登记人 | FIELD_TYPE_CREATED_USER |
| 今日实际产量 | FIELD_TYPE_NUMBER |
| 今日预期产量 | FIELD_TYPE_NUMBER |
| 今日完成度 | FIELD_TYPE_FORMULA |
| 生产日期 | FIELD_TYPE_DATE_TIME |

## 车间现场巡检

### 每日巡检记录

| 字段 | 类型 |
| --- | --- |
| 填写人（自动生成 | FIELD_TYPE_CREATED_USER |
| 具体问题描述 | FIELD_TYPE_TEXT |
| 整改责任人（可填多人 | FIELD_TYPE_USER |
| 整改进度 | FIELD_TYPE_SELECT |
| 检查地点 | FIELD_TYPE_SELECT |
| 日期 | FIELD_TYPE_DATE_TIME |
| 现场照片 | FIELD_TYPE_IMAGE |
| 问题类别 | FIELD_TYPE_SELECT |
| 有无问题 | FIELD_TYPE_SELECT |
| 检查时间（自动生成 | FIELD_TYPE_CREATED_TIME |
| 整改完拍照 | FIELD_TYPE_IMAGE |

### 本月巡检情况看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本月问题总数 | numberCard | [4, 1] | [4, 2] |
| 本月巡检记录总数 | numberCard | [0, 1] | [4, 2] |
| 本月各检查地点出问题比例 | pie | [0, 3] | [4, 4] |
| 每日问题数 | bar | [4, 3] | [8, 4] |
| 未整改问题数 | numberCard | [8, 1] | [4, 2] |

## 设备维护点检

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 合格点检记录数 | numberCard | [6, 0] | [3, 3] |
| 未整改问题数 | numberCard | [3, 0] | [3, 3] |
| 本月点检记录数 | numberCard | [9, 0] | [3, 3] |
| 点检记录总数 | numberCard | [0, 0] | [3, 3] |
| 各设备点检结果 | stackbar | [0, 3] | [6, 5] |
| 不合格点检记录汇总 | table | [6, 3] | [6, 5] |

### 点检登记

| 字段 | 类型 |
| --- | --- |
| 点检人员 | FIELD_TYPE_CREATED_USER |
| 跟进维护人 | FIELD_TYPE_USER |
| 整改进度 | FIELD_TYPE_SELECT |
| 设备名称 | FIELD_TYPE_TEXT |
| 检查结果 | FIELD_TYPE_SELECT |
| 现场照片 | FIELD_TYPE_IMAGE |
| 设备具体情况 | FIELD_TYPE_TEXT |
| 检查时间（自动生成） | FIELD_TYPE_CREATED_TIME |
| 整改完拍照 | FIELD_TYPE_IMAGE |

## 生产计划表

### 生产计划表

| 字段 | 类型 |
| --- | --- |
| 生产主管 | FIELD_TYPE_USER |
| 物料编号 | FIELD_TYPE_TEXT |
| 产品品类 | FIELD_TYPE_SELECT |
| 计划开工日期 | FIELD_TYPE_DATE_TIME |
| 交付日期 | FIELD_TYPE_DATE_TIME |
| 物料描述 | FIELD_TYPE_TEXT |
| 计划产量 (pcs) | FIELD_TYPE_NUMBER |
| 生产车间 | FIELD_TYPE_SELECT |
| 生产项目群 | FIELD_TYPE_WWGROUP |
| 工单编号 | FIELD_TYPE_AUTONUMBER |

### 生产计划仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 生产主管任务看板 | column | [8, 0] | [4, 3] |
| 各车间生产任务分布 | bar | [8, 3] | [4, 3] |
| 预期交付时间（按物料） | line | [0, 3] | [4, 3] |
| 各物料计划产量 | bar | [4, 0] | [4, 3] |
| 各品类计划产量 | bar | [0, 0] | [4, 3] |
| 计划开工日期 | bar | [4, 3] | [4, 3] |

## 生产进度管理

### 8月进度总览

| 字段 | 类型 |
| --- | --- |
| 是否完成生产 | FIELD_TYPE_FORMULA |
| 工序1进度 | FIELD_TYPE_FORMULA |
| 工序2进度 | FIELD_TYPE_FORMULA |
| 工序3当前总产量 | FIELD_TYPE_LOOKUP |
| 工序1-当前总产量 | FIELD_TYPE_LOOKUP |
| 当前总产量 | FIELD_TYPE_FORMULA |
| 预期生产总量 | FIELD_TYPE_NUMBER |
| 生产总进度 | FIELD_TYPE_FORMULA |
| 批次号 | FIELD_TYPE_TEXT |
| 工序2-当前总产量 | FIELD_TYPE_LOOKUP |
| 工序3进度 | FIELD_TYPE_FORMULA |
| 计划完成日期 | FIELD_TYPE_DATE_TIME |
| 产品 | FIELD_TYPE_SELECT |

### 工序1生产日报

| 字段 | 类型 |
| --- | --- |
| 关联生产批次 | FIELD_TYPE_REFERENCE |
| 批次号-自动填写 | FIELD_TYPE_LOOKUP |
| 今日实际产量 | FIELD_TYPE_NUMBER |
| 备注 | FIELD_TYPE_TEXT |
| 产品-自动填写 | FIELD_TYPE_LOOKUP |
| 预期生产总量 | FIELD_TYPE_LOOKUP |
| 今日预期产量 | FIELD_TYPE_NUMBER |
| 今日完成度 | FIELD_TYPE_FORMULA |
| 生产日期 | FIELD_TYPE_DATE_TIME |

### 工序2生产日报

| 字段 | 类型 |
| --- | --- |
| 今日实际产量 | FIELD_TYPE_NUMBER |
| 备注 | FIELD_TYPE_TEXT |
| 产品-自动填写 | FIELD_TYPE_LOOKUP |
| 预期生产总量 | FIELD_TYPE_LOOKUP |
| 今日预期产量 | FIELD_TYPE_NUMBER |
| 关联生产批次 | FIELD_TYPE_REFERENCE |
| 今日完成度 | FIELD_TYPE_FORMULA |
| 批次号-自动填写 | FIELD_TYPE_LOOKUP |
| 生产日期 | FIELD_TYPE_DATE_TIME |

### 工序3生产日报

| 字段 | 类型 |
| --- | --- |
| 批次号-自动填写 | FIELD_TYPE_LOOKUP |
| 今日完成度 | FIELD_TYPE_FORMULA |
| 关联生产批次 | FIELD_TYPE_REFERENCE |
| 今日预期产量 | FIELD_TYPE_NUMBER |
| 产品-自动填写 | FIELD_TYPE_LOOKUP |
| 预期生产总量 | FIELD_TYPE_LOOKUP |
| 今日实际产量 | FIELD_TYPE_NUMBER |
| 备注 | FIELD_TYPE_TEXT |
| 生产日期 | FIELD_TYPE_DATE_TIME |

### 产量盘点报表（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各批次生产进度 | combo | [0, 3] | [6, 3] |
| 工序2平均生产计划完成度 | line | [4, 8] | [4, 2] |
| 工序3平均生产计划完成度 | line | [8, 8] | [4, 2] |
| 工序1今日总产量 | numberCard | [0, 6] | [4, 2] |
| 工序2今日总产量 | numberCard | [4, 6] | [4, 2] |
| 工序1平均生产计划完成度 | line | [0, 8] | [4, 2] |
| 工序3今日总产量 | numberCard | [8, 6] | [4, 2] |
| 批次各工序进度 | stackbar | [6, 3] | [6, 3] |

## 异常问题记录

### 异常问题记录表

| 字段 | 类型 |
| --- | --- |
| 发现时间 | FIELD_TYPE_DATE_TIME |
| 发现车间 | FIELD_TYPE_SELECT |
| 处理人 | FIELD_TYPE_USER |
| 处理状态 | FIELD_TYPE_SELECT |
| 处理时间 | FIELD_TYPE_DATE_TIME |
| 异常类型 | FIELD_TYPE_SELECT |
| 处理时长 | FIELD_TYPE_FORMULA |
| 发现人 | FIELD_TYPE_USER |
| 处理回复 | FIELD_TYPE_TEXT |
| 异常描述 | FIELD_TYPE_TEXT |
| 异常工单号 | FIELD_TYPE_AUTONUMBER |

### 异常问题看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本月异常问题数 | numberCard | [0, 0] | [4, 3] |
| 本月异常问题处理情况 | pie | [8, 0] | [4, 3] |
| 异常问题的类型分布 | bar | [0, 3] | [4, 3] |
| 本月待处理问题数 | numberCard | [4, 0] | [2, 3] |
| 本月已解决问题数 | numberCard | [6, 0] | [2, 3] |
| 问题来源（按车间） | doughnut | [4, 3] | [4, 3] |
| 问题平均处理时长 | numberCard | [8, 3] | [4, 3] |

## 生产研发管理

### 生产研发流程

| 字段 | 类型 |
| --- | --- |
| (预计)完成时间 | FIELD_TYPE_DATE_TIME |
| 成果 | FIELD_TYPE_TEXT |
| 周期 (工作日数) | FIELD_TYPE_FORMULA |
| 总周期（从开始到结束的工作日数） | FIELD_TYPE_FORMULA |
| 责任人 | FIELD_TYPE_USER |
| 研发流程 | FIELD_TYPE_TEXT |
| 责任部门 | FIELD_TYPE_SELECT |
| 参与部门 | FIELD_TYPE_SELECT |
| 开始时间 | FIELD_TYPE_DATE_TIME |
| 研发阶段 | FIELD_TYPE_SELECT |

### 研发流程看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各研发阶段所需周期（天） | stackbar | [4, 0] | [4, 5] |
| 各部门参与周期（天） | doughnut | [8, 0] | [4, 5] |

## 样品登记表

### 样品管理表

| 字段 | 类型 |
| --- | --- |
| 样品状态 | FIELD_TYPE_SELECT |
| 接收日期 | FIELD_TYPE_DATE_TIME |
| 过期情况 | FIELD_TYPE_FORMULA |
| 样品名称 | FIELD_TYPE_TEXT |
| 送检单位 | FIELD_TYPE_TEXT |
| 检测报告 | FIELD_TYPE_ATTACHMENT |
| 样品类型 | FIELD_TYPE_SELECT |
| 检测完成日期 | FIELD_TYPE_DATE_TIME |
| 检测结果 | FIELD_TYPE_SELECT |
| 检测人 | FIELD_TYPE_USER |
| 有效期 | FIELD_TYPE_DATE_TIME |
| 样品编号 | FIELD_TYPE_AUTONUMBER |

### 样品信息看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 已检测样品数 | numberCard | [4, 0] | [4, 3] |
| 样品状态（按品类） | bar | [0, 3] | [4, 3] |
| 检测结果 | doughnut | [4, 3] | [4, 3] |
| 样品总数 | numberCard | [0, 0] | [4, 3] |
| 样品接收时间 | line | [8, 3] | [4, 3] |
| 待检测样品数 | numberCard | [8, 0] | [4, 3] |

## 不合格品统计

### 11月不良生产监测（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本月不良原因占比 | doughnut | [6, 1] | [6, 4] |
| 今日不良原因占比 | doughnut | [6, 5] | [6, 4] |
| 每日不良原因走势图 | smoothline | [0, 14] | [12, 3] |
| 每日总不良率走势 | smoothline | [0, 9] | [12, 2] |
| 每日不良数量波动 | stackbar | [0, 11] | [12, 3] |
| 每日各订单不良率 | combo | [0, 17] | [12, 3] |

### 质检任务下发（质检任务下发员填）

| 字段 | 类型 |
| --- | --- |
| 质检任务派发日期 | FIELD_TYPE_DATE_TIME |
| 订单不良总数（自动计算） | FIELD_TYPE_LOOKUP |
| 待检订单号 | FIELD_TYPE_BARCODE |
| 订单不良率（自动计算） | FIELD_TYPE_FORMULA |
| 质检任务编号 | FIELD_TYPE_TEXT |
| 待检查总数 | FIELD_TYPE_NUMBER |
| 质检任务完成时间（自动引用） | FIELD_TYPE_LOOKUP |
| 关联质检单 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 每日不良上报（质检员填）

| 字段 | 类型 |
| --- | --- |
| 质检员（自动填写 | FIELD_TYPE_CREATED_USER |
| 质检日期 | FIELD_TYPE_DATE_TIME |
| 对应质检任务 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 订单不良率（自动计算 | FIELD_TYPE_FORMULA |
| 不良原因（质检员填 | FIELD_TYPE_SELECT |
| 订单号（自动填写 | FIELD_TYPE_LOOKUP |
| 对应不良数量（质检员填 | FIELD_TYPE_NUMBER |
| 总检查量（自动填写 | FIELD_TYPE_LOOKUP |

### 11月每日不良率（自动计算）

| 字段 | 类型 |
| --- | --- |
| 日期 | FIELD_TYPE_DATE_TIME |
| 今日检查总数 | FIELD_TYPE_LOOKUP |
| 今日总不良率 | FIELD_TYPE_FORMULA |
| 今日不良总数 | FIELD_TYPE_LOOKUP |

### 11月总不良率（自动计算）

| 字段 | 类型 |
| --- | --- |
| 11月不良率 | FIELD_TYPE_FORMULA |

## 来料质检记录

### 质检记录

| 字段 | 类型 |
| --- | --- |
| 物料编号 | FIELD_TYPE_REFERENCE |
| 批次数量 | FIELD_TYPE_NUMBER |
| 物料类型 | FIELD_TYPE_LOOKUP |
| 检查结果 | FIELD_TYPE_SELECT |
| 不合格项 | FIELD_TYPE_TEXT |
| 处理意见 | FIELD_TYPE_TEXT |
| 检查员 | FIELD_TYPE_USER |
| 单位 | FIELD_TYPE_LOOKUP |
| 检查日期 | FIELD_TYPE_CREATED_TIME |
| 规格 | FIELD_TYPE_LOOKUP |
| 供应商名称 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 物料名称 | FIELD_TYPE_LOOKUP |
| 质检编号 | FIELD_TYPE_AUTONUMBER |

### BOM 物料清单

| 字段 | 类型 |
| --- | --- |
| 是否库存不足 | FIELD_TYPE_FORMULA |
| 物料类型 | FIELD_TYPE_SELECT |
| 单位 | FIELD_TYPE_TEXT |
| 物料名称 | FIELD_TYPE_TEXT |
| 通过质检的物料数 | FIELD_TYPE_FORMULA |
| 规格 | FIELD_TYPE_TEXT |
| 安全库存量 | FIELD_TYPE_NUMBER |
| 物料编号 | FIELD_TYPE_BARCODE |

### 供应商信息

| 字段 | 类型 |
| --- | --- |
| 交易次数 | FIELD_TYPE_NUMBER |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 建联时间 | FIELD_TYPE_DATE_TIME |
| 供应商编号 | FIELD_TYPE_TEXT |
| 供应商名称 | FIELD_TYPE_TEXT |
| 联系人 | FIELD_TYPE_USER |
| 地址 | FIELD_TYPE_TEXT |
| 信誉等级 | FIELD_TYPE_SELECT |

### 来料统计看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 不合格总记录数 | numberCard | [8, 0] | [4, 3] |
| 质检总数 | numberCard | [0, 0] | [4, 3] |
| 合格总记录数 | numberCard | [4, 0] | [4, 3] |
| 质检结果 | table | [6, 3] | [6, 3] |
| 各物料不合格数量 | stackbar | [6, 6] | [6, 3] |
| 供应商供货质量 | stackbar | [0, 6] | [6, 3] |
| 物料类型分布 | doughnut | [0, 3] | [6, 3] |
