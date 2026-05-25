# H3C 蓝版设计 Token 全集

> 本文是 H3C 集团对外 PPT(深邃蓝主版)的视觉基线。脚本 `audit_visual.py` 默认以这套 token 为准做规范扫描;若项目 base.css 自定义了 `--fs-*` 字号 token,脚本会优先采用项目实际值。

## 1. 配色 Token

### 1.1 主底层(三阶深邃蓝)

| Token | HEX | 用途 |
|---|---|---|
| `--bg-1` | `#000B1A` | 最深底,主背景 |
| `--bg-2` | `#021B33` | 卡片底 |
| `--bg-3` | `#0A2B47` | 提升层 / 悬浮卡 |
| `--bg-line` | `#06304D` | 描边 / 分割线 |

### 1.2 品牌锚点

| Token | HEX | 用途 |
|---|---|---|
| `--gx-red` | `#00AEEF` | H3C 蓝(主强调,token 名沿袭旧版,值为蓝) |
| `--gx-red-deep` | `#0099D4` | 深 H3C 蓝(浅文场景) |
| `--gold` | `#00F2FF` | 极光绿(主) |
| `--gold-bright` | `#7BFFFF` | 高光极光绿(标题/高亮) |
| `--gold-deep` | `#00A8B5` | 深极光(描边/章) |

### 1.3 文字色阶

| Token | HEX | 用途 |
|---|---|---|
| `--t-1` | `#F5F1EA` | 暖白主文 |
| `--t-2` | `#C9CDD3` | 副文 |
| `--t-3` | `#8B919A` | 辅助/注脚 |
| `--t-4` | `#555B66` | 极弱/水印 |

### 1.4 数据可视化

- `--data-green` `#4ADE80` 正面/达成
- `--data-orange` `#FBBF24` 风险/告警
- `--data-gray` `#8B919A` 中性/对照

### 1.5 配色铁律

- 整套 PPT **不超过 8 个色**
- **禁用色相**:任何霓虹色、紫色 `#9400D3 / #8A2BE2`、品红 `#FF00FF / #C71585 / #EE82EE`、青色高饱和
- 数字图表:正/负/中 三色制,**饼图禁用**(改环形或堆叠条)

## 2. 字号阶梯(严格 6 档 + KPI)

> 死线:**任何文字 ≥ 18px / 任何文字 ≤ 200px(KPI 例外)** · 字号档位 6 选 1,严禁混搭 14/16/19/20/24 等

| Token | px | 用途 | class 示例 |
|---|---|---|---|
| `--fs-cover` | **104** | 封面主标 | `.t-cover-main` |
| `--fs-section` | **80** | 章节扉页主标 | `.t-section` |
| `--fs-h1` | **52** | 内页主标 · action title | `.t-page-main`, `.page-title-main` |
| `--fs-h2` | **36** | 二级标题 / 区块标题 | `.t-subtitle` |
| `--fs-body` | **22** | 正文 / 卡片标题 | `.t-body` |
| `--fs-note` | **18** | 注脚 / 来源 / 标签 / 死线最小 | `.t-note`, `.page-eyebrow` |
| `--fs-kpi` | **144** | KPI 主数字 | `.t-kpi`, `.t-kpi-red` |
| `--fs-kpi-sm` | **96** | KPI 中等数字 | `.t-kpi-medium` |

### 2.1 排版铁律

- **一页最多 3 个内容字号档位**(KPI/封面/章节锚不算)
- **行长 ≤ 28 字**(中文),≥ 28 字必换行
- **数字必用 Inter**(不让数字跟着中文衬线变形)
- 强调**不用斜体**(中文斜体反美感),用粗体或色块
- **不下划线**(下划线 = web 链接,PPT 上廉价)

## 3. 字体栈

### 3.1 中文正文(默认)

```css
font-family: "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", system-ui, sans-serif;
```

### 3.2 数字 / 英文 / 代码

```css
font-family: "Inter", "SF Pro Display", "Helvetica Neue", -apple-system, sans-serif;
```

### 3.3 衬线(限定)

```css
/* 仅允许在以下 selector 内出现 */
.slide .quote,
.slide [class*="quote"],
.slide [class*="-cite"] {
  font-family: "Source Han Serif SC", "Noto Serif CJK SC", "Songti SC", serif;
}
```

### 3.4 monospace(代码块限定)

```css
.code-body {
  font-family: "JetBrains Mono", "SF Mono", Consolas, monospace;
}
```

### 3.5 字体黑名单(0 容忍)

`宋体 / SimSun / FangSong / 仿宋 / 楷体 / KaiTi / Songti` — **整套 PPT 任何位置都不允许出现**(含 inline `style="font-family:..."` 和 CSS class 内)。审计脚本会精确匹配关键词。

## 4. 间距 Token

| Token | px | 何时用 |
|---|---|---|
| `--gap-xs` | 8 | 标签内、icon 与文字 |
| `--gap-sm` | 16 | 同组元素内部 |
| `--gap-md` | 24 | 卡片内分块 |
| `--gap-lg` | 48 | 主区块之间 |
| `--gap-xl` | 96 | 章节切换、留白主导页 |

**留白原则**:每页留白 ≥ 30%。

## 5. 16:9 网格(1920×1080 设计基)

- 外边距 96px(左右)/ 72px(上下)
- 内容区 1728 × 936
- 12 列 × 8 行,列间距 24px,行间距 24px
- 基础间距单位 = 8px(所有间距 = 8 的倍数)

## 6. 组件位置铁律

### 6.1 右下角页码 `.page-pagenum / .slide-pagenum`

```css
position: absolute;
bottom: 28px !important;
right: 48px !important;
font-family: "Inter", "SF Pro Display", -apple-system, sans-serif;
font-size: var(--fs-note);  /* 18px */
padding: 6px 14px;
background: rgba(0, 11, 26, 0.55);
border-radius: 12px;
backdrop-filter: blur(6px);
z-index: 95;
```

### 6.2 全本 `.page-footer` 必须给页码让位

```css
.slide .page-footer {
  right: 240px !important;   /* 给右下角 56-220 的页码区让位 */
}
```

### 6.3 右上角 H3C logo(整本恒挂,庄重)

```css
.slide::after {
  content: "";
  position: absolute;
  top: 36px; right: 56px;
  width: 144px; height: 64px;
  background-image: url("h3c-brand/h3c-logo-white.png");
  background-size: contain;
  background-position: right center;
  opacity: 1.0;
  z-index: 100;
}
```

封面 / 章节扉页 (`.slide.cover / .slide.sec`) 不显示底部水印。

## 7. 反模式(踩一条全部重做)

1. ❌ 一页堆 5 个以上要点 → 拆页
2. ❌ 图表 + 大段文字同框 → 一页一件事
3. ❌ 动图 / GIF / 视频自动播放 → 全部静态(演示时点击触发)
4. ❌ 使用 emoji 作正式装饰(✅❌✨🚀)→ 用色块、icon、章
5. ❌ PPT 默认模板配色(深蓝渐变、浅灰背景方框)→ 用本规范配色
6. ❌ Times New Roman / 宋体 / 仿宋(老气)→ 用 PingFang / Inter
7. ❌ 下划线、双下划线、阴影文字、文字描边 → 用色块强调
8. ❌ 3D 图表、立体柱状、爆炸饼图 → 平面、克制
9. ❌ 每页放公司 logo + 标语 + 水印(廉价)→ 页眉一条 + 右上 logo 即可
10. ❌ "展望未来""携手共进""砥砺前行"等套话 → 用具体动词 + 具体数字
11. ❌ 斜体、楷体作正文 → 楷体仅限引文块
12. ❌ 居中正文段落 → 正文左对齐,标题可居中
