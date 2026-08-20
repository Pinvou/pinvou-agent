---
name: component-svg
description: SVG component - inline vector graphics, viewBox, path, shapes
---

# SVG 组件规范

SVG 组件允许直接使用 `<svg>` 标签渲染矢量图形，适合自定义图标、装饰形状或复杂的矢量图。

## 核心属性

- **width / height**: 必须指定。
- **viewBox**: 定义画布的坐标系。
- **children**: 内联 SVG 元素（如 `<rect>`, `<circle>`, `<path>`, `<g>` 等）。

## 布局与样式

- **默认布局**: `display: 'block'`。
- **支持属性**: `fill`, `stroke`, `strokeWidth`, `opacity` 等标准 SVG 属性。
- **注意事项**:
    - 建议使用 `viewBox` 以确保图形在不同尺寸下正确缩放。

## 示例代码

### 1. 基础用法 (简单形状)

```jsx
<svg width={100} height={100} viewBox='0 0 100 100'>
    <circle cx='50' cy='50' r='40' fill='#3b82f6' />
</svg>
```

### 2. 综合示例 (复杂路径)

```jsx
<svg width={24} height={24} viewBox='0 0 24 24'>
    <path d='M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5' fill='none' stroke='#64748b' strokeWidth='2' />
</svg>
```

## 最佳实践

- **保持简洁**: 优先使用 FAIcon，仅在需要特殊自定义图形时使用 SVG。
- **坐标对齐**: 确保 `viewBox` 与 `width/height` 比例一致，避免图形变形。
