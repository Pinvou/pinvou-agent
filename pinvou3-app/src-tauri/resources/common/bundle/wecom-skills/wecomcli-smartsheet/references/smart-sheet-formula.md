# 智能表格公式字段使用指南

## 概述

公式字段（`formula`）通过 `property_formula` 定义，其核心是 **`formulaModel`**——一个由多个 `FormulaItem` 组成的数组，用于描述完整的公式表达式。

**关键原则**：
- 所有公式必须通过构建 `formulaModel` 数组来表达
- 公式中的字符串常量使用**双引号** `""` 包裹（写在 `text` 字段中需转义为 `\""`）
- 函数名使用**大写**（如 `SUM`、`FILTER`、`IF`），写在 `type: "text"` 的 `text` 中
- 四则运算遵循数学优先级（`*` `/` 优先于 `+` `-`），需要改变优先级时**必须**用 `{"type":"text","text":"("}` 和 `{"type":"text","text":")"}` 括号分组
- 仅支持**四则运算 + 本文档列出的函数**，不支持取模 `%`、三元表达式 `?:` 等

---

## 一、数据结构

### 1.1 property_formula

公式字段通过 `property_formula` 属性定义，包含两个核心部分：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `formulaModel` | FormulaItem[] | 公式表达式模型，由多个公式项组成的数组 |
| `formatter` | Formatter | 展示格式配置，控制公式计算结果的显示格式 |

### 1.2 FormulaItem（公式项）

每个 `FormulaItem` 是公式中的一个原子片段。

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `type` | string (FormulaType) | 公式项类型枚举，见下方 |
| `text` | string | 文本内容（仅 `type="text"` 时使用） |
| `field_title` | string | 字段名称（`type="field"/"field_ref"` 时使用） |
| `field_type` | string (FieldType) | 字段类型常量（仅 `type="field"` 时使用） |
| `sheet_title` | string | 子表名称（`type="table_ref"/"field_ref"/"table_field_ref"` 时使用） |

> 公式 `formulaModel[].type` 必须传枚举值字符串（如 `"text"`、`"field"`、`"table_ref"` 等），不能写整数。字段用 `field_title`（字段名称）标识，子表用 `sheet_title`（子表名称）标识，无需传 ID。

### 1.3 FormulaType 枚举

| 枚举值 | 说明 | 何时使用 |
| --- | --- | --- |
| `text` | 文本片段 | 运算符 `+` `-` `*` `/`、函数名 `IF(` `SUM(`、常量、括号、参数分隔符等 |
| `field` | 当前记录字段 | 引用当前记录中的字段，需提供 `field_title` 和 `field_type` |
| `table_ref` | 表引用 | 引用整个表，需提供 `sheet_title`，通常配合 `FILTER` 函数 |
| `field_ref` | 列引用 | 在表引用/FILTER 结果后引用具体列，需提供 `field_title` 和 `sheet_title` |
| `table_field_ref` | 表.列引用 | 直接引用某表的某列（返回该列所有值），需提供 `sheet_title` + `field_title` |
| `current_value` | 当前值 | FILTER 等遍历函数中代表当前迭代的记录 |

### 1.4 Formatter（展示格式）

控制公式结果的显示方式，结构与字段属性一致：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `field_type` | string (FieldType) | 展示格式类型 |
| `property_*` | 对应的 FieldProperty | 根据 `field_type` 传入对应属性 |

常见配置：

```json
// 数字：保留2位小数，千分位
{ "field_type": "number", "property_number": { "decimal_places": 2, "use_separate": true } }

// 百分比：保留1位小数
{ "field_type": "percentage", "property_percentage": { "decimal_places": 1, "use_separate": false } }

// 货币：人民币
{ "field_type": "currency", "property_currency": { "currency_type": "cny", "decimal_places": 2, "use_separate": true } }

// 纯文本
{ "field_type": "text" }
```

---

## 二、FormulaItem 各类型用法

### type="text"：文本片段

所有非引用内容都用 `type: "text"` 表示，包括运算符、函数调用语法、常量值等。

```json
{ "type": "text", "text": " + " }          // 加法
{ "type": "text", "text": " * " }          // 乘法
{ "type": "text", "text": " - " }          // 减法
{ "type": "text", "text": " / " }          // 除法
{ "type": "text", "text": "(" }            // 左括号（用于分组，控制运算优先级）
{ "type": "text", "text": ")" }            // 右括号（用于分组，控制运算优先级）
{ "type": "text", "text": "IF(" }          // IF 函数开始
{ "type": "text", "text": "AND(" }         // AND 函数开始
{ "type": "text", "text": ", " }           // 参数分隔
{ "type": "text", "text": ")" }            // 函数/括号结束
{ "type": "text", "text": "\"完成\"" }     // 字符串常量
{ "type": "text", "text": "100" }          // 数字常量
{ "type": "text", "text": ".SUM()" }       // 聚合函数（跟在列引用后）
{ "type": "text", "text": ".AVERAGE()" }   // 聚合函数
{ "type": "text", "text": ".COUNTA()" }    // 聚合函数
{ "type": "text", "text": ".FILTER(" }     // FILTER 函数（跟在表引用后）
{ "type": "text", "text": "." }            // 属性访问点号
{ "type": "text", "text": "TODAY()" }      // 日期函数
{ "type": "text", "text": "MONTH(" }       // 月份函数开始
{ "type": "text", "text": "DATEDIF(" }     // 日期差函数开始
{ "type": "text", "text": ", TODAY(), \"Y\")" }  // DATEDIF 后续参数
{ "type": "text", "text": " = " }          // 等于比较
{ "type": "text", "text": " > " }          // 大于比较
{ "type": "text", "text": " < " }          // 小于比较
{ "type": "text", "text": " <> " }         // 不等于比较
{ "type": "text", "text": " & " }          // 文本连接
```

### type="field"：当前记录字段引用

引用当前记录中的字段值，必须提供 `field_title` 和 `field_type`：

```json
{ "type": "field", "field_title": "单价", "field_type": "number" }
{ "type": "field", "field_title": "备注", "field_type": "text" }
{ "type": "field", "field_title": "截止日期", "field_type": "date_time" }
{ "type": "field", "field_title": "优先级", "field_type": "single_select" }
```

### type="table_ref"：表引用

引用整个表，通常后接 `.FILTER()`：

```json
{ "type": "table_ref", "sheet_title": "订单表" }
```

### type="field_ref"：列引用

在表引用或 FILTER 结果之后，引用具体列。前面必须有 `{ "type": "text", "text": "." }`。必须提供 `field_title` 和 `sheet_title`：

```json
{ "type": "field_ref", "field_title": "金额", "sheet_title": "订单表" }
```

### type="table_field_ref"：表.列引用

直接引用某表某列的所有值（返回数组），通常后接聚合函数：

```json
{ "type": "table_field_ref", "sheet_title": "订单表", "field_title": "金额" }
```

### type="current_value"：当前迭代值

FILTER 中代表当前记录，后接 `.` + `type="field_ref"` 访问该记录的字段：

```json
{ "type": "current_value" }
```

---

## 三、支持的函数

### 3.1 聚合函数

写在 `type: "text"` 的 `text` 中，跟在列引用（`type="table_field_ref"` 或 `type="field_ref"`）之后。

| 函数 | text 值 | 说明 |
| --- | --- | --- |
| SUM | `.SUM()` | 求和 |
| AVERAGE | `.AVERAGE()` | 平均值 |
| MIN | `.MIN()` | 最小值 |
| MAX | `.MAX()` | 最大值 |
| COUNTA | `.COUNTA()` | 非空计数 |
| COUNTIF | `.COUNTIF(条件)` | 条件计数 |
| SUMIF | `.SUMIF(条件)` | 条件求和 |

### 3.2 列表函数

| 函数 | text 值 | 调用方式 | 说明 | 示例 |
| --- | --- | --- | --- | --- |
| FILTER | `.FILTER(` | 点调用，跟在表引用（`type="table_ref"`）后 | 筛选满足条件的记录。内部用 `type="current_value"` 代表当前记录，用 `type="field_ref"` 访问字段。多条件**必须**用 `AND()`/`OR()` 包裹。结束后可 `.列.聚合函数()` 链式调用 | `[表].FILTER(cur.状态 = "完成").任务名.COUNTA()` |
| CONTAINS | `.CONTAINS(` | 点调用，跟在列引用或 `LIST()` 后 | 判断范围中是否包含**任一**查找值，返回 TRUE/FALSE | `LIST(1,2,3,4).CONTAINS(2,5)` → TRUE；`[多选].CONTAINS("选项1","选项2")` |
| CONTAINSALL | `.CONTAINSALL(` | 点调用，跟在列引用或 `LIST()` 后 | 判断范围是否包含**所有**查找值，返回 TRUE/FALSE | `LIST(1,2,3,4).CONTAINSALL(1,2)` → TRUE；`LIST(1,2,3,4).CONTAINSALL(1,2,5)` → FALSE |
| CONTAINSONLY | `.CONTAINSONLY(` | 点调用，跟在列引用或 `LIST()` 后 | 判断范围是否**恰好仅**包含所有查找值（不要求顺序），返回 TRUE/FALSE | `LIST(1,2,3,4).CONTAINSONLY(1,2)` → FALSE；`LIST(1,2,3,4).CONTAINSONLY(1,2,4,3)` → TRUE |
| LOOKUP | `LOOKUP(` | 独立函数调用 | 查找匹配值并返回对应字段。参数：查找值, 匹配列, 返回列, [模式: 1=拆分选项, 0=不拆分] | `LOOKUP([负责人], [人员表].[姓名], [人员表].[部门], 1)` |
| LIST | `LIST(` | 独立函数调用 | 将任意个值组合为一个列表 | `LIST("智","能","表","格")` → `[智,能,表,格]` |
| LISTCOMBINE | `.LISTCOMBINE(` 或 `LISTCOMBINE(` | 点调用或独立调用 | 合并多个列表为一个（嵌套会被展开） | `LISTCOMBINE(LIST(1,2,LIST(3,4)),5,6)` → `[1,2,3,4,5,6]`；`字段1.LISTCOMBINE(字段2)` |
| LISTJOIN | `.LISTJOIN(` | 点调用，跟在列表后 | 用分隔符拼接列表为文本。参数：[分隔符]，默认英文逗号 | `LIST(1,2,3,4).LISTJOIN()` → `1,2,3,4`；`LIST("智","能","表","格").LISTJOIN("-")` → `智-能-表-格` |
| UNIQUE | `.UNIQUE()` | 点调用，跟在列表后 | 列表去重，可链式接聚合函数 | `LIST(1,2,2,3,1).UNIQUE()` → `[1,2,3]`；`[表].[列].UNIQUE().COUNTA()` |

### 3.3 逻辑函数

| 函数 | text 值 | 说明 |
| --- | --- | --- |
| IF | `IF(` | 条件判断，三个参数：条件, 真值, 假值 |
| IFS | `IFS(` | 多条件判断，参数：条件1, 值1, [条件2, ...], [值2, ...]，返回第一个 TRUE 条件对应的结果，比嵌套 IF 可读性更好 |
| AND | `AND(` | 逻辑与，包裹多个条件 |
| OR | `OR(` | 逻辑或，包裹多个条件 |
| TRUE | `TRUE()` | 返回逻辑值 TRUE |
| FALSE | `FALSE()` | 返回逻辑值 FALSE |
| IFBLANK | `IFBLANK(` | 检测值是否为空，为空则返回第二个参数，非空则返回值本身，两个参数：值, 空值情况的返回值 |
| IFERROR | `IFERROR(` | 检查值是否错误，错误则返回指定值，否则返回值本身，两个参数：值, 错误情况的返回值 |
| ISBLANK | `ISBLANK(` | 检测值是否为空，为空返回 TRUE，否则返回 FALSE |
| ISERROR | `ISERROR(` | 检测值是否为错误值，错误值返回 TRUE，否则返回 FALSE |
| ISNULL | `ISNULL(` | 检测值内容是否为空，为空返回 TRUE，否则返回 FALSE（空字符串不为空） |
| SWITCH | `SWITCH(` | 通过和表达式结果比较，按匹配结果返回对应值，如果不匹配，则返回可选默认值。参数：表达式, 值1, 结果1, [值2, ...], [结果2, ...]，末尾可附加一个不配对的参数作为默认值 |

> **重要**：FILTER 和 IF 中如果有多个条件，**必须**用 `AND()` 或 `OR()` 包裹，不能让条件散放。

### 3.4 日期函数

| 函数 | text 值 | 说明 |
| --- | --- | --- |
| TODAY | `TODAY()` | 返回今天日期 |
| NOW | `NOW()` | 返回当前日期和时间 |
| DATE | `DATE(` | 将年、月、日数字转换为日期，参数：年, 月, 日。如 `DATE(2026, 4, 18)` |
| DATEVALUE | `DATEVALUE(` | 将日期字符串转换为数字（距 1900-01-01 的天数）。如 `DATEVALUE("2026/04/18")` |
| TODATE | `TODATE(` | 将文本/字符串转换为日期值（文本→日期的唯一函数）。参数：日期文本。如 `TODATE("2026-5-9")` → `2026/05/09`，`TODATE([日期文本字段])` 将文本字段转为日期 |
| YEAR | `YEAR(` | 获取日期的年份。如 `YEAR("2026-4-20")` 返回 `2026` |
| MONTH | `MONTH(` | 获取日期的月份 |
| DAY | `DAY(` | 获取日期的日。如 `DAY("2026-4-20 10:30:55")` 返回 `20` |
| HOUR | `HOUR(` | 获取时间的小时数。如 `HOUR("2026-4-20 10:30:55")` 返回 `10` |
| MINUTE | `MINUTE(` | 获取时间的分钟数。如 `MINUTE("2026-4-20 10:30:55")` 返回 `30` |
| SECOND | `SECOND(` | 获取时间的秒数。如 `SECOND("2026-4-20 10:30:55")` 返回 `55` |
| WEEKDAY | `WEEKDAY(` | 返回日期对应一周中的第几天，参数：日期值, [类型]。类型用于确定返回值：1 或省略=1(周日)~7(周六)，2=1(周一)~7(周日)，3=0(周一)~6(周日)，11=1(周一)~7(周日)，12=1(周二)~7(周一)，13=1(周三)~7(周二)，14=1(周四)~7(周三)，15=1(周五)~7(周四)，16=1(周六)~7(周五)，17=1(周日)~7(周六) |
| WEEKNUM | `WEEKNUM(` | 返回日期在当前年份的第几周，参数：日期, [类型]。类型表示一周的第 1 天从星期几开始：1 或省略=周日开始，2=周一开始，11=周一开始，12=周二开始，13=周三开始，14=周四开始，15=周五开始，16=周六开始，17=周日开始，21=周一开始(ISO) |
| DATEDIF | `DATEDIF(` | 计算日期差，参数：开始日期, 结束日期, 单位。单位：`"Y"`(年) `"M"`(月) `"D"`(天) |
| NETWORKDAYS | `NETWORKDAYS(` | 返回两个日期之间的净工作日天数（排除周末和指定假期），参数：开始日期, 终止日期, [节假日]。节假日可选，默认仅排除双休日，可传入日期范围或数组常量如 `{"2026/4/19","2026/5/18"}` |
| WORKDAY | `WORKDAY(` | 返回起始日期之前或之后指定工作日数的日期（排除周末和指定假期），参数：起始日期, 天数, [节假日]。节假日可选，默认仅排除双休日，可传入日期范围或数组常量如 `{"2026/4/19","2026/5/18"}` |

### 3.5 数学函数

| 函数 | text 值 | 说明 |
| --- | --- | --- |
| ABS | `ABS(` | 返回数值的绝对值，参数：数值。如 `ABS(-3.5)` 返回 `3.5` |
| CEILING | `CEILING(` | 将数值向上舍入到最接近的指定基数的倍数，参数：数值, 基数。如 `CEILING(2.3, 1)` 返回 `3`，`CEILING(-2.5, 2)` 返回 `-2` |
| FLOOR | `FLOOR(` | 将数值向下舍入到最接近的指定基数的倍数，参数：数值, 基数。如 `FLOOR(2.7, 1)` 返回 `2`，`FLOOR(-2.5, 2)` 返回 `-4` |
| INT | `INT(` | 向下取整为最接近的整数，参数：数值。如 `INT(8.9)` 返回 `8`，`INT(-8.1)` 返回 `-9` |
| ROUND | `ROUND(` | 按指定小数位数四舍五入，参数：数值, 小数位数。如 `ROUND(2.155, 2)` 返回 `2.16`，小数位数可为负数表示到整数位 |
| POWER | `POWER(` | 返回数值的指定次幂，参数：底数, 指数。如 `POWER(2, 10)` 返回 `1024` |
| SQRT | `SQRT(` | 返回数值的平方根，参数：数值（必须为非负数）。如 `SQRT(16)` 返回 `4` |
| EXP | `EXP(` | 返回 e 的指定次幂，参数：指数。如 `EXP(1)` 返回 `2.71828...` |
| LOG | `LOG(` | 返回数值以指定数为底的对数，参数：数值, [底数]。底数省略时默认为 10。如 `LOG(100, 10)` 返回 `2`，`LOG(8, 2)` 返回 `3` |
| RAND | `RAND()` | 返回一个大于等于 0 且小于 1 的随机数，无参数 |

### 3.6 文本函数

| 函数 | text 值 | 说明 |
| --- | --- | --- |
| TEXTJOIN | `TEXTJOIN(` | 将多个文本值组合并在之间插入分隔符，参数：分隔符(文本字符串), 是否忽略空白值(TRUE/FALSE), 文本1, [文本2, ...]。如 `TEXTJOIN(" ", TRUE, "hello", "world")` 返回 `"hello world"`，分隔符为空字符串 `""` 时直接拼接 |
| & | ` & ` | 文本连接运算符 |
| CHAR | `CHAR(` | 返回数字代码所对应的 Unicode 字符，参数：数字。常用：`CHAR(10)` 换行符、`CHAR(32)` 空格、`CHAR(48~57)` 数字 0~9、`CHAR(65~90)` 大写字母 A~Z、`CHAR(97~122)` 小写字母 a~z |
| CONCAT | `CONCAT(` | 将多个文本拼接成单个文本，参数：文本1, [文本2, ...]。若要拼接双引号字符，需连续输入两个双引号 `""""`。如 `CONCAT([姓名], "-", [年龄])` → `小明-28` |
| CONTAINTEXT | `CONTAINTEXT(` | 判断文本中是否包含要查找的文本，返回 TRUE/FALSE，参数：文本, 查找文本。如 `CONTAINTEXT("智能表格", "表格")` → TRUE |
| FIND | `FIND(` | 从指定位置开始查找值，找到值在查找范围中第一次出现的位置，参数：查找的值, 查找范围, [起始位置]。起始位置默认为 1。如 `FIND("花", "人面桃花相映红")` → `4`；`FIND("红", LIST("人","面","桃","花","相","映","红"))` → `7` |
| LEFT | `LEFT(` | 从左提取字符串指定长度的子串，参数：字符串, [字符数]。如 `LEFT("人面桃花相映红", 2)` → `人面` |
| LEN | `LEN(` | 返回文本字符串中的字符个数（空格计为字符），参数：文本。如 `LEN("abcd")` → `4` |
| LOWER | `LOWER(` | 将文本中的全部大写字母替换为小写字母，参数：文本。如 `LOWER("SmartSheet")` → `smartsheet` |
| MID | `MID(` | 提取字符串中从指定开始位置开始的指定长度的子串，参数：文本, 开始位置, 提取长度。位置从 1 开始。如 `MID("腾讯文档智能表格", 5, 4)` → `智能表格` |
| REPLACE | `REPLACE(` | 将文本中指定位置和长度的部分替换为新文本，参数：文本, 位置, 长度, 新文本。如 `REPLACE("人面桃花相映红", -5, -1, "梨")` → `人面梨花相映红` |
| RIGHT | `RIGHT(` | 从右提取字符串指定长度的子串，参数：字符串, [字符数]。如 `RIGHT("人面桃花相映红", 2)` → `映红` |
| SEARCH | `SEARCH(` | 在被查询文本中查找查询文本，返回第一次出现的起始位置（从 1 开始），参数：查询文本, 被查询文本, [编号]。编号为开始搜索的字符位置。如 `SEARCH("e", "Hello", 1)` → `2` |
| SPLIT | `SPLIT(` | 使用分隔符对文本进行分割，返回列表，参数：文本, 分隔符。如 `SPLIT("智-能-表-格", "-")` → `["智","能","表","格"]` |
| SUBSTITUTE | `SUBSTITUTE(` | 在文本中用新文本替代指定的旧文本，参数：文本, 被替换文本, 新文本, [被替换文本序号]。序号省略时替换所有，指定序号时只替换第 N 个出现。如 `SUBSTITUTE("hello world", "hello", "Hello")` → `Hello world` |
| TEXT | `TEXT(` | 按指定格式将数值/日期转为文本，参数：数值, 格式。常用格式：`"YYYY/MM/DD"` 年月日、`"DDDD"` 星期全称、`"DDD"` 星期简称、`"0.0%"` 百分比。如 `TEXT("2026-05-14", "ddd")` → `周四` |
| TRIM | `TRIM(` | 移除文本最前和最后的空格，参数：文本。如 `TRIM(" 智能 表格 ")` → `智能 表格`（中间空格保留） |
| UPPER | `UPPER(` | 将文本中的全部小写字母替换为大写字母，参数：文本。如 `UPPER("SmartSheet")` → `SMARTSHEET` |
| VALUE | `VALUE(` | 将表示数值的文本字符串转换为数值，参数：文本。如 `VALUE("1,000")` → `1000` |

---

## 四、完整 formulaModel 示例

### 示例 1：简单乘法

语义：`单价 * 数量`

```json
{
  "formulaModel": [
    { "type": "field", "field_title": "单价", "field_type": "number" },
    { "type": "text", "text": " * " },
    { "type": "field", "field_title": "数量", "field_type": "number" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 2, "use_separate": true }
  }
}
```

### 示例 2：括号分组 — 折后率

语义：`(原价 - 折后价) / 原价`

> **注意**：当需要改变默认运算优先级时，**必须**使用括号 `(` `)` 进行分组。括号也是 `type: "text"` 的项。如果不加括号，`原价 - 折后价 / 原价` 会先算除法再算减法，结果完全错误。

```json
{
  "formulaModel": [
    { "type": "text", "text": "(" },
    { "type": "field", "field_title": "原价", "field_type": "number" },
    { "type": "text", "text": " - " },
    { "type": "field", "field_title": "折后价", "field_type": "number" },
    { "type": "text", "text": ")" },
    { "type": "text", "text": " / " },
    { "type": "field", "field_title": "原价", "field_type": "number" }
  ],
  "formatter": {
    "field_type": "percentage",
    "property_percentage": { "decimal_places": 2, "use_separate": false }
  }
}
```

### 示例 3：条件判断

语义：如果 状态="完成" 则显示"是"，否则显示"否"

```json
{
  "formulaModel": [
    { "type": "text", "text": "IF(" },
    { "type": "field", "field_title": "状态", "field_type": "single_select" },
    { "type": "text", "text": " = \"完成\", \"是\", \"否\")" }
  ],
  "formatter": { "field_type": "text" }
}
```

### 示例 4：聚合 — 对某表某列求和

语义：订单表的金额列求和

```json
{
  "formulaModel": [
    { "type": "table_field_ref", "sheet_title": "订单表", "field_title": "金额" },
    { "type": "text", "text": ".SUM()" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 2, "use_separate": true }
  }
}
```

### 示例 5：聚合 — 对某表某列求平均值

语义：订单表的金额列平均值

```json
{
  "formulaModel": [
    { "type": "table_field_ref", "sheet_title": "订单表", "field_title": "金额" },
    { "type": "text", "text": ".AVERAGE()" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 2, "use_separate": true }
  }
}
```

### 示例 6：聚合 — 对某表某列计数

语义：订单表的订单号列非空计数

```json
{
  "formulaModel": [
    { "type": "table_field_ref", "sheet_title": "订单表", "field_title": "订单号" },
    { "type": "text", "text": ".COUNTA()" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 0, "use_separate": false }
  }
}
```

### 示例 7：FILTER + 聚合

语义：筛选任务表中 状态="完成" 的记录，对任务名列计数

```json
{
  "formulaModel": [
    { "type": "table_ref", "sheet_title": "任务表" },
    { "type": "text", "text": ".FILTER(" },
    { "type": "current_value" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "状态", "sheet_title": "任务表" },
    { "type": "text", "text": " = \"完成\")" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "任务名", "sheet_title": "任务表" },
    { "type": "text", "text": ".COUNTA()" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 0, "use_separate": false }
  }
}
```

### 示例 8：FILTER + 多条件（AND）

语义：筛选任务表中 截止日期 < 今天 且 状态 ≠ "完成" 的记录，对任务名计数

```json
{
  "formulaModel": [
    { "type": "table_ref", "sheet_title": "任务表" },
    { "type": "text", "text": ".FILTER(AND(" },
    { "type": "current_value" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "截止日期", "sheet_title": "任务表" },
    { "type": "text", "text": " < TODAY(), " },
    { "type": "current_value" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "状态", "sheet_title": "任务表" },
    { "type": "text", "text": " <> \"完成\"))" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "任务名", "sheet_title": "任务表" },
    { "type": "text", "text": ".COUNTA()" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 0, "use_separate": false }
  }
}
```

### 示例 9：FILTER + 金额求和

语义：筛选订单表中 金额 > 10000 的记录，对金额列求和

```json
{
  "formulaModel": [
    { "type": "table_ref", "sheet_title": "订单表" },
    { "type": "text", "text": ".FILTER(" },
    { "type": "current_value" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "金额", "sheet_title": "订单表" },
    { "type": "text", "text": " > 10000)" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "金额", "sheet_title": "订单表" },
    { "type": "text", "text": ".SUM()" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 2, "use_separate": true }
  }
}
```

### 示例 10：FILTER + MONTH 日期筛选

语义：筛选订单表中 日期的月份 = 今天月份 的记录，对金额列求和

```json
{
  "formulaModel": [
    { "type": "table_ref", "sheet_title": "订单表" },
    { "type": "text", "text": ".FILTER(MONTH(" },
    { "type": "current_value" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "日期", "sheet_title": "订单表" },
    { "type": "text", "text": ") = MONTH(TODAY()))" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "金额", "sheet_title": "订单表" },
    { "type": "text", "text": ".SUM()" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 2, "use_separate": true }
  }
}
```

### 示例 11：日期差计算

语义：从入职日期到今天的年数

```json
{
  "formulaModel": [
    { "type": "text", "text": "DATEDIF(" },
    { "type": "field", "field_title": "入职日期", "field_type": "date_time" },
    { "type": "text", "text": ", TODAY(), \"Y\")" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 0, "use_separate": false }
  }
}
```

### 示例 12：文本连接

语义：姓 & 名

> 字符串常量必须用双引号包裹（`\"...\"`），且每个片段之间**必须**用 `&` 连接。不能将字符串常量和字段引用直接相邻排列，否则公式无法正确执行。

```json
{
  "formulaModel": [
    { "type": "field", "field_title": "姓", "field_type": "text" },
    { "type": "text", "text": " & " },
    { "type": "field", "field_title": "名", "field_type": "text" }
  ],
  "formatter": { "field_type": "text" }
}
```

### 示例 13：嵌套条件 IF + AND

语义：如果 截止日期 < 今天 且 状态 ≠ "完成"，显示"超期"，否则显示"正常"

```json
{
  "formulaModel": [
    { "type": "text", "text": "IF(AND(" },
    { "type": "field", "field_title": "截止日期", "field_type": "date_time" },
    { "type": "text", "text": " < TODAY(), " },
    { "type": "field", "field_title": "状态", "field_type": "single_select" },
    { "type": "text", "text": " <> \"完成\"), \"超期\", \"正常\")" }
  ],
  "formatter": { "field_type": "text" }
}
```

### 示例 14：SUMIF 条件求和

语义：对商品销售表的销售额列，仅对值 > 1000 的求和

```json
{
  "formulaModel": [
    { "type": "table_field_ref", "sheet_title": "销售表", "field_title": "销售额" },
    { "type": "text", "text": ".SUMIF(" },
    { "type": "current_value" },
    { "type": "text", "text": " > 1000)" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 2, "use_separate": true }
  }
}
```

### 示例 15：除法 — 完成率

语义：已完成任务数 / 总任务数

> 当 formatter 设置为 `percentage` 时，公式只需返回小数值（如 0.8），系统会自动显示为百分比（80%）。

```json
{
  "formulaModel": [
    { "type": "table_ref", "sheet_title": "任务表" },
    { "type": "text", "text": ".FILTER(" },
    { "type": "current_value" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "状态", "sheet_title": "任务表" },
    { "type": "text", "text": " = \"完成\")" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "任务名", "sheet_title": "任务表" },
    { "type": "text", "text": ".COUNTA() / " },
    { "type": "table_field_ref", "sheet_title": "任务表", "field_title": "任务名" },
    { "type": "text", "text": ".COUNTA()" }
  ],
  "formatter": {
    "field_type": "percentage",
    "property_percentage": { "decimal_places": 1, "use_separate": false }
  }
}
```

### 示例 16：关联字段引用

语义：通过关联字段"项目"访问被关联表的"预算"字段

```json
{
  "formulaModel": [
    { "type": "field", "field_title": "项目", "field_type": "reference" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "预算", "sheet_title": "项目表" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 2, "use_separate": true }
  }
}
```

### 示例 17：FILTER + 部门匹配（引用当前记录字段）

语义：筛选员工表中 部门 = 当前记录的部门 的记录，对姓名列计数

```json
{
  "formulaModel": [
    { "type": "table_ref", "sheet_title": "员工表" },
    { "type": "text", "text": ".FILTER(" },
    { "type": "current_value" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "部门", "sheet_title": "员工表" },
    { "type": "text", "text": " = " },
    { "type": "field", "field_title": "部门", "field_type": "single_select" },
    { "type": "text", "text": ")" },
    { "type": "text", "text": "." },
    { "type": "field_ref", "field_title": "姓名", "sheet_title": "员工表" },
    { "type": "text", "text": ".COUNTA()" }
  ],
  "formatter": {
    "field_type": "number",
    "property_number": { "decimal_places": 0, "use_separate": false }
  }
}
```

### 示例 18：LOOKUP 跨子表查找

语义：在"考勤表"中根据当前记录的 `员工姓名` 到同一智能表格下的"员工表"中匹配 `姓名`，返回对应的 `部门`。

> **关键点**：
> - `LOOKUP` 是独立函数调用，以 `LOOKUP(` 开头，参数之间用 `, ` 分隔。
> - 四个参数依次为：**查找值、匹配列、返回列、模式**。
> - 查找值用 `type="field"` 引用当前记录字段；匹配列/返回列用 `type="table_field_ref"` 直接引用目标子表的列（需 `sheet_title` + `field_title`）。
> - 模式 `0` = 不拆分（整体匹配，适用于文本/数字等单值字段）；模式 `1` = 拆分多选选项（见示例 19）。

```json
{
  "formulaModel": [
    { "type": "text", "text": "LOOKUP(" },
    { "type": "field", "field_title": "员工姓名", "field_type": "text" },
    { "type": "text", "text": ", " },
    { "type": "table_field_ref", "sheet_title": "员工表", "field_title": "姓名" },
    { "type": "text", "text": ", " },
    { "type": "table_field_ref", "sheet_title": "员工表", "field_title": "部门" },
    { "type": "text", "text": ", 0)" }
  ],
  "formatter": { "field_type": "text" }
}
```

### 示例 19：LOOKUP 多选字段拆分匹配

语义：当前记录的 `负责人` 字段是多选（可能包含多个员工），需要按每个选项分别在"员工表"中匹配 `姓名` 并返回对应的 `部门` 列表。此时模式参数用 `1`，LOOKUP 会把多选值拆开逐个查找。

```json
{
  "formulaModel": [
    { "type": "text", "text": "LOOKUP(" },
    { "type": "field", "field_title": "负责人", "field_type": "select" },
    { "type": "text", "text": ", " },
    { "type": "table_field_ref", "sheet_title": "员工表", "field_title": "姓名" },
    { "type": "text", "text": ", " },
    { "type": "table_field_ref", "sheet_title": "员工表", "field_title": "部门" },
    { "type": "text", "text": ", 1)" }
  ],
  "formatter": { "field_type": "text" }
}
```

---

## 五、通过 API 创建公式字段

通过 `wecom-cli smartsheet fields add` 命令传入公式字段定义：

```json
{
  "docid": "<docid>",
  "sheet_title": "<子表名称>",
  "fields": [
    {
      "field_title": "总价",
      "field_type": "formula",
      "property_formula": {
        "formulaModel": [
          { "type": "field", "field_title": "单价", "field_type": "number" },
          { "type": "text", "text": " * " },
          { "type": "field", "field_title": "数量", "field_type": "number" }
        ],
        "formatter": {
          "field_type": "number",
          "property_number": { "decimal_places": 2, "use_separate": true }
        }
      }
    }
  ]
}
```

---

## 六、formulaModel 构建模式总结

### 模式 A：当前记录字段运算

`字段A op 字段B`

```
[type="field", field_title=字段A] → [type="text", " op "] → [type="field", field_title=字段B]
```

需要括号分组时：`(字段A op 字段B) op2 字段C`

```
[type="text", "("] → [type="field", field_title=字段A] → [type="text", " op "] → [type="field", field_title=字段B] → [type="text", ")"] → [type="text", " op2 "] → [type="field", field_title=字段C]
```

### 模式 B：表.列聚合

`表.列.聚合函数()`

```
[type="table_field_ref", sheet_title+field_title] → [type="text", ".SUM()"]
```

### 模式 C：FILTER + 列聚合

`表.FILTER(条件).列.聚合函数()`

```
[type="table_ref", sheet_title] → [type="text", ".FILTER("] → 条件部分 → [type="text", ")"] → [type="text", "."] → [type="field_ref", field_title+sheet_title] → [type="text", ".COUNTA()"]
```

### 模式 D：FILTER 条件内部

访问当前记录的字段：
```
[type="current_value"] → [type="text", "."] → [type="field_ref", field_title+sheet_title] → [type="text", " = \"值\""]
```

多条件必须用 AND/OR 包裹：
```
[type="text", "AND("] → 条件1 → [type="text", ", "] → 条件2 → [type="text", ")"]
```

### 模式 E：IF 条件判断

```
[type="text", "IF("] → 条件部分 → [type="text", ", \"真值\", \"假值\")"]
```

### 模式 F：关联字段引用

```
[type="field", field_title=关联字段, "reference"] → [type="text", "."] → [type="field_ref", field_title+sheet_title(被关联表字段)]
```

### 模式 G：文本拼接（字符串常量 & 字段引用）

字符串常量**必须**用双引号包裹，片段之间**必须**用 `&` 连接，**不能直接相邻**。

`"常量文本A" & 字段B & "常量文本C"`

```
[type="text", "\"常量文本A\""] → [type="text", " & "] → [type="field", field_title=字段B] → [type="text", " & "] → [type="text", "\"常量文本C\""]
```

---

## 七、常见错误

### 7.1 FILTER/IF 多条件未用 AND/OR 包裹

```json
// 错误：条件散放
{ "type": "text", "text": ".FILTER(" },
// ... 条件1 ...
{ "type": "text", "text": ", " },
// ... 条件2 ...
{ "type": "text", "text": ")" }

// 正确：用 AND 包裹
{ "type": "text", "text": ".FILTER(AND(" },
// ... 条件1 ...
{ "type": "text", "text": ", " },
// ... 条件2 ...
{ "type": "text", "text": "))" }
```

### 7.2 type="field" 缺少 field_type

```json
// 错误
{ "type": "field", "field_title": "单价" }

// 正确
{ "type": "field", "field_title": "单价", "field_type": "number" }
```

### 7.3 type="table_field_ref" 缺少 sheet_title 或 field_title

```json
// 错误
{ "type": "table_field_ref", "field_title": "金额" }

// 正确
{ "type": "table_field_ref", "sheet_title": "订单表", "field_title": "金额" }
```

### 7.4 对文本字段误用数学运算

文本连接应使用 `&`，不能用 `+`。

### 7.5 四则运算缺少括号导致优先级错误

`*` `/` 优先级高于 `+` `-`。当需要先做加减再做乘除时，**必须**用括号分组。

```json
// 错误：想算 (A - B) / A，但实际计算的是 A - (B / A)
[
  { "type": "field", "field_title": "原价", "field_type": "number" },
  { "type": "text", "text": " - " },
  { "type": "field", "field_title": "折后价", "field_type": "number" },
  { "type": "text", "text": " / " },
  { "type": "field", "field_title": "原价", "field_type": "number" }
]

// 正确：用 type="text" 的 "(" 和 ")" 包裹需要优先计算的部分
[
  { "type": "text", "text": "(" },
  { "type": "field", "field_title": "原价", "field_type": "number" },
  { "type": "text", "text": " - " },
  { "type": "field", "field_title": "折后价", "field_type": "number" },
  { "type": "text", "text": ")" },
  { "type": "text", "text": " / " },
  { "type": "field", "field_title": "原价", "field_type": "number" }
]
```

> **规则**：遇到混合使用 `+-` 和 `*/` 的表达式，先写出数学公式，确认哪些部分需要括号，然后在 formulaModel 中对应位置插入 `{"type":"text","text":"("}` 和 `{"type":"text","text":")"}` 。

### 7.6 不支持的运算符/语法

公式系统**不支持**：
- 取模运算 `%`
- 三元表达式 `?:`
- 本文档未列出的任何函数
遇到不支持的需求应提示用户。
