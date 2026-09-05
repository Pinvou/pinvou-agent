# 视图类型（ViewType）完整参考

## View（视图结构）

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `view_id` | string | 视图 ID |
| `view_title` | string | 视图标题 |
| `view_type` | string (ViewType) | 视图类型，见下方 ViewType 枚举 |
| `property` | ViewProperty | 视图属性 |

---

## ViewParam（视图操作参数）

统一结构，根据操作指令不同使用不同字段组合：

> - `smartsheet views add` 时：传 `view_title` + `view_type`，甘特视图传 `property_gantt`，日历视图传 `property_calendar`
> - `smartsheet views update` 时：传 `view_id`，可选传 `view_title` 和 `property`。**不支持修改视图类型**，只能修改同一视图下的标题和属性（如筛选、排序、分组等），不能将一种视图类型改为另一种（例如不能把表格视图改为看板视图）。如需更换视图类型，只能先删除旧视图再新增新视图
> - `smartsheet views delete` 时：只传 `view_id`。若该子表只剩最后一个视图，须遵循 `references/smart-sheet-edit.md` 顶部**删除最后一个子表/字段/视图固定流程**处理

| 字段 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `view_id` | string | 条件 | 视图 ID（update、delete 时必传） |
| `view_title` | string | 条件 | 视图标题（add 时必传，update 时可选） |
| `view_type` | string (ViewType) | 条件 | 视图类型（add 时必传），见下方 ViewType 枚举 |
| `property` | ViewProperty | 否 | 视图属性（update 时可选） |
| `property_gantt` | GanttViewProperty | 否 | 甘特视图属性（add 甘特视图时必填） |
| `property_calendar` | CalendarViewProperty | 否 | 日历视图属性（add 日历视图时必填） |
| `col_infos` | ViewColInfos[] | 否 | 列宽设置 |

---

## ViewType 枚举

| 参数值 | 说明 |
| --- | --- |
| `grid` | 表格视图 |
| `kanban` | 看板视图 |
| `gallery` | 画册视图 |
| `gantt` | 甘特视图 |
| `calendar` | 日历视图 |
| `form` | 表单视图 |

---

## 特殊视图属性

### GanttViewProperty（甘特视图属性）

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `start_date_field_title` | string | 是 | 时间条起点字段名称，只允许日期类型 |
| `end_date_field_title` | string | 是 | 时间条终点字段名称，只允许日期类型 |

### CalendarViewProperty（日历视图属性）

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `start_date_field_title` | string | 是 | 时间条起点字段名称，只允许日期类型 |
| `end_date_field_title` | string | 是 | 时间条终点字段名称，只允许日期类型 |

### ViewColInfos（列宽信息）

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `field_title` | string | 是 | 字段名称 |
| `width` | int32 | 是 | 列宽，范围 1～1000 |

#### 列宽调整接口调用方式

通过 `smartsheet views update` 的 `col_infos` 参数设置列宽，调用前须先获取目标视图的 `view_id`：

```bash
# 1. 获取视图列表，取第一个视图的 view_id
wecom-cli smartsheet views list --json '{"docid": "<docid>", "sheet_title": "<子表名称>", "limit": 100}'

# 2. 调用 views update 设置列宽（可一次性传入所有字段）
wecom-cli smartsheet views update --json '{
  "docid": "<docid>",
  "sheet_title": "<子表名称>",
  "type": "update",
  "views": [{
    "view_id": "<view_id>",
    "col_infos": [
      {"field_title": "任务名称", "width": 280},
      {"field_title": "优先级", "width": 160},
      {"field_title": "状态", "width": 120}
    ]
  }]
}'
```

#### 新建字段时的列宽判断规则

新建字段（含随子表初始化的字段）后，AI 须为每个字段选择合适的列宽档位，最终写入对应的 px 值。共 4 个档位：

| 档位 | 宽度 |
| --- | --- |
| `compact` | 120px |
| `default` | 160px |
| `wide` | 280px |
| `extra_wide` | 400px |

**判断依据：字段类型初始档位 + 字段名语义**

**第一步：按字段类型查初始档位**

| 字段类型 | 初始档位 | 备注 |
| --- | --- | --- |
| `checkbox` | `compact` | 固定，跳过第二步 |
| `number` | `compact` | 固定，跳过第二步 |
| `autonumber` | `compact` | 固定，跳过第二步 |
| `currency` | `compact` | 固定，跳过第二步 |
| `percentage` | `compact` | 固定，跳过第二步 |
| `progress` | `compact` | 固定，跳过第二步 |
| `phone_number` | `compact` | 固定，跳过第二步 |
| `barcode` | `compact` | 固定，跳过第二步 |
| `date_time`（紧凑格式） | `compact` | 固定，跳过第二步 |
| `created_time`（紧凑格式） | `compact` | 固定，跳过第二步 |
| `modified_time`（紧凑格式） | `compact` | 固定，跳过第二步 |
| `date_time`（宽松格式） | `default` | 固定，跳过第二步 |
| `created_time`（宽松格式） | `default` | 固定，跳过第二步 |
| `modified_time`（宽松格式） | `default` | 固定，跳过第二步 |
| `created_user` | `default` | 固定，跳过第二步 |
| `modified_user` | `default` | 固定，跳过第二步 |
| `email` | `default` | 固定，跳过第二步 |
| `single_select` | `compact` | 可调 |
| `select` | `default` | 可调 |
| `user` | `default` | 可调 |
| `attachment` | `default` | 可调 |
| `image` | `default` | 可调 |
| `reference` | `default` | 可调 |
| `two_way_link_records` | `default` | 可调 |
| `wwgroup` | `default` | 可调 |
| `formula` | `default` | 可调 |
| `lookup` | `default` | 可调 |
| `url` | `wide` | 可调 |
| `location` | `wide` | 可调 |
| `text` | `wide` | 可调 |

**第二步：对"可调"类型，按字段名语义决定是否上调**

- 字段名含"描述/备注/说明/详情/内容/原因/摘要/简介/评论/补充" → 上调至 `extra_wide`
- 字段名含"标题/名称/任务/需求/项目" → 取初始档位与 `wide` 中较大的档位
- 字段名无明显语义指示 → 保持初始档位

**第三步：列名宽度兜底检查（所有字段，含固定档位）**

估算字段名的渲染宽度：汉字按 24px/字，非汉字按 14px/字符。若估算值超过当前档位宽度，则向上取能容纳的最小档位；最高升至 `extra_wide`（400px）。

> **示例**：字段名"创建时间"（4 汉字）→ 4×24 = 96px，`compact`（120px）够用 → 保持。  
> 字段名"是否已完成确认"（8 汉字）→ 8×24 = 192px，`compact` 不够 → 升到 `wide`（280px）。  
> 字段名"status"（6 非汉字）→ 6×14 = 84px，`compact`（120px）够用 → 保持。

---

## ViewProperty（视图属性）

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `auto_sort` | bool | 否 | 记录变更后自动重新排序 |
| `sort_spec` | SortSpec | 否 | 排序设置 |
| `group_spec` | GroupSpec | 否 | 分组设置 |
| `filter_spec` | FilterSpec | 否 | 过滤筛选设置，无筛选条件时，必须**完全省略** `filter_spec` 字段；禁止传 `"filter_spec": {}` 或空的 `conditions`。空对象会被后端当作不完整的 FilterSpec 解析，触发“无效的连接符”错误。只有确实需要筛选时，才传完整的 `filter_spec`，且必须包含合法的 `conjunction` 和非空 `conditions`。 |
| `is_field_stat_enabled` | bool | 否 | 是否使用数据统计 |
| `field_visibility` | object | 否 | key 为字段名称（`field_title`），value 为布尔值表示是否显示 |
| `frozen_field_count` | int32 | 否 | 冻结列数量，从首列开始 |
| `color_config` | ViewColorConfig | 否 | 填色设置 |

### SortSpec（排序设置）

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `sort_infos` | SortInfo[] | 否 | 参与排序的字段列表 |

### SortInfo

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `field_title` | string | 是 | 字段名称 |
| `desc` | bool | 否 | 是否降序 |

### GroupSpec（分组设置）

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `groups` | GroupInfo[] | 否 | 参与分组的字段列表 |

### GroupInfo

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `field_title` | string | 是 | 字段名称 |
| `desc` | bool | 否 | 是否降序 |

---

## FilterSpec（过滤设置）

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `conjunction` | string | 是 | 多个 conditions 之间的组合方式：`and` (条件与) 或 `or` (条件或) |
| `conditions` | Condition[] | 是 | 判断条件 |

### Condition（判断条件）

> 不同字段类型支持的筛选不同，需根据字段类型实际支持的筛选条件进行组合。

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `field_title` | string | 是 | 字段名称 |
| `field_type` | string | 是 | 字段类型 |
| `operator` | string (Operator) | 是 | 判断类型，见下方 Operator 枚举 |
| `string_value` | StringValue | 否 | 文本/网址/电话/邮箱/地理位置/单选/多选等列类型使用。单选/多选支持直接传选项文本，后端会自动匹配并存储对应的选项 ID，不要求一定传 `options[].id` |
| `number_value` | NumberValue | 否 | 数字/进度/货币/百分数等列类型使用 |
| `bool_value` | BoolValue | 否 | 复选框列类型使用 |
| `user_value` | UserValue | 否 | 成员/创建人/编辑人列类型使用 |
| `date_time_value` | FilterDateTimeValue | 否 | 日期/创建时间/编辑时间列类型使用 |

### StringValue

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `value` | string[] | 字符串值列表 |

### NumberValue

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `value` | double | 数字值 |

### BoolValue

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `value` | bool | 布尔值 |

### UserValue

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `value` | string[] | 成员 userid 列表 |

### FilterDateTimeValue

| 字段 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `type` | string (DateTimeType) | 是 | 日期类型，见下方 DateTimeType 枚举 |
| `value` | string[] | 是 | 具体日期值，type 为 `detail_date` 时必填，格式为 `YYYY-MM-DD HH:mm:ss`，例如 `["2026-06-01 00:00:00"]` |

---

## 通用枚举值

### Operator（判断类型）

| 参数值 | 说明 |
| --- | --- |
| `is` | 等于 |
| `is_not` | 不等于 |
| `contains` | 包含 |
| `does_not_contain` | 不包含 |
| `is_greater` | 大于/时间晚于 |
| `is_greater_or_equal` | 大于或等于/时间晚于 |
| `is_less` | 小于/早于 |
| `is_less_or_equal` | 小于或等于/时间早于 |
| `is_empty` | 为空 |
| `is_not_empty` | 不为空 |

### DateTimeType（日期类型）

| 参数值 | 说明 |
| --- | --- |
| `detail_date` | 具体时间 |
| `today` | 今天 |
| `tomorrow` | 明天 |
| `yesterday` | 昨天 |
| `current_week` | 本周 |
| `last_week` | 上周 |
| `current_month` | 本月 |
| `the_past_7_days` | 过去 7 天内 |
| `the_next_7_days` | 接下来 7 天内 |
| `last_month` | 上月 |
| `the_past_30_days` | 过去 30 天内 |
| `the_next_30_days` | 接下来 30 天内 |

---

## 填色设置

### ViewColorConfig

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `conditions` | ViewColorCondition[] | 是 | 填色条件列表 |

### ViewColorCondition

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `id` | string | 否 | 填色 ID，新增时不需要传入，更新时传入 |
| `type` | string (ViewColorConditionType) | 是 | 填色类型，见下方枚举 |
| `color` | string (ViewColor) | 是 | 颜色，见下方 ViewColor 枚举 |
| `condition` | Condition | 是 | 判断条件 |

### ViewColorConditionType

| 参数值 | 说明 |
| --- | --- |
| `row` | 行 |
| `column` | 列 |
| `cell` | 单元格 |

### ViewColor（颜色值）

| 颜色值 | 描述 |
| --- | --- |
| `fillColorGray_5` | 灰色\_5 |
| `accentBlueLighten_5` | 蓝色\_5 |
| `chromeCyanLighten_5` | 青色\_5 |
| `chromeMintLighten_5` | 薄荷色\_5 |
| `chromeRedLighten_5` | 红色\_5 |
| `chromeOrangeLighten_5` | 橙色\_5 |
| `chromeAmberLighten_5` | 琥珀色\_5 |
| `chromeVioletLighten_5` | 紫色\_5 |
| `chromePinkLighten_5` | 粉色\_5 |
| `fillColorGray_4` | 灰色\_4 |
| `accentBlueLighten_4` | 蓝色\_4 |
| `chromeCyanLighten_4` | 青色\_4 |
| `chromeMintLighten_4` | 薄荷色\_4 |
| `chromeRedLighten_4` | 红色\_4 |
| `chromeOrangeLighten_4` | 橙色\_4 |
| `chromeAmberLighten_4` | 琥珀色\_4 |
| `chromeVioletLighten_4` | 紫色\_4 |
| `chromePinkLighten_4` | 粉色\_4 |
| `fillColorGray_3` | 灰色\_3 |
| `accentBlueLighten_3` | 蓝色\_3 |
| `chromeCyanLighten_3` | 青色\_3 |
| `chromeMintLighten_3` | 薄荷色\_3 |
| `chromeRedLighten_3` | 红色\_3 |
| `chromeOrangeLighten_3` | 橙色\_3 |
| `chromeAmberLighten_3` | 琥珀色\_3 |
| `chromeVioletLighten_3` | 紫色\_3 |
| `chromePinkLighten_3` | 粉色\_3 |

---

## 其他通用结构

### Sort（排序参数）

| 参数 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `field_title` | string | 是 | 需要排序的字段名称 |
| `desc` | bool | 否 | 是否降序排序，默认 false |
