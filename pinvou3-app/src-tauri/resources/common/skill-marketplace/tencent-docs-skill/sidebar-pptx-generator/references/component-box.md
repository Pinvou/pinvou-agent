---
name: component-box
description: Box 容器组件规范 — Flex 布局、盒模型、常用样式属性、限制条款
---

# Box 组件规范

Box 是 slidep 中最基础的容器组件，承担所有**布局与分组**职责。支持 Flexbox 布局，可任意嵌套，是构建页面结构的基本单元。

## 默认布局

```jsx
<Box>...</Box>
// 等价于
<Box style={{ display: 'flex', flexDirection: 'column' }}>...</Box>
```

- 默认 `display: 'flex'`，`flexDirection: 'column'`（纵向流）
- 未指定 `width` / `height` 时，尺寸由子组件与父容器共同决定

## 盒模型

**遵循 `box-sizing: border-box`**：设置 `width` 时已包含 padding 和 border，内容可用空间 = `width - padding - border`。

例如：`<Box style={{ width: 400, padding: 20, border: '2px solid #333' }}>` 内部子组件可用宽度 = 400 − 20×2 − 2×2 = **356px**。

## 常用样式

| 类别 | 属性 |
| :--- | :--- |
| 布局 | `flexDirection` / `justifyContent` / `alignItems` / `gap` / `flex` / `flexWrap` |
| 尺寸 | `width` / `height` / `minWidth` / `minHeight` / `maxWidth` / `maxHeight` |
| 间距 | `padding` / `margin`（及其方向变体） |
| 视觉 | `background` / `border` / `borderRadius` / `boxShadow` / `opacity` |
| 定位 | `position: 'absolute' / 'relative'` / `top` / `left` / `right` / `bottom` / `zIndex` |

## 硬性限制（必须遵守）

1. **仅支持 `display: 'flex'`**。严禁 `display: 'grid'`、`display: 'table'` 以及任何 grid 相关属性（`gridTemplateColumns` 等）
2. **不支持 CSS `calc()` 函数**。需要动态尺寸时改用 `flex: 1` / 百分比 / 精确计算的固定值
3. 子组件类型可以是 `Text` / `FAIcon` / `Image` / `Chart` / `Table` / `Box`（嵌套）等任意 slidep 组件

## 编译偏差（写入 JSX 时必须补偿）

> slidep 将 JSX 编译为 OOXML，以下特性的实际渲染与 CSS 预期有偏差，写代码时必须主动补偿：

1. **`flex: 1` 编译后为估算固定值，实际宽度可能偏小 20–30%**。需要精确撑满时，用明确的 px 值代替 `flex: 1`。
2. **`gap` 编译后间距可能缩水**。关键间距改用 `marginLeft` / `marginTop` 手动设置。
3. **`width: '100%'` 不保证生效**。需要全宽时用明确 px 值。
4. **`justifyContent: 'center'` 在内容高度不足时导致上下双端空白（悬浮）**。内容页 B 区应使用 `justifyContent: 'flex-start'` 或 `'space-between'`；仅封面 / 结束页允许 center。

## 使用示例

### 横向排列（图标 + 文字组合）

```jsx
<Box style={{
    flexDirection: 'row',
    alignItems: 'center',
    gap: 20,
    padding: 20,
    background: '#f0f4f8',
    borderRadius: 8,
}}>
    <FAIcon name='info-circle' style={{ fill: '#3b82f6', width: 24, height: 24 }} />
    <Box>
        <Text style={{ fontSize: 18, fontWeight: 'bold' }}>标题</Text>
        <Text style={{ fontSize: 14, color: '#64748b' }}>描述文本</Text>
    </Box>
</Box>
```

### Flex 纵向均分（内容卡片组）

```jsx
<Box style={{
    display: 'flex',
    flexDirection: 'column',
    gap: 24,
    height: '100%',
    justifyContent: 'space-between',
}}>
    <Box style={{ flex: 1 }}>...卡片 1...</Box>
    <Box style={{ flex: 1 }}>...卡片 2...</Box>
    <Box style={{ flex: 1 }}>...卡片 3...</Box>
</Box>
```

### 绝对定位装饰元素

```jsx
<Box style={{ position: 'relative', width: '100%', height: '100%' }}>
    {/* 背景装饰 */}
    <Box style={{
        position: 'absolute',
        top: 0, right: 0,
        width: 200, height: 200,
        background: 'linear-gradient(135deg, #3b82f6 0%, transparent 100%)',
        opacity: 0.1,
    }} />
    {/* 主内容 */}
    <Box style={{ position: 'relative', zIndex: 1 }}>...</Box>
</Box>
```

## 最佳实践

- 页面内**每个视觉区块**用独立的容器 Box 包裹，必要时加背景（纯色或半透明）增强分区感
- 卡片组用 **Flex + `flex: 1`** 均分空间，而非固定高度堆叠
- 布局、留白与信息密度规范见 [DESIGN.md](DESIGN.md)，具体项目以根目录 `DESIGN.md` 为准
