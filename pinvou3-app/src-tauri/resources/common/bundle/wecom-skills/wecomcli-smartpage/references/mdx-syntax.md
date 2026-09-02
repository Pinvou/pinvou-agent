# MDX 语法参考

智能文档使用 MDX 语法编写页面内容，支持所有 Markdown 标准语法，并扩展了以下自定义组件。

> [前置依赖] 编写公式前请查阅 [公式参考](formula-reference.md)。本文档未提及的组件不要创造，否则会作为普通文本插入，导致页面不可读。

## smartpage 和 page 标签

```markdown
<smartpage>
<page title="页面 1">
  # 页面标题
  <card color="blue">
    子页面内部可以使用我们扩展的 Markdown 语法
  </card>
  <page title="页面 1 的子页面">
    子页面之间可以嵌套
  </page>
</page>
<page title="页面 2">
也可以并列
</page>
</smartpage>
```

使用规则：

- smartpage 和 page 标签是必要的
- 除非用户特意要求，使用单页面来承载内容
- 智能文档和子页面的标题应该符合对应内容的语义
- 如果使用嵌套页面，要满足总-分的结构
- **<page> 标签使用规范**: 
  - **新建智能文档场景**（使用 `wecom-cli smartpage import` 完成 Markdown 导入时）：使用 `<page title="xxx">` 控制首页标题，此时 title 必填
  - **追加/覆盖已有页面场景**（`wecom-cli smartpage pages append` / `wecom-cli smartpage pages overwrite`）：当前已存在页面结构，markdown 不需要再包含 `<page>` 标签，否则会作为普通文本插入到页面中
- **title 属性不要 HTML 转义**：`<page title="...">` 中的 title 值是纯文本标题，`&`、`<`、`>` 等字符**直接书写即可**，不要转义为 `&amp;`、`&lt;`、`&gt;`。

## 文本

```markdown
普通文本
**加粗文本**
_斜体文本_
~~删除线~~
```

## 富文本

```markdown
这是一个<span style="color: blue; background-color: light_red_background">蓝色前景且红色背景的文字</span>
```

## 高亮卡片

```markdown
<card color="blue">
<span style="color:blue">用于展示需要**突出**，也常与分栏共用实现更好的**对比**和**并列**效果。</span>
- 也可直接内嵌 Markdown 语法
</card>
```

> [注意] 卡片内部的字体颜色必须与卡片颜色一致，以达到更好的视觉统一效果

## 分栏布局

```markdown
<grid>
<area width-ratio="0.5">左侧内容，占 50% 宽度</area>
<area width-ratio="0.5">右侧内容，占 50% 宽度</area>
</grid>
```

- `width-ratio`：子容器宽度占比，范围 0.1~1.0，所有的子容器宽度占比之和为 1
- 分栏内可以嵌套卡片、列表、文本等内容
- 分栏的 area 元素可以内嵌 markdown 语法，个数大于等于 2

## 列表

**有序列表**：当各项内容之间存在依赖关系、时间先后或等级排名时使用

```markdown
1. 第一步
2. 第二步
3. 第三步
```

**无序列表**：当各项内容是并列关系时使用

```markdown
- 苹果
- 香蕉
- 橙子
```

## 分割线

```markdown
---
```

## 居中与对齐

```markdown
<div align="center">
使用 align 属性可以居中/左右对齐（center/left/right）一个段落或标题
</div>
```

## 链接

外部链接使用 Markdown 标准链接语法：

```markdown
[访问 Google](https://www.google.com)
```

如果你不确定资源对应的外部链接，使用`#`作为代替，例如

```markdown
[市场调研分析](#)
```

## 颜色

### 字体颜色（font-color）

| 值 | 效果 |
| --- | --- |
| default | 默认颜色 |
| grey | 灰色 |
| red | 红色 |
| orange | 橙色 |
| yellow | 黄色 |
| green | 绿色 |
| cyan | 青色 |
| blue | 蓝色 |
| accent_blue | 强调蓝 |
| purple | 紫色 |

### 背景颜色（background-color）

| 值 | 效果 |
| --- | --- |
| default_background | 默认背景 |
| light_grey_background | 浅灰背景 |
| grey_background | 灰色背景 |
| dark_background | 深色背景 |
| light_red_background | 浅红背景 |
| red_background | 红色背景 |
| light_orange_background | 浅橙色背景 |
| orange_background | 橙色背景 |
| light_yellow_background | 浅黄色背景 |
| yellow_background | 黄色背景 |
| light_green_background | 浅绿色背景 |
| green_background | 绿色背景 |
| light_cyan_background | 浅青色背景 |
| cyan_background | 青色背景 |
| light_blue_background | 浅蓝色背景 |
| blue_background | 蓝色背景 |
| light_accent_blue_background | 浅强调蓝背景 |
| accent_blue_background | 强调蓝背景 |
| light_purple_background | 浅紫色背景 |
| purple_background | 紫色背景 |

### 卡片颜色（card color）

| 值 | 效果 |
| --- | --- |
| blue | 蓝色卡片 |
| dark_blue | 深蓝色卡片 |
| green | 绿色卡片 |
| dark_green | 深绿色卡片 |
| yellow | 黄色卡片 |
| dark_yellow | 深黄色卡片 |
| red | 红色卡片 |
| dark_red | 深红色卡片 |
| purple | 紫色卡片 |
| dark_purple | 深紫色卡片 |
| gray | 灰色卡片 |
| dark_gray | 深灰色卡片 |
| orange | 橙色卡片 |
| dark_orange | 深橙色卡片 |
| cyan | 青色卡片 |
| dark_cyan | 深青色卡片 |
| indigo | 靛蓝卡片 |
| dark_indigo | 深靛蓝卡片 |

> [提示] AI 生成内容时优先使用浅色系卡片（如蓝色、绿色、黄色等），以获得更好的视觉效果和可读性

## 待办事项

使用原生 Markdown 任务列表语法，无需自定义标签：

```markdown
- [ ] 待完成的任务
- [x] 已完成的任务
```

## `<image>` 图片

编写 `image` 的 MDX 内容前，需要先调用 `wecom-cli smartpage images upload` 上传图片，获取图片 URL。

```markdown
<image src="图片url"/>
```

属性表：

| 属性 / 内容 | 必填 | 说明 |
| --- | --- | --- |
| `align` | 否 | 图片对齐方式 |
| `size` | 否 | 图片尺寸 |

## `<formulaSpan>` 公式Span

内联公式组件，标签内文本即公式字符串。

```markdown
<formulaSpan id="本月销售额">[订单表].FILTER(MONTH([Each].[日期]) = MONTH(TODAY())).[金额].SUM()</formulaSpan>
```

属性表：

| 属性 / 内容 | 必填 | 说明 |
| --- | --- | --- |
| `id` | 否 | 公式名称，可供其它公式通过 [页面名.公式名] 引用 |

使用规则：

- 公式内容直接写在标签内，必填，公式中的特殊符号需 XML 转义（`<` → `&lt;`、`>` → `&gt;`、`&` → `&amp;`、`"` → `&quot;`）

> [提示] 普通 Markdown 文本中，`&` 等特殊字符无需转义，直接书写即可。XML 转义仅在特定组件内部需要（如 `<formulaSpan>` 公式内容的标签体内）

## `<input>` 输入框

文本输入控件，输入结果可被按钮公式、图表筛选等场景读取。

```markdown
<input name="姓名输入框" placeholder="请输入姓名" defaultValue="纯文本预填值" defaultValueFormula="">
  <style size="large"></style> 
</input>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `name` | 是 | 控件唯一标识，按钮公式中用 `[页面名.控件名]` 引用；也供图表/表格筛选条件通过 `valueScBlockId` 引用 |
| `placeholder` | 否 | 占位提示文字 |
| `defaultValue` | 否 | 纯文本预填值，与 `defaultValueFormula` 互斥 |
| `defaultValueFormula` | 否 | 公式预填值（如 `USER()`），与 `defaultValue` 互斥 |
| `style` | 否 | 样式子标签，属性包含：`size` 可选 `medium` / `large`，`width` 可选 `auto` / `fill`，`align` 可选 `left` / `mid` / `right` |

## `<select>` 选择器

```markdown
<select id="select_1" name="城市选择器" placeholder="请选择城市" allowMultiple="false" allowAddOption="true">
  <options>
    <option>北京</option>
    <option>上海</option>
  </options>
  <defaultValue>北京</defaultValue>
  <style size="large"></style>
</select>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `id` | 否 | 控件唯一标识，按钮公式中用 [页面名.控件id] 引用，也供图表/表格筛选条件通过 valueScBlockId 引用 |
| `name` | 否 | 控件名称 |
| `placeholder` | 否 | 占位提示文字 |
| `defaultValue` | 否 | 纯文本预填值 |
| `allowMultiple` | 否 | 是否允许多选，可选 `true` / `false` |
| `allowAddOption` | 否 | 是否允许用户在下拉选项中新增选项，可选 `true` / `false` |
| `options.option` | 否 | 预设的下拉选项，多个 `<option>` 标签定义多个可选项 |
| `style` | 否 | 样式子标签，属性包含：`size` 可选 `medium` / `large`，`width` 可选 `auto` / `fill` |


## `<datePicker>` 日期选择器

日期输入控件，所选日期可被按钮公式、图表筛选等场景读取。

```markdown
<datePicker id="date_1" name="控件名称" placeholder="未选择时的提示文字" format="YYYY-MM-DD" defaultValue="2026-01-01">
  <style size="large"></style>
</datePicker>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `id` | 否 | 逻辑ID，供图表筛选条件引用 |
| `name` | 否 | 控件名称 |
| `placeholder` | 否 | 未选择时的提示文字 |
| `format` | 否 | 日期格式，默认 `YYYY-MM-DD`；可选 `YYYY年M月D日` / `YYYY/M/D` / `M月D日` / `M/D/YYYY` / `D/M/YYYY` / `YYYY年M月D日 HH:mm` / `YYYY-MM-DD HH:mm` |
| `defaultValue` | 否 | 默认日期，格式 `YYYY-MM-DD` |
| `style` | 否 | 样式子标签，属性包含：`size` 可选 `medium` / `large`，`width` 可选 `auto` / `fill` |


## `<button>` 按钮

按钮控件，点击时执行 `formulaString` 中的公式。

```markdown
<button id="button_1" displayValue="提交到表格" formulaString="ADDRECORD([成绩表], [成绩表.姓名], [学生成绩提交页.姓名输入框])">
  <style size="large" color="blue"></style>
</button>
```

属性表：

| 属性 | 必填 | 说明 |
| --- | --- | --- |
| `id` | 否 | 控件唯一标识，用于公式引用 |
| `displayValue` | 否 | 按钮显示文字，默认 `按钮` |
| `formulaString` | 是 | 触发公式，如 `[表名.字段名]` 或 `[页面名.控件id]` |
| `style` | 否 | 样式字符串，分号分隔；`size` 可选 `medium` / `large`，`color` 可选 `blue` / `red` / `gray` / `white` |

## 图表组件

> **前置依赖**：所有统计图表（`<columnChart>` / `<barChart>` / `<lineChart>` / `<pieChart>` / `<comboChart>` / `<statisticsChart>` / `<wordCloudChart>`）以及 `<smartsheetView>` 均需基于智能文档**内置绑定的智能表格**。
> 创建智能文档后，通过 `wecom-cli smartpage databases get` 获取内置数据表的子表 ID，再委托 `wecomcli-smartsheet` 技能完成数据表建设（创建子表 / 字段），最后再编写页面的 mdx 内容。**不要**使用外部独立创建的智能表格。

### `<filterInfo>` 筛选条件

图表、智能表格视图等组件通用的筛选条件容器。

```markdown
<filterInfo type="custom" conjunction="and">
  <conditions>
    <!-- 静态筛选：直接使用 value -->
    <condition fieldId="日期字段" operator="is" value="2026-01-15"></condition>
    <!-- 动态筛选：引用上方控件逻辑 id（如 input_1） -->
    <condition fieldId="姓名" operator="contains" valueScBlockId="input_1"></condition>
    <!-- 单选/多选字段筛选（option 类型）：使用 value 绑定选项名称 -->
    <condition fieldId="状态" operator="is" value="已完成"></condition>
  </conditions>
</filterInfo>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `type` | 是 | 固定值 `custom` |
| `conjunction` | 是 | 多条件逻辑关系，可选 `and` / `or` |
| `<condition>` | 是 | 筛选条件项，可包含多条 |
| `condition.fieldId` | 是 | 筛选字段名 |
| `condition.operator` | 是 | 可选值：`is` / `is_not` / `contains` / `does_not_contain` / `is_greater` / `is_greater_or_equal` / `is_less` / `is_less_or_equal` / `is_empty` / `is_not_empty` |
| `condition.value` | 否 | 静态筛选值，与 `valueScBlockId` 互斥 |
| `condition.valueScBlockId` | 否 | 动态绑定控件的 `id`，与 `value` 互斥 |

使用规则：

- 多条件之间的关系由 `conjunction` 决定，全部组件共用此规则
- **时间类型字段筛选**：当筛选参数为时间时，`value` 必须传入 `YYYY-mm-dd` 格式的字符串，如 `2026-01-15`，且 `operator` 支持选择 `is` / `is_not` / `is_greater` / `is_less` / `is_empty` / `is_not_empty`，其余均不支持，传入将导致组件数据不可用
- **时间范围筛选**：当需要筛选某段时间范围（如早于某日期且晚于某日期/本月/本年）时，需要设置两个条件分别使用 `is_greater` 和 `is_less` 操作符，并使用 `and` 逻辑连接。
- **本月 / 本年等区间筛选的端点取值规则**：由于 `is_greater` 与 `is_less` 均为**严格大于 / 严格小于**（不含等号），筛选「本月」「本年」等闭区间时，端点必须分别取**目标区间第一天的前一天**与**目标区间最后一天的后一天**，从而保证目标区间内的所有日期都被包含。
   - 示例：筛选「本月」（以 5 月为例），应使用 `is_greater 2026-04-30` 且 `is_less 2026-06-01`；
   - 示例：筛选「本年」（以 2026 年为例），应使用 `is_greater 2025-12-31` 且 `is_less 2027-01-01`。
- **单选类型字段筛选**：当筛选的字段为单选类型时，`operator` 支持选择 `is` / `is_not` / `contains` / `does_not_contain` / `is_empty` / `is_not_empty`，其余均不支持

### statType 统计类型速查

下表为图表组件中 `statType` / `series.statType` 属性的可选值，多图表公用。

| 值 | 含义 | 适用字段类型 |
| --- | --- | --- |
| 8 | 求和 | 数字 |
| 9 | 平均值 | 数字 |
| 10 | 最大值 | 数字 |
| 11 | 最小值 | 数字 |

使用规则：
- statType 只能用于数字类型的字段，或公式输出为数字的字段。如果字段类型不是数字，使用 statType 可能会导致图表无法正常显示或统计结果不正确。

### seriesType 统计方式

下表为图表组件中 `seriesConfig.seriesType` 属性的可选值，多图表公用。

| 值 | 含义 |
| --- | --- |
| 0 | 未知 |
| 1 | 统计记录总数 |
| 2 | 列统计 |

使用规则：

- **当 `seriesType="1"`（统计记录总数 / 行数统计）时，`<seriesConfig>` 内部不需要填写 `<series>` 子标签**，图表会直接对当前数据表/筛选后的记录条数做统计。
- 当 `seriesType="2"`（列统计）时，必须在 `<seriesConfig>` 内填写 `<series>` 子标签，并通过 `series.fieldId` 与 `series.statType` 指定统计字段及统计方式（求和、平均值等）。
- 不显式填写 `seriesType` 时，按图表默认行为（一般等同于 `2` 列统计）处理。

### `<columnChart>` 柱状图

以纵向柱子呈现分类数值对比的图表。适用于在有限类别上进行量化对比，如各部门销售额、各产品销量。提供二级分组后可表达嵌套对比（堆积 / 百分比堆积）。

```markdown
<columnChart>
  <tableId>tbl001</tableId>
  <categoryFieldId>月份</categoryFieldId>
  <secondaryCategoryFieldId>类别</secondaryCategoryFieldId>
  <config title="标题" chartSubType="13">
    <seriesConfig seriesType="2">
      <series fieldId="金额" statType="8"></series>
    </seriesConfig>
  </config>
  <filterInfo type="custom" conjunction="and">
    <conditions>
      <condition fieldId="状态" operator="is" value="已完成"></condition>
    </conditions>
  </filterInfo>
</columnChart>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `<tableId>` | 是 | 关联的数据表标识，可填入数据表名称或数据表ID |
| `<categoryFieldId>` | 是 | 横轴分组字段名 |
| `<secondaryCategoryFieldId>` | 否 | 二级分组字段；使用时 `<series>` 只能有 1 个 |
| `config.title` | 否 | 图表标题 |
| `config.chartSubType` | 否 | 子类型，`13` 普通（默认） / `33` 堆积 / `34` 百分比堆积 |
| `seriesConfig.seriesType` | 否 | 统计方式，见 [seriesType 统计方式](#seriestype-统计方式)；为 `1`（行数统计）时内部 `<series>` 不填 |
| `series.fieldId` | 列统计必填 | 统计字段名称（仅 `seriesType="2"` 时填写） |
| `series.statType` | 列统计必填 | 统计类型，见 [statType 统计类型速查](#stattype-统计类型速查)（仅 `seriesType="2"` 时填写） |
| `<filterInfo>` | 否 | 筛选条件，详见 [<filterInfo>](#filterinfo-筛选条件) |

### `<barChart>` 条形图

条形图即横向柱状图，适用于分类名称较长、类别数量较多，或需要按数值排名展示的场景（如 TOP 客户、各项目耗时排行榜）。

```markdown
<barChart>
  <tableId>订单表</tableId>
  <categoryFieldId>地区</categoryFieldId>
  <config title="各地区销售额" chartSubType="29">
    <seriesConfig seriesType="2">
      <series fieldId="金额" statType="8"></series>
    </seriesConfig>
  </config>
  <filterInfo type="custom" conjunction="and">
    <conditions>
      <condition fieldId="状态" operator="is" value="已完成"></condition>
    </conditions>
  </filterInfo>
</barChart>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `<tableId>` | 是 | 关联的数据表标识，可填入数据表名称或数据表ID |
| `<categoryFieldId>` | 是 | 纵轴字段名 |
| `config.title` | 否 | 图表标题 |
| `config.chartSubType` | 否 | 子类型，`11` 普通（默认） / `29` 堆积 / `30` 百分比堆积 |
| 其余字段 | — | 同 [柱状图公用字段说明](#columnchart-柱状图)（`seriesConfig` / `series` / `<filterInfo>`） |


### `<lineChart>` 折线图

折线图以点连线的方式展示连续变化趋势，适用于观察指标随时间的趋势（月度销售走势、每日活跃用户变化等）。

```markdown
<lineChart>
  <tableId>销售表</tableId>
  <categoryFieldId>日期</categoryFieldId>
  <config title="销售额趋势" isSmooth="true">
    <seriesConfig seriesType="2">
      <series fieldId="金额" statType="8"></series>
    </seriesConfig>
  </config>
  <filterInfo type="custom" conjunction="and">
    <conditions>
      <condition fieldId="状态" operator="is" value="已完成"></condition>
    </conditions>
  </filterInfo>
</lineChart>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `<tableId>` | 是 | 关联的数据表标识，可填入数据表名称或数据表ID |
| `<categoryFieldId>` | 是 | 横轴字段，建议使用时间字段 |
| `config.title` | 否 | 图表标题 |
| `config.isSmooth` | 否 | 是否平滑曲线，可选 `true` / `false`，默认 `false` |
| 其余字段 | — | 同 [柱状图公用字段说明](#columnchart-柱状图)（`seriesConfig` / `series` / `<filterInfo>`） |


### `<pieChart>` 饼图 / 环图

以扇形区块展示各分类在总体中的占比，适用于展示构成比例（成本构成、不同渠道贡献占比等）。

```markdown
<pieChart>
  <tableId>销售表</tableId>
  <categoryFieldId>类别</categoryFieldId>
  <config title="各类别销售额分布" chartSubType="8">
    <seriesConfig seriesType="2">
      <series fieldId="金额" statType="8"></series>
    </seriesConfig>
  </config>
  <filterInfo type="custom" conjunction="and">
    <conditions>
      <condition fieldId="状态" operator="is" value="已完成"></condition>
    </conditions>
  </filterInfo>
</pieChart>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `<tableId>` | 是 | 关联的数据表标识，可填入数据表名称或数据表ID |
| `<categoryFieldId>` | 是 | 分组字段名称 |
| `config.title` | 否 | 图表标题 |
| `config.chartSubType` | 否 | 子类型，`8` 饼图（默认） / `10` 环图 |
| 其余字段 | — | 同 [柱状图公用字段说明](#columnchart-柱状图)（`seriesConfig` / `series` / `<filterInfo>`） |


### `<comboChart>` 组合图

可将每个系列渲染为柱状图或折线图，支持左右双轴，适用于数值范围差异较大的跨指标展示（如销售额 vs 增长率）。

```markdown
<comboChart>
  <tableId>tbl001</tableId>
  <categoryFieldId>fld_month</categoryFieldId>
  <config title="销售额与增长率">
    <seriesConfig seriesType="2">
      <series fieldId="fld_amount" statType="8" chartType="13" axisPosition="2"></series>
      <series fieldId="fld_growth_rate" statType="9" chartType="3" axisPosition="3"></series>
    </seriesConfig>
  </config>
  <filterInfo type="custom" conjunction="and">
    <conditions>
      <condition fieldId="fld_status" operator="is" value="option-string"></condition>
    </conditions>
  </filterInfo>
</comboChart>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `<tableId>` | 是 | 关联的数据表标识，可填入数据表名称或数据表ID |
| `<categoryFieldId>` | 是 | 横轴字段名 |
| `config.title` | 否 | 图表标题 |
| `seriesConfig.seriesType` | 否 | 统计方式，见 [seriesType 统计方式](#seriestype-统计方式)；组合图通常使用 `2`（列统计） |
| `series.fieldId` | 是 | 统计字段名 |
| `series.statType` | 是 | 统计类型，见 [statType 统计类型速查](#stattype-统计类型速查) |
| `series.chartType` | 是 | 系列图表类型，`13` 柱状图 / `3` 折线图 |
| `series.axisPosition` | 否 | 所在坐标轴，`2` 左轴（默认） / `3` 右轴 |
| `<filterInfo>` | 否 | 筛选条件，详见 [<filterInfo>](#filterinfo-筛选条件) |

- 至少提供 2 个 `<series>` 才能体现组合效果
- 组合图依赖具体字段的不同统计方式做对比，因此一般不使用 `seriesType="1"` 行数统计模式

### `<statisticsChart>` 指标卡

单个统计数值的大字号展示。适用于看板顶部突出关键指标，如“本月订单总数”、“当前在线人数”、“全年销售总额”。

```markdown
<statisticsChart>
  <tableId>员工表</tableId>
  <statisticsFieldId>金额</statisticsFieldId>
  <config title="总销售额" statType="8">
  </config>
  <filterInfo type="custom" conjunction="and">
    <conditions>
      <condition fieldId="fld_amount" operator="is_greater" valueScBlockId="input_1"></condition>
    </conditions>
  </filterInfo>
</statisticsChart>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `<tableId>` | 是 | 关联的数据表标识，可填入数据表名称或数据表ID |
| `<statisticsFieldId>` | 否 | 统计字段名称，不填则统计记录总数 |
| `config.title` | 否 | 图表标题 |
| `config.statType` | 否 | 统计类型，见 [statType 统计类型速查](#stattype-统计类型速查)；不填时为记录计数模式 |
| `<filterInfo>` | 否 | 筛选条件，详见 [<filterInfo>](#filterinfo-筛选条件) |

### `<wordCloudChart>` 词云图

按词频大小展示文本中的高频词汇。适用于快速识别评论、反馈、资讯标题等文本字段中的热点词汇。

```markdown
<wordCloudChart>
  <tableId>tbl001</tableId>
  <keywordFieldId>fld_comments</keywordFieldId>
  <config title="评论关键词" wordCount="50" hideCommonWords="false">
  </config>
  <filterInfo type="custom" conjunction="and">
    <conditions>
      <condition fieldId="fld_priority" operator="is" value="highOptionId"></condition>
    </conditions>
  </filterInfo>
</wordCloudChart>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `<tableId>` | 是 | 关联的数据表标识，可填入数据表名称或数据表ID |
| `<keywordFieldId>` | 是 | 关键字字段，仅支持文本类型 |
| `config.title` | 否 | 图表标题 |
| `config.wordCount` | 否 | 最大显示词数 |
| `config.hideCommonWords` | 否 | 是否过滤常用词，可选 `true` / `false` |
| `<filterInfo>` | 否 | 筛选条件，详见 [<filterInfo>](#filterinfo-筛选条件) |

使用规则：

- `<keywordFieldId>` 仅支持文本类型字段

## `<smartsheetView>` 智能表格视图

将关联智能表格的数据以表格视图的形式直接嵌入到智能文档中，可叠加筛选条件。适用于在文档中直接展示某张子表的明细数据，并配合上方的输入控件做联动筛选。

```markdown
<smartsheetView tableId="数据表ID" title="视图标题">
  <filterInfo type="custom" conjunction="and">
    <conditions>
      <!-- 动态筛选：引用上方 input_1 控件的输入值 -->
      <condition fieldId="name-field-id" operator="contains" valueScBlockId="input_1"></condition>
    </conditions>
  </filterInfo>
</smartsheetView>
```

属性表：

| 属性 / 子标签 | 必填 | 说明 |
| --- | --- | --- |
| `tableId` | 是 | 数据表ID（注意：本组件以**属性**而非子标签出现） |
| `title` | 否 | 视图标题 |
| `<filterInfo>` | 否 | 筛选条件，格式与图表组件完全一致，详见 [<filterInfo>](#filterinfo-筛选条件) |

使用规则：

- 标签名为驼峰命名法 `smartsheetView`，属性名也是驼峰式，不要写作 `smartsheet_view`
- 推荐通过 `valueScBlockId` 实现与上方控件的动态联动筛选

## `<linkcard>` 链接卡片

外链卡片组件，将一个链接以带标题、描述、缩略图、图标的卡片形式展示。适用于推荐外部资源、引用站外资料等场景。

```markdown
<linkcard linkUrl="https://docs.qq.com" linkName="链接标题" linkDescription="描述文字，默认为链接地址" linkThumbnail="缩略图URL" linkIcon="图标URL">
</linkcard>
```

属性表：

| 属性 | 必填 | 说明 |
| --- | --- | --- |
| `linkUrl` | 是 | 链接地址 |
| `linkName` | 是 | 链接标题 |
| `linkDescription` | 否 | 描述文字，未填时默认显示链接地址 |
| `linkThumbnail` | 否 | 缩略图 URL，未填时使用默认缩略图 |
| `linkIcon` | 否 | 图标 URL，未填时使用默认 icon |

## `<flowChart>` 流程图（只读组件）

智能文档中的流程图组件。**只读，不可通过 MDX 创建或修改，改写页面时必须原样保留。**

```markdown
<flowChart hinaId="..." width="..." height="..." />
```


## 普通表格

普通表格支持两种写法：Markdown 风格的表格适合常规数据展示，HTML 风格的表格支持合并单元格、对齐方式与背景颜色等复杂样式。

### Markdown 风格表格

适用于表头简单、无合并单元格的常规表格场景：

```markdown
| 序号 | 姓名 | 部门 | 状态 |
| --- | --- | --- | --- |
| 1 | 张三 | 研发部 | 进行中 |
| 2 | 李四 | 产品部 | 已完成 |
| 3 | 王五 | 设计部 | 待开始 |
```

### HTML 风格表格

当需要合并单元格、设置列宽、添加背景色等复杂样式时，使用 HTML 表格语法：

> **提示**：当智能文档返回带有复杂样式（`width`、`colspan`、`rowspan` 等）的 HTML 表格时，请在修改时保持相同的 HTML 格式，以确保样式信息不被丢失。

```markdown
<table>
  <colgroup><col span="2" width="120"/></colgroup>
  <thead><tr><th background-color="light_grey_background">表头</th><th background-color="light_grey_background">表头</th></tr></thead>
  <tbody><tr><td>单元格</td><td>单元格</td></tr></tbody>
</table>
```

支持的能力：

- 合并单元格（`colspan` / `rowspan`）
- 对齐（`align="left|center|right"`）
- 背景颜色（`background-color`）

## 转义规则

MDX 把 `<`、`>`、`{`、`}` 视为 JSX 语法符号，正文中出现时需转义：

| 原文字符 | 转义写法 |
| --- | --- |
| `<` | `&lt;` |
| `>` | `&gt;` |
| `{` | `&#123;` |
| `}` | `&#125;` |
| `~` | `\~` |

正文中的 `<`、`>`、`{`、`}` 按上表转义并以正文形式呈现，不要用代码块包裹来规避转义。

### 不需要转义的场景

- **MDX 标签属性值内**（如 `<span style="color: grey">`）：属性值里的 `<` `>` 已在引号内，不需要额外转义
- **代码围栏（` ``` … ``` `）内**：代码块内容原样保留，渲染器不解析 JSX，无需转义；
- **行内代码（`` `…` ``）内**：同上，原样保留
- **Markdown 链接 URL 部分**（如 `[文字](https://…)`）：URL 里的 `&` 等字符保持原样，不转义
- **删除线**：`~` 是删除线时无需转义
