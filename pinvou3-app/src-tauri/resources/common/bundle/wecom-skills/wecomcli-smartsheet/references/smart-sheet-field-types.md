# 字段类型（FieldType）完整参考

## 字段类型枚举

| 参数值 | 说明 | 对应属性（property） |
| --- | --- | --- |
| `text` | 文本 | 无额外属性 |
| `number` | 数字 | `property_number` |
| `checkbox` | 复选框 | `property_checkbox` |
| `date_time` | 日期 | `property_date_time` |
| `image` | 图片 | 无额外属性 |
| `attachment` | 文件 | `property_attachment` |
| `user` | 成员 | `property_user` |
| `url` | 超链接 | `property_url` |
| `select` | 多选 | `property_select` |
| `created_user` | 创建人 | 系统字段，无额外属性 |
| `modified_user` | 最后编辑人 | 系统字段，无额外属性 |
| `created_time` | 创建时间 | `property_created_time` |
| `modified_time` | 最后编辑时间 | `property_modified_time` |
| `progress` | 进度 | `property_progress` |
| `phone_number` | 电话 | 无额外属性 |
| `email` | 邮箱 | 无额外属性 |
| `single_select` | 单选 | `property_single_select` |
| `reference` | 关联 | `property_reference` |
| `location` | 地理位置 | `property_location` |
| `formula` | 公式 | `property_formula` |
| `lookup` | 查找引用 | `property_lookup` |
| `two_way_link_records` | 双向关联 | `property_two_way_link_records` |
| `currency` | 货币 | `property_currency` |
| `wwgroup` | 群 | `property_ww_group` |
| `autonumber` | 自动编号 | `property_auto_number` |
| `percentage` | 百分数 | `property_percentage` |
| `barcode` | 条码 | `property_barcode` |

### 模板中的字段类型

`assets/templates/` 使用 `FIELD_TYPE_*` 常量描述字段类型。调用 `wecom-cli smartsheet sheets add`、`fields add` 或 `fields update` 时，以本节上方“字段类型枚举”表为唯一依据，将模板常量转换为表中的 `参数值`，不能把 `FIELD_TYPE_*` 原样传给接口。

转换规则：去掉 `FIELD_TYPE_` 前缀，将剩余部分转为小写并保留下划线。例如，`FIELD_TYPE_TEXT` 转为 `text`，`FIELD_TYPE_DATE_TIME` 转为 `date_time`，`FIELD_TYPE_TWOWAYLINKRECORDS` 转为 `two_way_link_records`。转换后仍需按枚举表的“对应属性（property）”列补齐相应的 `property_xxx`。

> **暂不支持插入 AI 字段**：相关接口暂不支持创建，若命中此类需求时，告知用户手动创建。

---

## 各字段属性（property）详细参数

### property_number（数字）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `decimal_places` | int (DecimalPlaces) | 小数位数，参考 DecimalPlaces 定义 |
| `use_separate` | bool | 是否千分位分隔（如 1,000） |

### property_checkbox（复选框）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `checked` | bool | 新增时是否默认勾选 |

### property_date_time（日期）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `format` | string (Format) | 日期格式，取值参考 Format 定义 |
| `auto_fill` | bool | 新建记录时是否自动填充时间 |

### property_attachment（文件）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `display_mode` | string (DisplayMode) | 展示样式，参考 DisplayMode 定义 |

### property_user（成员）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `is_multiple` | bool | 允许添加多个人员 |
| `is_notified` | bool | 添加人员时通知用户 |

### property_url（超链接）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `type` | string (LinkType) | 超链接展示样式，参考 LinkType 定义 |

### property_select（多选）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `is_quick_add` | bool | 是否允许填写时新增选项 |
| `options` | Option[] | 选项列表（见下方 Option 结构） |

### property_single_select（单选）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `is_quick_add` | bool | 是否允许填写时新增选项 |
| `options` | Option[] | 选项列表（见下方 Option 结构） |

### property_created_time（创建时间）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `format` | string (Format) | 日期格式，取值参考 Format 定义 |

### property_modified_time（最后编辑时间）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `format` | string (Format) | 日期格式，取值参考 Format 定义 |

### property_progress（进度）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `decimal_places` | int (DecimalPlaces) | 小数位数，参考 DecimalPlaces 定义 |

### property_reference（关联）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `sub_title` | string | 关联的子表名称，不传=关联本子表 |
| `field_title` | string | 关联的字段名称 |
| `is_multiple` | bool | 是否允许多选 |
| `view_id` | string | 视图 id |

### property_location（地理位置）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `input_type` | string (LOCATION_INPUT_TYPE) | 位置输入类型，参考 LOCATION_INPUT_TYPE 定义 |

### property_auto_number（自动编号）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `type` | string (NUMBER_TYPE) | 自动编号类型，参考 NUMBER_TYPE 枚举定义 |
| `rules` | NumberRule[] | 自定义规则，参考 NumberRule 定义 |
| `reformat_existing_record` | bool | 是否应用于已有编号 |

### property_currency（货币）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `currency_type` | string | 货币类型，取值参考 CURRENCY_TYPE 枚举定义 |
| `decimal_places` | int (DecimalPlaces) | 小数位数，参考 DecimalPlaces 定义 |
| `use_separate` | bool | 是否千分位分隔 |

### property_ww_group（群）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `allow_multiple` | bool | 是否允许多个群聊 |

### property_percentage（百分比）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `decimal_places` | int (DecimalPlaces) | 小数位数，参考 DecimalPlaces 定义 |
| `use_separate` | bool | 是否千分位分隔 |

### property_barcode（条码）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `mobile_scan_only` | bool | 仅限手机扫描录入 |

### property_lookup（查找引用）

> 说明：当前仅支持按条件查找模式，`lookup_field_title`、`lookup_sub_title`、`filter`（至少一条 condition）均为必填。

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `lookup_field_title` | string | 引用列查找的字段名称（必填） |
| `rollup_type` | string (RollupType) | 统计类型，参考 RollupType 定义 |
| `lookup_sub_title` | string | 引用的子表名称（必填） |
| `filter` | LookupFilter | 筛选条件（必填，`conditions` 至少一条） |

### LookupFilter（查找筛选）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `conjunction` | string (Conjunction) | 组合方式，参考 Conjunction 定义 |
| `conditions` | LookupCondition[] | 查找条件列表 |

### LookupCondition（查找条件）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `conditionId` | string | 条件 ID（可选） |
| `field_title` | string | 列名称（必填） |
| `fieldType` | string (FieldType) | 列类型（字段类型枚举值） |
| `operator` | string (Operator) | 操作符，见下方 Operator 枚举 |
| `matchValue` | LookupConditionMatchValue | 匹配值（`operator` 非空判断时必填） |

### LookupConditionMatchValue（条件匹配值）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `valueType` | string (LookupConditionValueType) | 匹配方式：`"0"`（和具体值比较） / `"1"`（和列比较） |
| `field_title` | string | 当 `valueType`=`"1"` 时，引用的列名称 |
| `value` | ConditionValue | 当 `valueType`=`"0"` 时，具体匹配值 |
| `computedKeyType` | string (FieldType) | 计算类型（公式等场景） |

### ConditionValue（条件值）

> 此处传值的格式请参考 `references/smart-sheet-record-values.md`。

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `valueText` | StringValue | 文本值 |
| `valueNumber` | NumberValue | 数字值 |
| `valueCheckbox` | BoolValue | 布尔值 |
| `valueDateTime` | FilterDateTimeValue | 日期时间值 |
| `valueUsers` | ListValue | 成员值 |
| `valueSelects` | ListValue | 多选值 |
| `valueSingleSelect` | ListValue | 单选值 |
| `valuePhoneNumber` | StringValue | 电话号码 |
| `valueEmail` | StringValue | 邮箱 |
| `valueReference` | StringValue | 关联引用值 |
| `valueTwoWayLinkRecords` | StringValue | 双向关联值 |
| `valueBarcode` | StringValue | 条形码值 |
| `valuePercentage` | NumberValue | 百分比值 |

### property_lookup 校验规则

| 校验项 | 规则 |
| --- | --- |
| `property_lookup` | 不能为空 |
| `lookup_field_title` | 必填，不能为空字符串 |
| `lookup_sub_title` | 必填，不能为空字符串 |
| `filter.conditions` | 必填，至少包含一条有效条件 |

每条 `LookupCondition` 的校验规则：

| 校验项 | 规则 |
| --- | --- |
| `field_title` | 必填，不能为空 |
| `operator` 是 `is_empty` / `is_not_empty` | 直接通过，不要求 `matchValue` |
| 其他 `operator` | `matchValue` 必须非空：若 `valueType="1"`，则 `field_title` 不能为空；若 `valueType="0"`，则 `value` 至少一个字段非空 |

### property_two_way_link_records（双向关联）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `pad_id` | string | 关联的文档 ID，为空表示当前文档 |
| `sub_title` | string | 关联的子表名称，不传表示本子表 |
| `field_title` | string | 关联的字段名称 |
| `is_multiple` | bool | 是否允许多选 |
| `view_id` | string | 视图 ID |
| `back_field_title` | string | 双向关联的对应列名称 |

### property_formula（公式）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `formulaModel` | FormulaItem[] | 公式表达式模型 |
| `formatter` | Formatter | 展示格式配置 |

`FormulaItem` 和 `Formatter` 的定义和用法请参考 `references/smart-sheet-formula.md`

---

## 通用枚举值

### DecimalPlaces（小数位数）

| 值 | 说明 |
| --- | --- |
| -1 | 显示原值 |
| 0 | 整数 |
| 1~4 | 精确到小数点后 1~4 位 |

### Format（日期格式）

> **重要**：格式中的汉字必须用英文双引号 `"` 包裹，如 `yyyy"年"m"月"d"日"`，**不能**写成 `yyyy年m月d日`

| 格式字符串 | 显示效果 | 说明 |
| --- | --- | --- |
| `yyyy"年"m"月"d"日"` | 2018年4月20日 | 汉字必须用 `"` 包裹 |
| `yyyy"年"m"月"d"日" dddd` | 2018年4月20日 星期五 | 汉字必须用 `"` 包裹 |
| `yyyy"年"m"月"d"日" hh:mm` | 2018年4月20日 14:30 | 汉字必须用 `"` 包裹 |
| `yyyy-mm-dd` | 2018-04-20 | 纯符号无需引号 |
| `yyyy-mm-dd hh:mm` | 2018-04-20 14:30 | 纯符号无需引号 |
| `yyyy/m/d` | 2018/4/20 | 纯符号无需引号 |
| `m/d/yyyy` | 4/20/2018 | 纯符号无需引号 |
| `d/m/yyyy` | 20/4/2018 | 纯符号无需引号 |
| `m"月"d"日"` | 4月20日 | 汉字必须用 `"` 包裹 |

> 日期格式只是日期字段值在智能表格中的显示格式，日期值读写的统一格式为 `"YYYY-MM-DD HH:mm:ss"` 标准时间格式。尽管所有显示格式都不显示秒，但是写入日期字段值时，严禁忽略秒。

**正确示例**：

```json
{ "format": "yyyy\"年\"m\"月\"d\"日\"", "auto_fill": false }
```

**错误示例**（汉字没用引号包裹，会导致格式无效）：

```json
{ "format": "yyyy年m月d日", "auto_fill": false }
```

### DisplayMode（展示样式）

| 参数值 | 说明 |
| --- | --- |
| `list` | 列表模式 |
| `grid` | 网格模式 |

### LinkType（超链接展示样式）

| 参数值 | 说明 |
| --- | --- |
| `pure_text` | 文字 |
| `icon_text` | 图标文字 |

### Option（选项结构）

```json
{ "id": "选项ID", "text": "选项文本", "style": 样式编号 }
```

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 选项 ID（由服务端返回，选已有选项时使用） |
| `text` | string | 选项文本（新增选项时必填） |
| `style` | int (Style) | 颜色 ID 1-27（可选，默认 1） |

style 颜色对照：1=浅红1, 2=浅橙1, 3=浅天蓝1, 4=浅绿1, 5=浅紫1, 6=浅粉1, 7=浅灰1, 8=白, 9=灰, 10=浅蓝1, 11=浅蓝2, 12=蓝, 13=浅天蓝2, 14=天蓝, 15=浅绿2, 16=绿, 17=浅红2, 18=红, 19=浅橙2, 20=橙, 21=浅黄1, 22=浅黄2, 23=黄, 24=浅紫2, 25=紫, 26=浅粉2, 27=粉

### CURRENCY_TYPE（货币类型）

| 参数值 | 说明 |
| --- | --- |
| `cny` | 人民币 |
| `usd` | 美元 |
| `eur` | 欧元 |
| `gbp` | 英镑 |
| `jpy` | 日元 |
| `krw` | 韩元 |
| `hkd` | 港元 |
| `mop` | 澳门元 |
| `twd` | 新台币 |
| `aed` | 阿联酋迪拉姆 |
| `aud` | 澳大利亚元 |
| `brl` | 巴西雷亚尔 |
| `cad` | 加拿大元 |
| `chf` | 瑞士法郎 |
| `idr` | 印尼卢比 |
| `inr` | 印度卢比 |
| `mxn` | 墨西哥比索 |
| `myr` | 马来西亚林吉特 |
| `php` | 菲律宾比索 |
| `pln` | 波兰兹罗提 |
| `rub` | 俄罗斯卢布 |
| `sgd` | 新加坡元 |
| `thb` | 泰国铢 |
| `try` | 土耳其里拉 |
| `vnd` | 越南盾 |

### LOCATION_INPUT_TYPE（位置输入类型）

| 参数值 | 说明 |
| --- | --- |
| `manual` | 手动输入 |
| `auto` | 自动定位，不可手动更新 |

### NUMBER_TYPE（自动编号类型）

| 参数值 | 说明 |
| --- | --- |
| `incr` | 自增 |
| `custom` | 自定义 |

### NumberRule（自动编号规则）

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `type` | string | `incr`(自增) / `fixed_char`(固定字符) / `time`(创建时间) |
| `value` | string | 自增=位数，固定字符=字符串，时间=CreateTimeFormat（见下表） |

### CreateTimeFormat（创建时间格式，用于自动编号）

| 参数值 | 输出示例 |
| --- | --- |
| `YYYYMMDD` | 20260301 |
| `YYYYMM` | 202603 |
| `MMDD` | 0301 |
| `YYYY` | 2026 |
| `MM` | 03 |
| `DD` | 01 |

### RollupType（统计类型）

| 参数值 | 说明 |
| --- | --- |
| `original` | 原样引用，默认值 |
| `unique` | 去重引用 |
| `sum` | 求和 |
| `count` | 计数 |
| `count_unique` | 去重计数 |
| `average` | 平均值 |
| `max` | 最大值 |
| `min` | 最小值 |

### Conjunction（条件组合）

| 参数值 | 说明 |
| --- | --- |
| `and` | 条件与 |
| `or` | 条件或 |

### Operator（查找条件操作符）

| 参数值 | 说明 |
| --- | --- |
| `is` | 等于 |
| `is_not` | 不等于 |
| `contains` | 包含 |
| `does_not_contain` | 不包含 |
| `is_greater` | 大于 |
| `is_greater_or_equal` | 大于或等于 |
| `is_less` | 小于 |
| `is_less_or_equal` | 小于或等于 |
| `is_empty` | 为空 |
| `is_not_empty` | 不为空 |

### LookupConditionValueType（查找条件匹配方式）

| 参数值 | 说明 |
| --- | --- |
| `"0"` | 与具体值比较 |
| `"1"` | 与列比较 |

---

> **添加/更新字段时必须带属性**：日期、超链接、人员、单选、多选、数字等字段类型，**必须带上对应的 `property_xxx` 属性**，否则会报 `调用失败, ret=-1`。只有纯文本(`text`)等简单类型不需要额外属性。
