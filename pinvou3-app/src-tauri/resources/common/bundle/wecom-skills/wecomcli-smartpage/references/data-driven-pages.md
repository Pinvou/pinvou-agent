# 数据驱动页面搭建指引

本文档汇总**依赖智能文档内置数据表**的页面搭建流程，覆盖两大场景：

- **系统/图表页面**：任务系统、数据看板、项目跟踪等，页面上的图表/视图需要绑定内置表字段。
- **表单页面**：数据录入、信息收集，提交按钮通过 `ADDRECORD` 公式把控件值写入数据表。

两类场景的**共性铁律**：**必须先让内置表的子表与字段就位，再追加引用它们的页面内容**。否则图表会渲染失败、按钮会因引用不存在的字段而无法落库。

---

## 场景一：搭建含数据源的系统/图表页面

**适用**：任务系统、数据看板、项目跟踪页等需要图表/视图绑定数据的页面。

**与「从零创建智能文档」路径 A/B 的区别**：页面引用了数据，必须先让内置表的字段/视图就位，再写引用这些字段的图表组件。

### 执行步骤

1. **确定目标文档**：
   - *新建文档*：走 [`SKILL.md`](../SKILL.md) 「路径 B：先创建空白再追加内容」先建空文档，记录 `docid`。智能文档已自动绑定内置数据源，**勿另建独立智能表格**。
   - *已有文档新增图表页*：`smartpage pages update` 直接建页，无需重复创建文档。
2. **获取内置数据源**：`smartpage databases get` 拿到 `database_info.id` 与 `database_info.tables[].id`/`.name`，后续图表按子表 ID 绑定。
3. **配置数据表结构**：委托 `wecomcli-smartsheet` 完成子表创建、字段定义、数据初始化。
4. **写入页面内容**：字段就位后，用 `smartpage pages append` / `overwrite`（见 [`smartpage-edit.md`](smartpage-edit.md)）写入图表组件 MDX（见 [`mdx-syntax.md`](mdx-syntax.md)）。**切勿用 `smartpage import` / `create` 写内容**，否则会新建无数据表的文档。

---

## 场景二：创建表单页面（数据录入 / 信息收集）

**核心特征**：提交按钮通过 `ADDRECORD` 公式把控件值写入数据表，因此**必须先让目标子表与字段就位**，再追加包含控件和按钮的页面内容；否则按钮会因引用的字段不存在而无法落库。

### 执行步骤

1. **确定目标文档与页面**：
   - *新建文档*：`smartpage create` 创建空白智能文档，记录 `docid` 和默认首页。
   - *已有文档*：`smartpage pages update` 新建一个页面用于放置表单。
2. **获取内置数据表**：`smartpage databases get` 取 `database_info.id` 与 `database_info.tables[]`，后续配置字段和按钮公式的引用依据。
3. **委托 `wecomcli-smartsheet` 补子表与字段**：在上一步拿到的内置表上创建子表（如「报名表」）并定义字段。字段类型需与控件匹配：文本字段对应 `<input>`，单选/多选字段对应 `<select>`。
4. **重命名表单页面**：`smartpage pages update` 将目标页面改为有意义的名称（如「报名表单页」）——该名称将用于 `ADDRECORD` 公式中引用控件值。引用格式为 `[页面名.控件名]`，**必须与页面名完全一致**，**不得使用文档名称**；跳过此步将导致按钮因公式错误无法使用。
5. **追加表单页面内容**：`smartpage pages get` 拿到 `page_id` 后，`smartpage pages append` 将表单 MDX 追加到该页面。控件与按钮写法参考 [`mdx-syntax.md`](mdx-syntax.md) 中 `<input>` / `<select>` / `<button>` 章节，`formulaString` 中 `ADDRECORD` 的写法参考 [`formula/pageblock.md`](formula/pageblock.md)。

---

## 通用约束

- **数据源来源唯一**：智能文档创建后自带内置数据源，通过 `smartpage databases get` 获取，不要委托 `wecomcli-smartsheet` 另建独立智能表格。
- **字段先行、内容后置**：无论图表还是表单按钮，只要 MDX 中引用了字段，就必须在写页面内容前完成字段定义。
- **控件与字段类型匹配**：表单场景下，`<input>` ↔ 文本字段、`<select>` ↔ 单选/多选字段；错配会导致落库失败。
- **公式引用格式**：`ADDRECORD` 公式中的引用为 `[页面名.控件名]`，页面名必须与 `smartpage pages update` 后的实际名称完全一致。
