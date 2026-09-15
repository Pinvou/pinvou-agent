# 人事行政的数据表模版

## 包含表格模版

- **OKR制定和复盘**：管理团队 OKR 目标和关键结果，记录完成度、负责人及优先级，支持各部门平均完成度统计和低完成度 KR 预警。
- **计件工资管理**：记录员工计件工作数量和单价，自动计算工资，支持按计件类型的工资分布和每日工资情况统计。
- **招聘进度管理**：管理候选人招聘全流程，记录面试状态、面试官、能力标签及教育背景，支持各部门应聘人数和待面试人数统计。
- **员工休假情况收集**：通过表单收集员工休假申请，自动计算休假天数，支持各部门请假天数统计和请假申请回收数分析。
- **员工信息登记表**：管理员工基本信息，记录学历、部门、联系方式、银行卡等信息，支持员工民族、学历和户籍来源分布统计。
- **会议室管理**：管理会议室预约申请和审批，记录会议主题、参会人数、设备需求及使用时长，支持预约审批情况统计。
- **活动签到表**：通过表单收集活动签到信息，自动对比预计名单，统计已签到/未签到人数、用餐需求及部门分布。
- **员工满意度调研**：通过多维度问卷收集员工满意度，AI 分析各项内容平均满意度，支持待改进内容分布和员工建议词云展示。
- **会议记录管理**：记录会议时间、议题、参会人及摘要，支持会议类别统计、月份分布和议题词云分析。
- **员工绩效考核**：支持自评、互评和直属领导评分三维度绩效考核，自动汇总最终绩效等级，支持各部门平均分对比和未完成评选预警。
- **员工薪资计算**：自动计算员工月度薪资，涵盖基础工资、绩效、加班费、五险及各类扣款，支持各部门薪资支出和人效比统计。

## OKR制定和复盘

### KR 情况仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 完成度低于 50% KR 情况 | bar | [6, 3] | [6, 5] |
| 人力部平均完成度 | numberCard | [0, 0] | [3, 3] |
| 直播部平均完成度 | numberCard | [6, 0] | [3, 3] |
| 市场部平均完成度 | numberCard | [9, 0] | [3, 3] |
| 各 KR 完成进度统计 | stackcolumn | [0, 3] | [6, 5] |
| 行政部平均完成度 | numberCard | [3, 0] | [3, 3] |

### KR关键结果

| 字段 | 类型 |
| --- | --- |
| 所属目标 | FIELD_TYPE_REFERENCE |
| 完成时间 | FIELD_TYPE_DATE_TIME |
| 完成度 | FIELD_TYPE_PROGRESS |
| 关键结果 | FIELD_TYPE_TEXT |
| 开始时间 | FIELD_TYPE_DATE_TIME |
| 所属部门 | FIELD_TYPE_LOOKUP |
| 负责人 | FIELD_TYPE_LOOKUP |
| 优先级 | FIELD_TYPE_SELECT |

### Objective目标

| 字段 | 类型 |
| --- | --- |
| 部门 | FIELD_TYPE_SELECT |
| 目标完成度 | FIELD_TYPE_LOOKUP |
| 负责人 | FIELD_TYPE_USER |
| Objective目标 | FIELD_TYPE_TEXT |
| 关键结果 | FIELD_TYPE_REFERENCE |

### OKR复盘

| 字段 | 类型 |
| --- | --- |
| 评分 | FIELD_TYPE_NUMBER |
| 负责人 | FIELD_TYPE_LOOKUP |
| 目标 | FIELD_TYPE_REFERENCE |
| Objective目标 | FIELD_TYPE_TEXT |
| 目标完成度 | FIELD_TYPE_LOOKUP |
| 经验复盘与收获 | FIELD_TYPE_TEXT |

## 计件工资管理

### 计件工资汇总表

| 字段 | 类型 |
| --- | --- |
| 数量 | FIELD_TYPE_NUMBER |
| 计件类型 | FIELD_TYPE_SELECT |
| 单价（元） | FIELD_TYPE_CURRENCY |
| 日期 | FIELD_TYPE_DATE_TIME |
| 姓名 | FIELD_TYPE_USER |
| 工资 | FIELD_TYPE_FORMULA |

### 工资统计看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 按计件类型的工资分布 | pie | [0, 2] | [4, 4] |
| 每日工资分布情况 | bar | [7, 2] | [4, 4] |

## 招聘进度管理

### 招聘进度管理

| 字段 | 类型 |
| --- | --- |
| 候选人来源 | FIELD_TYPE_SELECT |
| 面试状态 | FIELD_TYPE_SELECT |
| 面试部门 | FIELD_TYPE_SELECT |
| 一面面试时间 | FIELD_TYPE_DATE_TIME |
| 二面面试官 | FIELD_TYPE_USER |
| 能力标签 | FIELD_TYPE_SELECT |
| 工作年限 | FIELD_TYPE_SELECT |
| 一面面试官 | FIELD_TYPE_USER |
| 备注 | FIELD_TYPE_TEXT |
| 二面面试时间 | FIELD_TYPE_DATE_TIME |
| 教育背景 | FIELD_TYPE_TEXT |
| 候选人 | FIELD_TYPE_TEXT |

### 招聘看板（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 总应聘人数（按部门） | bar | [0, 3] | [6, 5] |
| 待面试人数（按部门） | bar | [6, 3] | [6, 5] |

## 员工休假情况收集

### 员工休假信息表

| 字段 | 类型 |
| --- | --- |
| 提交人 | FIELD_TYPE_USER |
| 所在部门 | FIELD_TYPE_SELECT |
| 开始休假时间（休假第一天） | FIELD_TYPE_DATE_TIME |
| 请假材料补充 | FIELD_TYPE_ATTACHMENT |
| 总休假天数 | FIELD_TYPE_FORMULA |
| 员工工号 | FIELD_TYPE_NUMBER |
| 结束休假时间（休假最后一天） | FIELD_TYPE_DATE_TIME |
| 请假备注 | FIELD_TYPE_TEXT |
| 员工姓名 | FIELD_TYPE_TEXT |

### 员工休假数据图表（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 员工休假天数明细（表格图） | bar | [8, 0] | [4, 3] |
| 员工请假申请回收数 | numberCard | [0, 0] | [4, 3] |
| 员工请假天数（柱状图） | bar | [4, 0] | [4, 3] |

## 员工信息登记表

### 员工信息登记

| 字段 | 类型 |
| --- | --- |
| 户籍所在地 | FIELD_TYPE_TEXT |
| 紧急联系人与本人关系 | FIELD_TYPE_SELECT |
| 民族 | FIELD_TYPE_SELECT |
| 最高学历 | FIELD_TYPE_SELECT |
| 紧急联系人联系方式 | FIELD_TYPE_PHONE_NUMBER |
| 职位 | FIELD_TYPE_TEXT |
| 银行卡号 | FIELD_TYPE_TEXT |
| 邮箱 | FIELD_TYPE_URL |
| 婚姻情况 | FIELD_TYPE_SELECT |
| 所属部门 | FIELD_TYPE_SELECT |
| 所属银行 | FIELD_TYPE_TEXT |
| 员工编号 | FIELD_TYPE_TEXT |
| 毕业院校 | FIELD_TYPE_TEXT |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 家庭地址 | FIELD_TYPE_TEXT |
| 紧急联系人 | FIELD_TYPE_TEXT |
| 个人照片 | FIELD_TYPE_IMAGE |
| 出生日期 | FIELD_TYPE_DATE_TIME |
| 员工姓名 | FIELD_TYPE_TEXT |

### 仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 员工民族分布 | doughnut | [8, 0] | [4, 3] |
| 员工户籍来源分布 | bar | [0, 3] | [6, 5] |
| 员工学历分布 | doughnut | [6, 3] | [6, 5] |
| 总员工数 | numberCard | [4, 0] | [4, 3] |

## 会议室管理

### 会议室预约登记

| 字段 | 类型 |
| --- | --- |
| 预约会议室（甘特图标题） | FIELD_TYPE_FORMULA |
| 参会人数 | FIELD_TYPE_NUMBER |
| 会议结束时间 | FIELD_TYPE_DATE_TIME |
| 审批人 | FIELD_TYPE_LOOKUP |
| 预约会议室 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 设备需求 | FIELD_TYPE_SELECT |
| 申请时间 | FIELD_TYPE_CREATED_TIME |
| 备注 | FIELD_TYPE_TEXT |
| 审批状态 | FIELD_TYPE_SELECT |
| 预约人 | FIELD_TYPE_CREATED_USER |
| 会议主题 | FIELD_TYPE_TEXT |
| 会议开始时间 | FIELD_TYPE_DATE_TIME |
| 使用时长 (h) | FIELD_TYPE_FORMULA |

### 会议室基础信息

| 字段 | 类型 |
| --- | --- |
| 关联预约单 | FIELD_TYPE_TWOWAYLINKRECORDS |
| 最后编辑时间 | FIELD_TYPE_MODIFIED_TIME |
| 可容纳人数 | FIELD_TYPE_NUMBER |
| 会议室名称 | FIELD_TYPE_TEXT |
| 负责人 | FIELD_TYPE_USER |
| 设备列表 | FIELD_TYPE_SELECT |
| 可用状态 | FIELD_TYPE_SELECT |

### 仪表盘1（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 预约审批情况 | pie | [8, 0] | [4, 4] |
| 会议室可用状态 | bar | [0, 4] | [4, 4] |
| 总预约数 | numberCard | [0, 0] | [4, 4] |
| 通过数 | numberCard | [4, 0] | [4, 4] |

## 活动签到表

### 活动签到表

| 字段 | 类型 |
| --- | --- |
| 备注 | FIELD_TYPE_TEXT |
| 请填写您所在的部门。 | FIELD_TYPE_SELECT |
| 请填写您的联系方式，便于后续更多通知。 | FIELD_TYPE_PHONE_NUMBER |
| 今日是否需要用餐？ | FIELD_TYPE_CHECKBOX |
| 请填写您的真实姓名。 | FIELD_TYPE_TEXT |
| 填写者 | FIELD_TYPE_CREATED_USER |

### 活动人员名单

| 字段 | 类型 |
| --- | --- |
| 是否未签到 | FIELD_TYPE_FORMULA |
| 预计是否需要用餐 | FIELD_TYPE_CHECKBOX |
| 部门负责人 | FIELD_TYPE_USER |
| 序号 | FIELD_TYPE_AUTONUMBER |
| 姓名 | FIELD_TYPE_TEXT |
| 所在部门 | FIELD_TYPE_SELECT |
| 是否已签到 | FIELD_TYPE_LOOKUP |

### 数据统计图（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 已签到人数 | numberCard | [3, 0] | [3, 3] |
| 预计是否用餐数据统计 | column | [0, 3] | [6, 4] |
| 特殊备注内容 | wordCloud | [0, 7] | [12, 6] |
| 实际是否需要用餐统计 | column | [6, 3] | [6, 4] |
| 总参与人数 | numberCard | [0, 0] | [3, 3] |
| 未签到人员所在部门 | bar | [6, 0] | [6, 3] |

## 员工满意度调研

### 员工满意度调研问卷及数据

| 字段 | 类型 |
| --- | --- |
| 您觉得与相关方及上级领导的沟通是否顺畅？ | FIELD_TYPE_SELECT |
| 结果分析 | FIELD_TYPE_TEXT |
| 请您对目前的工作岗位进行评分。 | FIELD_TYPE_SELECT |
| 请您对目前的薪酬福利进行评分。-转换 | FIELD_TYPE_FORMULA |
| 请您对目前的工作内容进行评分。 | FIELD_TYPE_SELECT |
| 请您对公司提供的员工培训及职业发展机会进行评分。 | FIELD_TYPE_SELECT |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 您觉得与相关方及上级领导的沟通是否顺畅？-转换 | FIELD_TYPE_FORMULA |
| 您已入职多久了？ | FIELD_TYPE_SELECT |
| 请您对目前的工作内容进行评分。-转换 | FIELD_TYPE_FORMULA |
| 请您对目前的薪酬福利进行评分。 | FIELD_TYPE_SELECT |
| 请您对目前的管理制度进行评分。-转换 | FIELD_TYPE_FORMULA |
| 请您对目前的工作伙伴进行评分。-转换 | FIELD_TYPE_FORMULA |
| 请您对公司提供的员工培训及职业发展机会进行评分。-转换 | FIELD_TYPE_FORMULA |
| 请您对目前的工作环境进行评分。-转换 | FIELD_TYPE_FORMULA |
| 您觉得公司在哪些方面可以进行改进？ | FIELD_TYPE_SELECT |
| 请您对目前的工作环境进行评分。 | FIELD_TYPE_SELECT |
| 请您对目前的工作岗位进行评分。-转换 | FIELD_TYPE_FORMULA |
| 请尽情抒发您对公司的期许、建议和反馈~ | FIELD_TYPE_TEXT |
| 您所在的部门是？ | FIELD_TYPE_SELECT |
| 请您对目前的工作伙伴进行评分。 | FIELD_TYPE_SELECT |
| 请您对目前的管理制度进行评分。 | FIELD_TYPE_SELECT |

### 满意度分析（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各项内容满意度（平均值） | combo | [4, 0] | [8, 5] |
| 待改进内容分布 | bar | [0, 5] | [4, 6] |
| 员工建议词云 | wordCloud | [4, 5] | [8, 6] |
| 已填写人数 | numberCard | [0, 0] | [4, 5] |

## 会议记录管理

### 会议记录表及汇总

| 字段 | 类型 |
| --- | --- |
| 会议相关材料 | FIELD_TYPE_IMAGE |
| 会议时间-提取年月 | FIELD_TYPE_FORMULA |
| 参会人 | FIELD_TYPE_USER |
| 会议时间 | FIELD_TYPE_DATE_TIME |
| 会议中重点提及的内容 | FIELD_TYPE_TEXT |
| 会议摘要 | FIELD_TYPE_TEXT |
| 会议议题 | FIELD_TYPE_TEXT |
| 会议场地 | FIELD_TYPE_TEXT |
| 参会部门 | FIELD_TYPE_SELECT |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 会议所属类别 | FIELD_TYPE_SELECT |

### 会议记录数据分析盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 会议议题词云 | wordCloud | [6, 4] | [6, 4] |
| 本年度会议召开总计 | numberCard | [0, 0] | [6, 4] |
| 本年度各月份会议占比图 | doughnut | [6, 0] | [6, 4] |
| 会议所属类别汇总 | bar | [0, 4] | [6, 4] |

## 员工绩效考核

### 自评表

| 字段 | 类型 |
| --- | --- |
| 您认为自己在本季度的工作成果可以得几分？ | FIELD_TYPE_SELECT |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 请说明您的心态获得了哪些成长、产生了哪些变化。 | FIELD_TYPE_TEXT |
| 请总结您在第x季度的工作内容及成果 | FIELD_TYPE_TEXT |
| 您填写本表的时间是？ | FIELD_TYPE_DATE_TIME |
| 您所在的部门是？ | FIELD_TYPE_SELECT |
| 请具体罗列出最能体现您工作成果的项目。 | FIELD_TYPE_TEXT |
| 您的上级领导是？ | FIELD_TYPE_USER |
| 请罗列下一季度您的工作计划。 | FIELD_TYPE_TEXT |
| 您认为自己在本季度的心态成长可以得几分？ | FIELD_TYPE_SELECT |
| 您的姓名是？ | FIELD_TYPE_TEXT |

### 互评表

| 字段 | 类型 |
| --- | --- |
| 您认为TA在第x季度的工作成果可以得几分？ | FIELD_TYPE_SELECT |
| 您认为TA在第x季度的心态成长/变化可以得几分？-转文本 | FIELD_TYPE_FORMULA |
| 您认为TA在第x季度的工作成果可以得几分？-转文本 | FIELD_TYPE_FORMULA |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 请讲述选择该分数的原因 | FIELD_TYPE_TEXT |
| 请选择您要互评的同事。 | FIELD_TYPE_SELECT |
| 您填写本表的时间是？ | FIELD_TYPE_DATE_TIME |
| 您所在的部门是？ | FIELD_TYPE_SELECT |
| 请讲述选择该分数的原因。 | FIELD_TYPE_TEXT |
| 您的上级领导是？ | FIELD_TYPE_USER |
| 您认为TA在第x季度的心态成长/变化可以得几分？ | FIELD_TYPE_SELECT |
| 您的姓名是？ | FIELD_TYPE_TEXT |

### 直属领导评分表

| 字段 | 类型 |
| --- | --- |
| 您认为TA在第x季度的工作成果可以得几分？ | FIELD_TYPE_SELECT |
| 填写者 | FIELD_TYPE_CREATED_USER |
| 请讲述选择该分数的原因 | FIELD_TYPE_TEXT |
| 请选择您要评价的部门成员。 | FIELD_TYPE_SELECT |
| 您填写本表的时间是？ | FIELD_TYPE_DATE_TIME |
| 您所在的部门是？ | FIELD_TYPE_SELECT |
| 请讲述选择该分数的原因。 | FIELD_TYPE_TEXT |
| 您的上级领导是？ | FIELD_TYPE_USER |
| 您认为TA在第x季度的心态成长/变化可以得几分？ | FIELD_TYPE_SELECT |
| 您的姓名是？ | FIELD_TYPE_TEXT |

### 绩效评分汇总表

| 字段 | 类型 |
| --- | --- |
| 被评人 | FIELD_TYPE_TEXT |
| 您认为自己在本季度的工作成果可以得几分？（原文档） | FIELD_TYPE_LOOKUP |
| 互评分 | FIELD_TYPE_FORMULA |
| 同事认为TA在第x季度的工作成果可以得几分？ | FIELD_TYPE_LOOKUP |
| 自评所占比例 | FIELD_TYPE_PERCENTAGE |
| 部门 | FIELD_TYPE_LOOKUP |
| 直属领导认为TA在第x季度的工作成果可以得几分？ | FIELD_TYPE_FORMULA |
| 汇总时间 | FIELD_TYPE_DATE_TIME |
| 自评分 | FIELD_TYPE_FORMULA |
| 最终绩效等级 | FIELD_TYPE_FORMULA |
| 同事认为TA在第x季度的心态成长/变化可以得几分？ | FIELD_TYPE_LOOKUP |
| 直属领导评所占比例 | FIELD_TYPE_PERCENTAGE |
| 您认为自己在本季度的心态成长可以得几分？（原文档） | FIELD_TYPE_LOOKUP |
| 互评所占比例 | FIELD_TYPE_PERCENTAGE |
| 最终得分总计 | FIELD_TYPE_FORMULA |
| TA认为自己在本季度的工作成果可以得几分？ | FIELD_TYPE_FORMULA |
| TA认为自己在本季度的心态成长可以得几分？ | FIELD_TYPE_FORMULA |
| 直属领导认为TA在第x季度的工作成果可以得几分？（原数据） | FIELD_TYPE_LOOKUP |

### 人员花名册

| 字段 | 类型 |
| --- | --- |
| 是否需要参与绩效 | FIELD_TYPE_SELECT |
| 直属领导 | FIELD_TYPE_TEXT |
| 入职时间 | FIELD_TYPE_DATE_TIME |
| 直属领导是否未评 | FIELD_TYPE_FORMULA |
| 部门 | FIELD_TYPE_SELECT |
| 姓名 | FIELD_TYPE_TEXT |
| 是否未自评 | FIELD_TYPE_FORMULA |
| 分管领导 | FIELD_TYPE_TEXT |
| 备注 | FIELD_TYPE_TEXT |
| 是否未互评 | FIELD_TYPE_FORMULA |
| 直属领导是否已评（原数据） | FIELD_TYPE_LOOKUP |
| 是否已自评（原数据） | FIELD_TYPE_LOOKUP |
| 是否已互评（原数据） | FIELD_TYPE_LOOKUP |

### 数据分析仪表盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 最终绩效等级分布图 | stackbar | [0, 3] | [6, 3] |
| 特殊情况人员 | numberCard | [9, 0] | [3, 3] |
| 未完成绩效评选人数 | numberCard | [6, 0] | [3, 3] |
| 实际参与本次绩效人数 | numberCard | [3, 0] | [3, 3] |
| 总人数 | numberCard | [0, 0] | [3, 3] |
| 各部门平均分对比图 | column | [0, 6] | [6, 3] |
| 未完成绩效评选的人员分布 | pie | [6, 3] | [6, 6] |

## 员工薪资计算

### 员工薪资计算表

| 字段 | 类型 |
| --- | --- |
| 养老保险（个人缴纳） | FIELD_TYPE_FORMULA |
| 本月应付薪资 | FIELD_TYPE_FORMULA |
| 病假天数 | FIELD_TYPE_NUMBER |
| 医疗保险（企业缴纳） | FIELD_TYPE_FORMULA |
| 加班费用 | FIELD_TYPE_FORMULA |
| 工资小计1 | FIELD_TYPE_FORMULA |
| 医疗保险（个人缴纳） | FIELD_TYPE_FORMULA |
| 基础岗位工资 | FIELD_TYPE_NUMBER |
| 绩效工资 | FIELD_TYPE_NUMBER |
| 实出勤天数 | FIELD_TYPE_NUMBER |
| 出勤天数是否无误 | FIELD_TYPE_FORMULA |
| 工资小计2 | FIELD_TYPE_FORMULA |
| 失业保险（企业缴纳） | FIELD_TYPE_FORMULA |
| 季度/年度奖金 | FIELD_TYPE_NUMBER |
| 扣除旷工费用 | FIELD_TYPE_FORMULA |
| 工龄奖 | FIELD_TYPE_NUMBER |
| 无薪事假天数 | FIELD_TYPE_NUMBER |
| 加班时长（分钟） | FIELD_TYPE_NUMBER |
| 是否已提交病假材料 | FIELD_TYPE_TEXT |
| 失业保险（个人缴纳） | FIELD_TYPE_FORMULA |
| 应出勤天数 | FIELD_TYPE_NUMBER |
| 工伤保险（企业缴纳） | FIELD_TYPE_FORMULA |
| 扣除无薪事假费用 | FIELD_TYPE_FORMULA |
| 所在部门 | FIELD_TYPE_SELECT |
| 工资小计3 | FIELD_TYPE_FORMULA |
| 带薪假天数（含年假） | FIELD_TYPE_NUMBER |
| 全勤奖 | FIELD_TYPE_FORMULA |
| 员工工号 | FIELD_TYPE_TEXT |
| 扣除病假费用 | FIELD_TYPE_FORMULA |
| 本月应付费用（五险） | FIELD_TYPE_FORMULA |
| 任职岗位 | FIELD_TYPE_TEXT |
| 旷工天数 | FIELD_TYPE_NUMBER |
| 员工姓名 | FIELD_TYPE_TEXT |
| 养老保险（企业缴纳） | FIELD_TYPE_FORMULA |

### 员工花名册

| 字段 | 类型 |
| --- | --- |
| 是否在职 | FIELD_TYPE_CHECKBOX |
| 工龄 | FIELD_TYPE_FORMULA |
| 所在部门 | FIELD_TYPE_SELECT |
| 入职时间 | FIELD_TYPE_DATE_TIME |
| 员工工号 | FIELD_TYPE_TEXT |
| 任职岗位 | FIELD_TYPE_TEXT |
| 员工姓名 | FIELD_TYPE_TEXT |

### 员工薪资数据盘（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 各部门人员占比 | pie | [0, 4] | [6, 5] |
| 本月共支出五险费用 | numberCard | [6, 2] | [6, 2] |
| 本月共支出薪资 | numberCard | [6, 0] | [6, 2] |
| 本月人效比 | combo | [6, 4] | [6, 5] |
| 当前在职人员 | numberCard | [0, 0] | [6, 4] |
