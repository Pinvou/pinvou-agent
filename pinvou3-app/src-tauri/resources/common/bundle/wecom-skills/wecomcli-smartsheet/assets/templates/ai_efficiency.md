# AI提效的数据表模版

## 包含表格模版

- **团队日报 AI 总结**：通过 AI 自动汇总团队成员日报内容，生成进展总结，并统计日报提交情况和项目任务分布。
- **用户评价 AI 分析**：利用 AI 对用户评价进行维度打标和满意度分析，自动识别好评/差评，并通过仪表盘展示评价渠道和维度分布。
- **朋友圈文案 AI 生成**：根据产品信息（功效、成分、使用感受等）自动生成朋友圈推广文案，提升营销内容生产效率。
- **拍照巡检 AI 识别**：通过上传现场照片，AI 自动识别巡检问题并生成巡检结果，支持整改状态跟踪和问题分布统计。
- **售后问题 AI 总结**：利用 AI 对售后问题描述进行自动总结，关联客户信息，支持问题分配、跟进状态管理和高频问题词云分析。
- **项目进展 AI 总结**：通过 AI 自动分析项目子任务的进展和风险，生成总结报告，支持项目状态看板和部门周报管理。
- **工作完成情况 AI 复盘**：基于员工填写的本周工作计划和实际进展，由 AI 自动生成完成情况总结，辅助团队复盘。
- **用户反馈 AI 打标签**：利用 AI 对用户反馈进行维度分析和满意度分类，自动生成客服回复话术，并展示反馈趋势和关键词词云。
- **巡检问题 AI 分类**：通过 AI 对巡检问题进行自动分类，关联门店信息，支持各门店问题分布和片区问题统计分析。
- **工单问题 AI 分类**：利用 AI 分析工单异常原因并自动分类工单类型，支持车间问题来源统计和处理时长分析。
- **新媒体内容 AI 选题管理**：管理新媒体内容选题，AI 提供制作建议，支持按发布渠道和内容形式统计选题分布。
- **短视频脚本 AI 生成**：根据短视频创意和主题，由 AI 自动生成短视频脚本，提升内容创作效率。
- **门店营销方案 AI 生成**：基于门店客户画像和运营数据，AI 自动生成产品销售方案，支持全国店铺数据总览和新门店规划管理。
- **直播情况 AI 管理**：管理直播活动策划、产品、排期和复盘全流程，AI 辅助分析风险和优化方案，支持直播情况总览仪表盘。
- **电商选品 AI 管理**：通过 AI 辅助评估选品可行性，管理供应商信息和产品登记，支持选品状态、品类分布和供应商信誉分析。
- **购物小票 AI 提取**：通过上传购物小票图片，AI 自动提取金额、时间、购买分类等信息，简化费用记录流程。
- **身份证号 AI 提取**：通过上传身份证图片，AI 自动识别并提取身份证号码，适用于需要批量录入证件信息的场景。
- **货品状态 AI 解析**：通过 AI 解析货品出入库状态，管理货品库存总表、供应商信息和商品编码，支持库存总览仪表盘。

## 团队日报 AI 总结

### 团队日报汇总

| 字段 | 类型 |
| --- | --- |
| 汇报给 | FIELD_TYPE_USER |
| 今日工作总结 | FIELD_TYPE_TEXT |
| AI 总结进展 | FIELD_TYPE_TEXT |
| 困难及需要的支持 | FIELD_TYPE_TEXT |
| 是否涉及多部门合作 | FIELD_TYPE_CHECKBOX |
| 项目 | FIELD_TYPE_SELECT |
| 提交人 | FIELD_TYPE_SELECT |
| 明日工作计划 | FIELD_TYPE_TEXT |
| 附件 | FIELD_TYPE_ATTACHMENT |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 日报提交日期 | FIELD_TYPE_DATE_TIME |

### 团队成员管理

| 字段 | 类型 |
| --- | --- |
| 资料创建人 | FIELD_TYPE_SELECT |
| 是否提交今日月报 | FIELD_TYPE_FORMULA |
| 部门 | FIELD_TYPE_SELECT |
| 最近修改时间 | FIELD_TYPE_DATE_TIME |
| 是否提交日报 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 工号 | FIELD_TYPE_NUMBER |
| 备注 | FIELD_TYPE_TEXT |

### 日报情况统计（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 团队日报情况 | stackbar | [6, 1] | [6, 3] |
| 项目任务数 | pie | [0, 4] | [12, 4] |
| 今日日报总数 | numberCard | [0, 1] | [6, 3] |

## 用户评价 AI 分析

### 用户评价

| 字段 | 类型 |
| --- | --- |
| 评价维度打标 | FIELD_TYPE_SELECT |
| 评价时间 | FIELD_TYPE_DATE_TIME |
| 颜色 | FIELD_TYPE_SELECT |
| 商品型号 | FIELD_TYPE_SELECT |
| 商品名称 | FIELD_TYPE_SELECT |
| 反馈渠道 | FIELD_TYPE_SELECT |
| 满意度分析 | FIELD_TYPE_SELECT |
| 用户评价 | FIELD_TYPE_TEXT |

### 用户评价看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 用户评价维度分布 | bar | [0, 4] | [6, 4] |
| 差评数 | numberCard | [4, 0] | [3, 4] |
| 用户满意度分布（AI 分析） | pie | [7, 0] | [5, 4] |
| 总评价数 | numberCard | [0, 0] | [4, 4] |
| 评价渠道分布 | column | [6, 4] | [6, 4] |

## 朋友圈文案 AI 生成

### 产品信息表

| 字段 | 类型 |
| --- | --- |
| 适合的肤质 | FIELD_TYPE_SELECT |
| 使用感受 | FIELD_TYPE_TEXT |
| 朋友圈文案（推广用） | FIELD_TYPE_TEXT |
| 主要成分 | FIELD_TYPE_SELECT |
| 其他备注 | FIELD_TYPE_TEXT |
| 主要功效 | FIELD_TYPE_TEXT |
| 产品名称 | FIELD_TYPE_TEXT |

## 拍照巡检 AI 识别

### 巡检记录表

| 字段 | 类型 |
| --- | --- |
| 巡检人员 | FIELD_TYPE_CREATED_USER |
| 巡检日期 | FIELD_TYPE_CREATED_TIME |
| 责任人 | FIELD_TYPE_USER |
| 整改状态 | FIELD_TYPE_SELECT |
| 巡检安全要求 | FIELD_TYPE_LOOKUP |
| 现场拍照 | FIELD_TYPE_IMAGE |
| AI 智能巡检识别 | FIELD_TYPE_TEXT |
| 巡检结果 | FIELD_TYPE_TEXT |
| 巡检项目 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 巡检要求明细表

| 字段 | 类型 |
| --- | --- |
| 关联巡检记录 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 巡检安全要求 | FIELD_TYPE_TEXT |
| 巡检项目 | FIELD_TYPE_TEXT |
| 最后编辑时间 | FIELD_TYPE_MODIFIED_TIME |
| 最后编辑人 | FIELD_TYPE_MODIFIED_USER |

### 巡检问题分布（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 待整改项目责任分属情况 | smoothline | [7, 0] | [5, 4] |
| 总巡检任务数 | numberCard | [0, 0] | [3, 4] |
| 整改情况汇总 | pie | [3, 0] | [4, 4] |

## 售后问题 AI 总结

### 售后问题跟进表

| 字段 | 类型 |
| --- | --- |
| 跟进状态 | FIELD_TYPE_SELECT |
| 反馈日期 | FIELD_TYPE_DATE_TIME |
| AI 问题总结 | FIELD_TYPE_TEXT |
| 问题跟进人 | FIELD_TYPE_USER |
| 问题截图 | FIELD_TYPE_IMAGE |
| 详细问题描述 | FIELD_TYPE_TEXT |
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
| 高频问题（词云） | wordCloud | [7, 3] | [5, 3] |
| 本月 - 反馈问题数 | numberCard | [3, 0] | [2, 3] |
| 总问题数 | numberCard | [0, 0] | [3, 3] |
| 售后问题来源（按客户） | pie | [0, 3] | [3, 3] |
| 本月 - 待解决问题数 | numberCard | [5, 0] | [2, 3] |
| 问题分配情况（按负责人） | bar | [3, 3] | [4, 3] |
| 本月 - 问题跟进情况 | bar | [7, 0] | [5, 3] |

## 项目进展 AI 总结

### 子任务进展 AI 总结

| 字段 | 类型 |
| --- | --- |
| 优先级 | FIELD_TYPE_SELECT |
| 实际完成时间 | FIELD_TYPE_DATE_TIME |
| 任务状态(自动计算) | FIELD_TYPE_FORMULA |
| 负责人 | FIELD_TYPE_USER |
| 所属项目 | FIELD_TYPE_SELECT |
| 任务状态 | FIELD_TYPE_SELECT |
| 讨论群 | FIELD_TYPE_WWGROUP |
| AI 风险总结 | FIELD_TYPE_TEXT |
| AI 进展总结 | FIELD_TYPE_TEXT |
| 关联的项目信息 | FIELD_TYPE_REFERENCE |
| 所属部门 | FIELD_TYPE_SELECT |
| 任务描述 | FIELD_TYPE_TEXT |
| 任务名称 | FIELD_TYPE_TEXT |
| 启动时间 | FIELD_TYPE_DATE_TIME |
| 截止时间 | FIELD_TYPE_DATE_TIME |

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

## 工作完成情况 AI 复盘

### 周工作计划表

| 字段 | 类型 |
| --- | --- |
| 所属部门 | FIELD_TYPE_SELECT |
| AI 完成情况总结 | FIELD_TYPE_TEXT |
| 最后编辑时间 | FIELD_TYPE_MODIFIED_TIME |
| 本周工作计划 | FIELD_TYPE_TEXT |
| 负责人 | FIELD_TYPE_CREATED_USER |
| 实际工作进展 | FIELD_TYPE_TEXT |
| 创建时间 | FIELD_TYPE_CREATED_TIME |

## 用户反馈 AI 打标签

### 用户反馈

| 字段 | 类型 |
| --- | --- |
| AI 反馈维度分析 | FIELD_TYPE_SELECT |
| 反馈日期 | FIELD_TYPE_DATE_TIME |
| 售后跟进人 | FIELD_TYPE_USER |
| 客服回复话术 | FIELD_TYPE_TEXT |
| 反馈渠道 | FIELD_TYPE_SELECT |
| AI 满意度分析 | FIELD_TYPE_SELECT |
| 用户反馈 | FIELD_TYPE_TEXT |

### 反馈情况看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 用户反馈总数 | numberCard | [0, 0] | [3, 3] |
| 好评数 | numberCard | [3, 0] | [3, 3] |
| 用户反馈情感分布 | pie | [4, 3] | [4, 4] |
| 差评数 | numberCard | [6, 0] | [3, 3] |
| 反馈趋势图 | line | [9, 0] | [3, 3] |
| 用户反馈提及维度 | bar | [8, 3] | [4, 4] |
| 用户反馈关键词 | wordCloud | [0, 3] | [4, 4] |

## 巡检问题 AI 分类

### 巡检问题 AI 分类

| 字段 | 类型 |
| --- | --- |
| 片区 | FIELD_TYPE_LOOKUP |
| 店长 | FIELD_TYPE_LOOKUP |
| 发现问题区域 | FIELD_TYPE_SELECT |
| 处理备注 | FIELD_TYPE_TEXT |
| 处理状态 | FIELD_TYPE_SELECT |
| 反馈日期 | FIELD_TYPE_DATE_TIME |
| 处理人 | FIELD_TYPE_USER |
| AI 问题分类 | FIELD_TYPE_SELECT |
| 问题反馈人 | FIELD_TYPE_USER |
| 问题编号 | FIELD_TYPE_AUTONUMBER |
| 问题截图/录像 | FIELD_TYPE_ATTACHMENT |
| 处理日期 | FIELD_TYPE_DATE_TIME |
| 问题描述 | FIELD_TYPE_TEXT |
| 门店名称 | FIELD_TYPE_TWOWAYLINKRECORDS |

### 巡检问题看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各门店问题分布 | bar | [5, 3] | [7, 5] |
| 不同类别问题占比 | doughnut | [0, 3] | [5, 5] |
| （本月）各片区问题一览 | stackcolumn | [7, 0] | [5, 3] |

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

## 工单问题 AI 分类

### 工单问题记录表

| 字段 | 类型 |
| --- | --- |
| 发现时间 | FIELD_TYPE_DATE_TIME |
| 发现车间 | FIELD_TYPE_SELECT |
| 处理人 | FIELD_TYPE_USER |
| 处理状态 | FIELD_TYPE_SELECT |
| AI 分析异常原因 | FIELD_TYPE_TEXT |
| 处理时间 | FIELD_TYPE_DATE_TIME |
| 工单类型 (AI 分类) | FIELD_TYPE_SELECT |
| 处理时长 | FIELD_TYPE_FORMULA |
| 发现人 | FIELD_TYPE_USER |
| 处理回复 | FIELD_TYPE_TEXT |
| 详细问题描述 | FIELD_TYPE_TEXT |
| 紧急程度 | FIELD_TYPE_SELECT |
| 工单号 | FIELD_TYPE_AUTONUMBER |

### 异常问题看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本月待处理问题数 | numberCard | [4, 0] | [2, 3] |
| 本月已解决问题数 | numberCard | [6, 0] | [2, 3] |
| 问题来源（按车间） | doughnut | [4, 3] | [4, 3] |
| 问题平均处理时长 | numberCard | [8, 3] | [4, 3] |
| 本月异常问题数 | numberCard | [0, 0] | [4, 3] |
| 本月异常问题处理情况 | pie | [8, 0] | [4, 3] |
| 异常问题的类型分布 | bar | [0, 3] | [4, 3] |

## 新媒体内容 AI 选题管理

### 内容选题

| 字段 | 类型 |
| --- | --- |
| 负责人 | FIELD_TYPE_USER |
| 发布及推流日期 | FIELD_TYPE_DATE_TIME |
| AI 制作建议 | FIELD_TYPE_TEXT |
| 内容状态 | FIELD_TYPE_SELECT |
| 内容形式 | FIELD_TYPE_SELECT |
| 推流结束日期 | FIELD_TYPE_DATE_TIME |
| 发布渠道 | FIELD_TYPE_SELECT |
| 内容主题 | FIELD_TYPE_TEXT |

### 内容状态概览（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 发布渠道统计 | bar | [6, 3] | [6, 5] |
| 内容类型分布 | pie | [0, 3] | [6, 5] |

## 短视频脚本 AI 生成

### 短视频脚本 AI 生成

| 字段 | 类型 |
| --- | --- |
| AI 生成脚本 | FIELD_TYPE_TEXT |
| 短视频脚本创意 | FIELD_TYPE_TEXT |
| 视频主题 | FIELD_TYPE_SELECT |

## 门店营销方案 AI 生成

### 门店客户管理

| 字段 | 类型 |
| --- | --- |
| 产品销售方案 | FIELD_TYPE_TEXT |
| 门店 | FIELD_TYPE_TEXT |
| 主要年龄段 | FIELD_TYPE_SELECT |
| TOP3复购产品 | FIELD_TYPE_TEXT |
| 留存率 | FIELD_TYPE_PERCENTAGE |
| 会员数量 | FIELD_TYPE_NUMBER |
| 复购率 | FIELD_TYPE_PERCENTAGE |
| 经营状态 | FIELD_TYPE_SELECT |
| 店铺门面 | FIELD_TYPE_IMAGE |
| 门店产品类型 | FIELD_TYPE_SELECT |
| 开店日期 | FIELD_TYPE_DATE_TIME |
| 门店负责人 | FIELD_TYPE_USER |
| 大区 | FIELD_TYPE_SELECT |
| 地理位置 | FIELD_TYPE_LOCATION |
| 门店定位 | FIELD_TYPE_SELECT |
| 大区负责人 | FIELD_TYPE_USER |
| 所在区域 | FIELD_TYPE_TEXT |

### 门店运营数据

| 字段 | 类型 |
| --- | --- |
| 服务评价分 | FIELD_TYPE_NUMBER |
| 季度 | FIELD_TYPE_SELECT |
| 销售额-万 | FIELD_TYPE_FORMULA |
| 门店 | FIELD_TYPE_TEXT |
| 销售额 | FIELD_TYPE_CURRENCY |
| 人均销售额（万） | FIELD_TYPE_FORMULA |
| 在职员工数量 | FIELD_TYPE_NUMBER |
| 所在大区 | FIELD_TYPE_LOOKUP |

### 新门店规划

| 字段 | 类型 |
| --- | --- |
| 规划进度 | FIELD_TYPE_SELECT |
| 具体规划方案 | FIELD_TYPE_ATTACHMENT |
| 门店定位 | FIELD_TYPE_SELECT |
| 门店名称 | FIELD_TYPE_TEXT |
| 拟选产品类型 | FIELD_TYPE_SELECT |
| 登记人 | FIELD_TYPE_USER |

### 全国店铺数据总览（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各门店季度销售额对比情况（万） | line | [4, 7] | [8, 3] |
| 店铺状态 | pie | [0, 3] | [4, 3] |
| 当前总店铺数 | numberCard | [0, 1] | [4, 2] |
| 总销售额（万） | numberCard | [0, 7] | [4, 3] |
| 各大区营业额（万） | combo | [0, 10] | [12, 3] |
| 各大区店铺数 | bar | [4, 1] | [8, 5] |
| 新门店规划进度 | pie | [5, 14] | [7, 3] |
| 新门店定位及产品类型情况 | bar | [0, 17] | [12, 4] |
| 新门店规划总数 | numberCard | [0, 14] | [5, 3] |

## 直播情况 AI 管理

### 直播活动方案管理

| 字段 | 类型 |
| --- | --- |
| 当前进度 | FIELD_TYPE_SELECT |
| 最佳直播启动节点 | FIELD_TYPE_DATE_TIME |
| 最后编辑时间 | FIELD_TYPE_MODIFIED_TIME |
| 活动预算 | FIELD_TYPE_TEXT |
| 活动目标 | FIELD_TYPE_TEXT |
| 选品 | FIELD_TYPE_TEXT |
| 最终活动方案 | FIELD_TYPE_ATTACHMENT |
| 风险点 | FIELD_TYPE_TEXT |
| 活动方案 | FIELD_TYPE_TEXT |
| 活动主题 | FIELD_TYPE_TEXT |

### 直播产品管理

| 字段 | 类型 |
| --- | --- |
| 直播话术 | FIELD_TYPE_TEXT |
| 类目 | FIELD_TYPE_SELECT |
| 产品图片 | FIELD_TYPE_IMAGE |
| 产品活动价 | FIELD_TYPE_CURRENCY |
| 直播产品 | FIELD_TYPE_TEXT |
| 产品原价 | FIELD_TYPE_CURRENCY |
| 产品亮点 | FIELD_TYPE_TEXT |

### 直播排期管理

| 字段 | 类型 |
| --- | --- |
| 直播平台 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 主播 | FIELD_TYPE_USER |
| 直播产品 | FIELD_TYPE_TEXT |
| 结束时间 | FIELD_TYPE_DATE_TIME |
| 直播间布置策略 | FIELD_TYPE_TEXT |
| 直播号 | FIELD_TYPE_TEXT |
| 是否已直播结束 | FIELD_TYPE_CHECKBOX |
| 直播开始时间 | FIELD_TYPE_DATE_TIME |
| 活动名称 | FIELD_TYPE_TEXT |

### 直播复盘

| 字段 | 类型 |
| --- | --- |
| 活动商品 | FIELD_TYPE_LOOKUP |
| 数据复盘 | FIELD_TYPE_TEXT |
| 风险处理方案留存 | FIELD_TYPE_TEXT |
| 直播期间是否出现风险点 | FIELD_TYPE_SELECT |
| 直播成交件数 | FIELD_TYPE_FORMULA |
| 优化调整 | FIELD_TYPE_TEXT |
| 成交金额 | FIELD_TYPE_TEXT |
| 成交件数 | FIELD_TYPE_TEXT |
| 直播数据图 | FIELD_TYPE_IMAGE |
| 风险描述及现场处理方案 | FIELD_TYPE_TEXT |
| 直播数据提取 | FIELD_TYPE_TEXT |
| 活动成交金额 | FIELD_TYPE_FORMULA |
| 开播时长（分钟） | FIELD_TYPE_TEXT |
| 活动名称 | FIELD_TYPE_TEXT |
| 处理结果 | FIELD_TYPE_TEXT |

### 直播平台管理

| 字段 | 类型 |
| --- | --- |
| 常规直播风格 | FIELD_TYPE_TEXT |
| 运营人员 | FIELD_TYPE_USER |
| 粉丝量 | FIELD_TYPE_NUMBER |
| 历史直播场次 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 直播平台 | FIELD_TYPE_TEXT |
| 直播号名称 | FIELD_TYPE_TEXT |

### 直播情况总览（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 累计成交金额（￥） | numberCard | [0, 1] | [7, 3] |
| 现存活动策划数量 | numberCard | [0, 10] | [3, 4] |
| 风险描述及处理方案关键词 | wordCloud | [3, 4] | [4, 5] |
| 累计成交件数 | numberCard | [7, 1] | [5, 3] |
| 现有产品亮点关键词 | wordCloud | [3, 15] | [4, 5] |
| 各直播平台直播频次 | pie | [7, 15] | [3, 5] |
| 风险出现频率 | pie | [0, 4] | [3, 5] |
| 直播活动风险处理情况 | bar | [7, 4] | [5, 5] |
| 方案关键词 | wordCloud | [3, 10] | [4, 4] |
| 现有产品类目分布情况 | pie | [0, 15] | [3, 5] |
| 主播直播场次情况 | column | [10, 15] | [2, 5] |
| 活动策划进度分布情况 | bar | [7, 10] | [5, 4] |

## 电商选品 AI 管理

### 选品管理

| 字段 | 类型 |
| --- | --- |
| 合作可行性 | FIELD_TYPE_TEXT |
| 商品体积类型 | FIELD_TYPE_SELECT |
| 包装规格 | FIELD_TYPE_LOOKUP |
| 产品名称 | FIELD_TYPE_TEXT |
| 是否无产品质量证书 | FIELD_TYPE_FORMULA |
| 产品图片 | FIELD_TYPE_LOOKUP |
| 所属类目 | FIELD_TYPE_LOOKUP |
| 供应商 | FIELD_TYPE_REFERENCE |
| 选品结果 | FIELD_TYPE_SELECT |
| 商品差评标签 | FIELD_TYPE_SELECT |
| 商品类型 | FIELD_TYPE_SELECT |
| 商品是否存在侵权争议 | FIELD_TYPE_SELECT |
| 产品质量证书 | FIELD_TYPE_LOOKUP |
| 商品SKU | FIELD_TYPE_AUTONUMBER |

### 选品登记及汇总

| 字段 | 类型 |
| --- | --- |
| 产品质量证书 | FIELD_TYPE_ATTACHMENT |
| 产品图片 | FIELD_TYPE_IMAGE |
| 起订量（件） | FIELD_TYPE_NUMBER |
| 是否有相关质检证书 | FIELD_TYPE_SELECT |
| 该产品是否拥有权利证书 | FIELD_TYPE_SELECT |
| 三级类目 | FIELD_TYPE_SELECT |
| 产品优势 | FIELD_TYPE_TEXT |
| 企业完整名称 | FIELD_TYPE_TEXT |
| 交货周期（天） | FIELD_TYPE_NUMBER |
| 登记日期 | FIELD_TYPE_DATE_TIME |
| 合作价（元） | FIELD_TYPE_CURRENCY |
| 产品卖点 | FIELD_TYPE_TEXT |
| 产品权利证书 | FIELD_TYPE_IMAGE |
| 二级类目 | FIELD_TYPE_SELECT |
| 关联 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 包装类型 | FIELD_TYPE_SELECT |
| 所属品牌 | FIELD_TYPE_TEXT |
| 联系方式 | FIELD_TYPE_PHONE_NUMBER |
| 产品规格 | FIELD_TYPE_TEXT |
| 产品名称 | FIELD_TYPE_TEXT |
| 常规价（元） | FIELD_TYPE_CURRENCY |
| 联系人 | FIELD_TYPE_TEXT |
| 一级类目 | FIELD_TYPE_SELECT |
| 填写者 | FIELD_TYPE_CREATED_USER |

### 供应商汇总

| 字段 | 类型 |
| --- | --- |
| 所属品牌 | FIELD_TYPE_LOOKUP |
| 企业完整名称 | FIELD_TYPE_LOOKUP |
| 供应商 | FIELD_TYPE_TEXT |
| 产品名称 | FIELD_TYPE_TEXT |
| 联系人 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 是否存在侵权争议 | FIELD_TYPE_LOOKUP |
| 供应商信誉等级 | FIELD_TYPE_SELECT |
| 备注 | FIELD_TYPE_TEXT |

### 选品看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 选品状态分布 | doughnut | [0, 10] | [6, 4] |
| 通过选品的商品类型 | pie | [6, 10] | [6, 4] |
| 各供应商信誉等级分布 | line | [6, 1] | [6, 3] |
| 品类登记分布（三级类目） | pie | [0, 5] | [6, 5] |
| 风险供应商 | numberCard | [3, 1] | [3, 3] |
| 已登记供应商 | numberCard | [0, 1] | [3, 3] |
| 仓储占用体积分布 | bar | [6, 5] | [6, 5] |

## 购物小票 AI 提取

### 购物小票 AI 提取

| 字段 | 类型 |
| --- | --- |
| 服装费用 | FIELD_TYPE_NUMBER |
| 购买分类 | FIELD_TYPE_SELECT |
| 小票信息识别 | FIELD_TYPE_TEXT |
| 开票时间 | FIELD_TYPE_DATE_TIME |
| 餐饮费用 | FIELD_TYPE_NUMBER |
| 购物小票 | FIELD_TYPE_IMAGE |
| 实付金额(元) | FIELD_TYPE_TEXT |

## 身份证号 AI 提取

### 身份证号 AI 提取

| 字段 | 类型 |
| --- | --- |
| 身份证号码 | FIELD_TYPE_TEXT |
| 身份证 | FIELD_TYPE_IMAGE |
| 身份证识别 | FIELD_TYPE_TEXT |

## 货品状态 AI 解析

### 出入库登记表

| 字段 | 类型 |
| --- | --- |
| 一级分类 | FIELD_TYPE_SELECT |
| 出/入库数量 | FIELD_TYPE_NUMBER |
| 所属供应商 | FIELD_TYPE_LOOKUP |
| 内部对接人 | FIELD_TYPE_LOOKUP |
| 经手人（仓管员） | FIELD_TYPE_USER |
| 二级分类 | FIELD_TYPE_SELECT |
| 货品名称 | FIELD_TYPE_LOOKUP |
| 出/入库位置 | FIELD_TYPE_LOCATION |
| 是否存在异常 | FIELD_TYPE_SELECT |
| AI解析货品状态 | FIELD_TYPE_TEXT |
| 货品编码 | FIELD_TYPE_BARCODE |
| 出/入库时间 | FIELD_TYPE_DATE_TIME |
| 货品图片 | FIELD_TYPE_IMAGE |
| 出/入库 | FIELD_TYPE_SELECT |

### 货品库存总表

| 字段 | 类型 |
| --- | --- |
| 一级分类 | FIELD_TYPE_SELECT |
| 二级分类 | FIELD_TYPE_SELECT |
| 成本价 | FIELD_TYPE_CURRENCY |
| 出库总数 | FIELD_TYPE_LOOKUP |
| 仓管员 | FIELD_TYPE_USER |
| 销售单价 | FIELD_TYPE_CURRENCY |
| 入库总数 | FIELD_TYPE_LOOKUP |
| 所属供应商 | FIELD_TYPE_LOOKUP |
| 货品名称 | FIELD_TYPE_LOOKUP |
| 最新数据更新时间 | FIELD_TYPE_MODIFIED_TIME |
| 库存总价值 | FIELD_TYPE_FORMULA |
| 内部对接人 | FIELD_TYPE_LOOKUP |
| 历史剩余库存数量 | FIELD_TYPE_NUMBER |
| 货品规格 | FIELD_TYPE_TEXT |
| 存储位置 | FIELD_TYPE_LOCATION |
| 现存库存数量 | FIELD_TYPE_FORMULA |
| 货品编码 | FIELD_TYPE_TEXT |

### 供应商管理表

| 字段 | 类型 |
| --- | --- |
| 联系方式 | FIELD_TYPE_PHONE_NUMBER |
| 供应商名称 | FIELD_TYPE_TEXT |
| 出入库记录 | FIELD_TYPE_REFERENCE |
| 所在城市 | FIELD_TYPE_FORMULA |
| 内部对接人 | FIELD_TYPE_USER |
| 累计采购数量 | FIELD_TYPE_LOOKUP |
| 具体地址 | FIELD_TYPE_LOCATION |
| 联系人 | FIELD_TYPE_TEXT |

### 商品编码目录

| 字段 | 类型 |
| --- | --- |
| 货品编码 | FIELD_TYPE_TEXT |
| 货品样图 | FIELD_TYPE_IMAGE |
| 货品名称 | FIELD_TYPE_TEXT |
| 供应商 | FIELD_TYPE_TEXT |

### 库存总览（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各供应商货品历史来货情况 | bar | [5, 3] | [7, 4] |
| 累计出库数量 | numberCard | [9, 0] | [3, 3] |
| 出入库产品情况 | bar | [0, 3] | [5, 4] |
| 各品类成本价与单价的对比 | combo | [0, 7] | [12, 5] |
| 累计入库数量 | numberCard | [5, 0] | [4, 3] |
| 当前总库存数 | numberCard | [0, 0] | [5, 3] |
