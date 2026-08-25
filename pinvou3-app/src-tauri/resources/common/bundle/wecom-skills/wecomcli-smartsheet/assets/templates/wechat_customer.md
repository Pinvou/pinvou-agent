# 管理微信上的客户的数据表模版

## 包含表格模版

- **客户服务跟进**：基于企业微信外部联系人数据，管理客户线索、服务跟进记录及满意度调研，支持业绩仪表盘展示销售额、客户状态和来源分布。
- **客户群服务跟进**：以客户群为单位管理服务跟进，记录群主、群人数、客户状态及订单信息，适合通过微信群维护客户关系的销售场景。
- **客户销售跟进**：整合客户商机、订单跟进和产品库存管理，记录客户意向数量、订单进度及产品报价，支持销售全流程可视化管理。
- **学员服务跟进**：面向教育培训机构，管理学员线索、课程预约、上课记录及学员评价，支持教练课时统计和销售业绩分析。
- **学员群服务跟进**：以学员群为单位管理课程服务，记录群主、学员状态、课程预约及上课记录，适合通过微信群运营学员的培训机构。

## 客户服务跟进

### 客户线索（示例）

| 字段 | 类型 |
| --- | --- |
| 职务 | FIELD_TYPE_TEXT |
| 地址 | FIELD_TYPE_TEXT |
| 企业 | FIELD_TYPE_TEXT |
| 添加人所属部门 | FIELD_TYPE_SELECT |
| 对接销售 | FIELD_TYPE_USER |
| 客户 | FIELD_TYPE_USER |
| 跟进备注（可编辑） | FIELD_TYPE_TEXT |
| 标签组 | FIELD_TYPE_SELECT |
| 客户状态（可编辑） | FIELD_TYPE_SELECT |
| 客户跟进总结 | FIELD_TYPE_TEXT |
| 添加人 | FIELD_TYPE_USER |
| 手机 | FIELD_TYPE_PHONE_NUMBER |
| 其他添加人 | FIELD_TYPE_LOOKUP |
| 来源 | FIELD_TYPE_SELECT |
| 添加时间 | FIELD_TYPE_DATE_TIME |
| 描述 | FIELD_TYPE_TEXT |
| 电话 | FIELD_TYPE_PHONE_NUMBER |
| 添加人账号 | FIELD_TYPE_TEXT |
| 邮箱 | FIELD_TYPE_EMAIL |
| 客户名称 | FIELD_TYPE_TEXT |

### 服务跟进（示例）

| 字段 | 类型 |
| --- | --- |
| 订单类型 | FIELD_TYPE_SELECT |
| 客户电话 | FIELD_TYPE_LOOKUP |
| 服务状态 | FIELD_TYPE_SELECT |
| 客户名称 | FIELD_TYPE_REFERENCE |
| 订单编号 | FIELD_TYPE_AUTONUMBER |
| 客户反馈 | FIELD_TYPE_TEXT |
| 服务对接人 | FIELD_TYPE_USER |
| 成交金额 | FIELD_TYPE_CURRENCY |
| 成交时间 | FIELD_TYPE_DATE_TIME |

### 满意度调研（示例）

| 字段 | 类型 |
| --- | --- |
| 订单编号 | FIELD_TYPE_REFERENCE |
| 提交时间 | FIELD_TYPE_CREATED_TIME |
| 是否考虑回购 | FIELD_TYPE_SELECT |
| 客户名称 | FIELD_TYPE_TEXT |
| 订单类型 | FIELD_TYPE_LOOKUP |
| 其他意见和建议 | FIELD_TYPE_TEXT |
| 服务打分 | FIELD_TYPE_NUMBER |
| 是否需要申请售后？ | FIELD_TYPE_SELECT |
| 产品打分 | FIELD_TYPE_NUMBER |

### 业绩仪表盘（示例）（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 客户记录数 | numberCard | [0, 1] | [4, 3] |
| 订单分布 | stackbar | [8, 5] | [4, 3] |
| 销售分布 | pie | [4, 5] | [4, 3] |
| 客户状态分布 | pie | [8, 1] | [4, 3] |
| 本周新增客户数 | numberCard | [4, 1] | [4, 3] |
| 添加人分布 | bar | [4, 9] | [4, 3] |
| 添加时间分布 | smoothline | [0, 9] | [4, 3] |
| 总销售额 | numberCard | [0, 5] | [4, 3] |
| 客户来源分布 | pie | [8, 9] | [4, 3] |

## 客户群服务跟进

### 客户线索（示例）

| 字段 | 类型 |
| --- | --- |
| 职务 | FIELD_TYPE_TEXT |
| 企业 | FIELD_TYPE_TEXT |
| 客户群 | FIELD_TYPE_WWGROUP |
| 跟进备注 | FIELD_TYPE_TEXT |
| 客户状态（可编辑） | FIELD_TYPE_SELECT |
| 对接销售 | FIELD_TYPE_USER |
| 群主 | FIELD_TYPE_USER |
| 客户跟进总结 | FIELD_TYPE_TEXT |
| 群人数 | FIELD_TYPE_NUMBER |
| 群主所在部门 | FIELD_TYPE_SELECT |
| 客户（可编辑） | FIELD_TYPE_USER |
| 创建时间 | FIELD_TYPE_DATE_TIME |
| 手机 | FIELD_TYPE_PHONE_NUMBER |
| 邮箱 | FIELD_TYPE_EMAIL |

### 服务跟进（示例）

| 字段 | 类型 |
| --- | --- |
| 客户群 | FIELD_TYPE_LOOKUP |
| 订单类型 | FIELD_TYPE_SELECT |
| 客户电话 | FIELD_TYPE_LOOKUP |
| 服务状态 | FIELD_TYPE_SELECT |
| 客户名称 | FIELD_TYPE_REFERENCE |
| 订单编号 | FIELD_TYPE_AUTONUMBER |
| 客户反馈 | FIELD_TYPE_TEXT |
| 服务对接人 | FIELD_TYPE_USER |
| 成交金额 | FIELD_TYPE_CURRENCY |
| 成交时间 | FIELD_TYPE_DATE_TIME |

### 满意度调研（示例）

| 字段 | 类型 |
| --- | --- |
| 订单编号 | FIELD_TYPE_REFERENCE |
| 提交时间 | FIELD_TYPE_CREATED_TIME |
| 是否考虑回购 | FIELD_TYPE_SELECT |
| 用户名称 | FIELD_TYPE_TEXT |
| 订单类型 | FIELD_TYPE_LOOKUP |
| 其他意见和建议 | FIELD_TYPE_TEXT |
| 服务打分 | FIELD_TYPE_NUMBER |
| 是否需要申请售后？ | FIELD_TYPE_SELECT |
| 产品打分 | FIELD_TYPE_NUMBER |

### 业绩仪表盘（示例）（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 客户记录数 | numberCard | [0, 1] | [4, 3] |
| 总销售额 | numberCard | [0, 5] | [4, 3] |
| 订单类型分布 | doughnut | [8, 5] | [4, 3] |
| 销售人员业绩分布 | bar | [4, 5] | [4, 3] |
| 创建时间分布 | smoothline | [6, 9] | [6, 4] |
| 本周新增客户群数 | numberCard | [4, 1] | [4, 3] |
| 客户状态分布 | pie | [0, 9] | [6, 4] |
| 群主分布 | bar | [8, 1] | [4, 3] |

## 客户销售跟进

### 客户商机（示例）

| 字段 | 类型 |
| --- | --- |
| 职务 | FIELD_TYPE_TEXT |
| 地址 | FIELD_TYPE_TEXT |
| 企业 | FIELD_TYPE_TEXT |
| 添加人所属部门 | FIELD_TYPE_SELECT |
| 客户 | FIELD_TYPE_USER |
| 客户名称 2 | FIELD_TYPE_LOCATION |
| 标签组 | FIELD_TYPE_SELECT |
| 客户跟进总结 | FIELD_TYPE_TEXT |
| 文本 | FIELD_TYPE_TEXT |
| 添加人 | FIELD_TYPE_USER |
| 客户状态 | FIELD_TYPE_SELECT |
| 电话 | FIELD_TYPE_PHONE_NUMBER |
| 其他添加人 | FIELD_TYPE_LOOKUP |
| 来源 | FIELD_TYPE_SELECT |
| 添加时间 | FIELD_TYPE_DATE_TIME |
| 描述 | FIELD_TYPE_TEXT |
| 手机 | FIELD_TYPE_PHONE_NUMBER |
| 地理位置 | FIELD_TYPE_LOCATION |
| 跟进备注 | FIELD_TYPE_TEXT |
| 添加人账号 | FIELD_TYPE_TEXT |
| 邮箱 | FIELD_TYPE_EMAIL |
| 客户名称 | FIELD_TYPE_TEXT |

### 订单跟进（示例）

| 字段 | 类型 |
| --- | --- |
| 职务 | FIELD_TYPE_TEXT |
| 地址 | FIELD_TYPE_TEXT |
| 预估订单金额 | FIELD_TYPE_FORMULA |
| 企业 | FIELD_TYPE_TEXT |
| 添加人所属部门 | FIELD_TYPE_SELECT |
| 产品 | FIELD_TYPE_REFERENCE |
| 订单进度 | FIELD_TYPE_SELECT |
| 跟进销售 | FIELD_TYPE_USER |
| 最近跟进时间 | FIELD_TYPE_DATE_TIME |
| 客户 | FIELD_TYPE_USER |
| 单价 | FIELD_TYPE_CURRENCY |
| 自动编号 | FIELD_TYPE_AUTONUMBER |
| 跟进备注 | FIELD_TYPE_TEXT |
| 标签组 | FIELD_TYPE_SELECT |
| 意向数量 | FIELD_TYPE_NUMBER |
| 手机 | FIELD_TYPE_PHONE_NUMBER |
| 添加人 | FIELD_TYPE_USER |
| 用户来源 | FIELD_TYPE_SELECT |
| 添加时间 | FIELD_TYPE_DATE_TIME |
| 描述 | FIELD_TYPE_TEXT |
| 电话 | FIELD_TYPE_PHONE_NUMBER |
| 添加人账号 | FIELD_TYPE_TEXT |
| 邮箱 | FIELD_TYPE_EMAIL |
| 客户名称 | FIELD_TYPE_TEXT |

### 产品库存（示例）

| 字段 | 类型 |
| --- | --- |
| 图片 | FIELD_TYPE_IMAGE |
| 更新时间 | FIELD_TYPE_MODIFIED_TIME |
| 产品名称 | FIELD_TYPE_TEXT |
| 库存数量 | FIELD_TYPE_NUMBER |
| 货号 | FIELD_TYPE_AUTONUMBER |
| 报价 | FIELD_TYPE_CURRENCY |
| 上架季节 | FIELD_TYPE_SELECT |

### 业绩仪表盘（示例）（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 本周新增客户数 | numberCard | [4, 1] | [4, 3] |
| 添加时间分布 | smoothline | [4, 14] | [4, 3] |
| 产品报价及库存 | combo | [0, 9] | [12, 4] |
| 客户分布 | doughnut | [8, 5] | [4, 3] |
| 总销售额 | numberCard | [0, 5] | [4, 3] |
| 添加人分布 | bar | [0, 14] | [4, 3] |
| 客户记录数 | numberCard | [0, 1] | [4, 3] |
| 销售分布 | pie | [4, 5] | [4, 3] |
| 客户来源分布 | pie | [8, 14] | [4, 3] |
| 客户状态分布 | pie | [8, 1] | [4, 3] |

## 学员服务跟进

### 学员线索（示例）

| 字段 | 类型 |
| --- | --- |
| 学员跟进总结 | FIELD_TYPE_TEXT |
| 添加人所属部门 | FIELD_TYPE_SELECT |
| 邮箱 | FIELD_TYPE_EMAIL |
| 添加人账号 | FIELD_TYPE_TEXT |
| 描述 | FIELD_TYPE_TEXT |
| 标签组 | FIELD_TYPE_SELECT |
| 添加人 | FIELD_TYPE_USER |
| 学员 | FIELD_TYPE_USER |
| 对接销售 | FIELD_TYPE_USER |
| 跟进备注 | FIELD_TYPE_TEXT |
| 学员状态 | FIELD_TYPE_SELECT |
| 手机 | FIELD_TYPE_PHONE_NUMBER |
| 地址 | FIELD_TYPE_TEXT |
| 企业 | FIELD_TYPE_TEXT |
| 来源 | FIELD_TYPE_SELECT |
| 职务 | FIELD_TYPE_TEXT |
| 添加时间 | FIELD_TYPE_DATE_TIME |
| 其他添加人 | FIELD_TYPE_LOOKUP |
| 学员名称 | FIELD_TYPE_TEXT |

### 课程预约（示例）

| 字段 | 类型 |
| --- | --- |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 预约上课时间 | FIELD_TYPE_DATE_TIME |
| 姓名 | FIELD_TYPE_TEXT |
| 订单ID | FIELD_TYPE_FORMULA |
| 课程类型 | FIELD_TYPE_SELECT |
| 课时/小时 | FIELD_TYPE_NUMBER |

### 上课记录（示例）

| 字段 | 类型 |
| --- | --- |
| 是否转化为会员 | FIELD_TYPE_SELECT |
| 课程类型 | FIELD_TYPE_LOOKUP |
| 客户电话 | FIELD_TYPE_LOOKUP |
| 是否付款 | FIELD_TYPE_SELECT |
| 学员名称 | FIELD_TYPE_REFERENCE |
| 订单编号 | FIELD_TYPE_LOOKUP |
| 是否上课 | FIELD_TYPE_SELECT |
| 付款方式 | FIELD_TYPE_SELECT |
| 负责销售 | FIELD_TYPE_USER |
| 时长/小时 | FIELD_TYPE_LOOKUP |
| 上课备注 | FIELD_TYPE_TEXT |
| 教练 | FIELD_TYPE_USER |
| 付费课时 | FIELD_TYPE_NUMBER |
| 订单金额 | FIELD_TYPE_CURRENCY |
| 预约时间 | FIELD_TYPE_LOOKUP |

### 学员评价（示例）

| 字段 | 类型 |
| --- | --- |
| 教练专业度 | FIELD_TYPE_NUMBER |
| 上课环境 | FIELD_TYPE_NUMBER |
| 是否会推荐给朋友 | FIELD_TYPE_SELECT |
| 其他评价和建议 | FIELD_TYPE_TEXT |
| 提交时间 | FIELD_TYPE_DATE_TIME |
| 学员名称 | FIELD_TYPE_TEXT |

### 业绩仪表盘（示例）（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 添加时间分布 | smoothline | [4, 13] | [4, 3] |
| 总销售额 | numberCard | [0, 5] | [4, 3] |
| 课程分布 | bar | [8, 5] | [4, 3] |
| 教练课时数 | stackbar | [0, 9] | [6, 3] |
| 本周新增学员数 | numberCard | [4, 1] | [4, 3] |
| 学员状态分布 | pie | [8, 1] | [4, 3] |
| 学员来源分布 | pie | [8, 13] | [4, 3] |
| 添加人分布 | bar | [0, 13] | [4, 3] |
| 学员记录数 | numberCard | [0, 1] | [4, 3] |
| 销售人员业绩 | doughnut | [4, 5] | [4, 3] |

## 学员群服务跟进

### 学员线索（示例）

| 字段 | 类型 |
| --- | --- |
| 创建时间 | FIELD_TYPE_DATE_TIME |
| 群人数 | FIELD_TYPE_NUMBER |
| 学员群 | FIELD_TYPE_WWGROUP |
| 学员（可编辑） | FIELD_TYPE_USER |
| 对接销售 | FIELD_TYPE_USER |
| 跟进备注（可编辑） | FIELD_TYPE_TEXT |
| 学员状态（可编辑） | FIELD_TYPE_SELECT |
| 手机 | FIELD_TYPE_PHONE_NUMBER |
| 群主 | FIELD_TYPE_USER |
| 客户跟进总结 | FIELD_TYPE_TEXT |
| 群主所在部门 | FIELD_TYPE_SELECT |

### 课程预约（示例）

| 字段 | 类型 |
| --- | --- |
| 联系电话 | FIELD_TYPE_PHONE_NUMBER |
| 预约上课时间 | FIELD_TYPE_DATE_TIME |
| 姓名 | FIELD_TYPE_TEXT |
| 订单ID | FIELD_TYPE_FORMULA |
| 课程类型 | FIELD_TYPE_SELECT |
| 课时/小时 | FIELD_TYPE_NUMBER |

### 上课记录（示例）

| 字段 | 类型 |
| --- | --- |
| 是否转化为会员 | FIELD_TYPE_SELECT |
| 课程类型 | FIELD_TYPE_LOOKUP |
| 客户电话 | FIELD_TYPE_LOOKUP |
| 是否付款 | FIELD_TYPE_SELECT |
| 学员名称 | FIELD_TYPE_REFERENCE |
| 订单编号 | FIELD_TYPE_LOOKUP |
| 是否上课 | FIELD_TYPE_SELECT |
| 付款方式 | FIELD_TYPE_SELECT |
| 负责销售 | FIELD_TYPE_USER |
| 时长/小时 | FIELD_TYPE_LOOKUP |
| 上课备注 | FIELD_TYPE_TEXT |
| 教练 | FIELD_TYPE_USER |
| 付费课时 | FIELD_TYPE_NUMBER |
| 订单金额 | FIELD_TYPE_CURRENCY |
| 预约时间 | FIELD_TYPE_LOOKUP |

### 学员评价（示例）

| 字段 | 类型 |
| --- | --- |
| 教练专业度 | FIELD_TYPE_NUMBER |
| 上课环境 | FIELD_TYPE_NUMBER |
| 是否会推荐给朋友 | FIELD_TYPE_SELECT |
| 其他评价和建议 | FIELD_TYPE_TEXT |
| 提交时间 | FIELD_TYPE_DATE_TIME |
| 学员名称 | FIELD_TYPE_TEXT |

### 业绩仪表盘（示例）（仪表盘）

| 图表名称 | 图表类型 | 坐标 | 尺寸 |
| --- | --- | --- | --- |
| 教练课时数 | stackbar | [0, 9] | [6, 3] |
| 教练评价 | bar | [6, 9] | [6, 3] |
| 课程分布 | bar | [8, 5] | [4, 3] |
| 学员记录数 | numberCard | [0, 1] | [4, 3] |
| 群主分布 | bar | [8, 1] | [4, 3] |
| 学员状态分布 | pie | [0, 13] | [6, 4] |
| 销售人员业绩 | doughnut | [4, 5] | [4, 3] |
| 本周新增学员群数 | numberCard | [4, 1] | [4, 3] |
| 创建时间分布 | smoothline | [6, 13] | [6, 4] |
| 总销售额 | numberCard | [0, 5] | [4, 3] |
