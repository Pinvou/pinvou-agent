# h3c-ppt · templates/ · 母板使用指南

这里是 H3C 对外 PPT 的可复用 HTML 母板。新项目按下面 3 步用,30 秒完成"项目脚手架"。

## 文件清单

```
templates/
├── README.md            ← 本文件
├── base.css             ← 蓝版完整 base.css(直接 cp 到项目 assets/)
├── h3c-brand/           ← 4 张品牌资产(直接 cp 到项目 assets/h3c-brand/)
│   ├── background-cover.jpg     封面背景(H3C 总部大楼)
│   ├── background-section.jpg   章节扉页(粒子波浪)
│   ├── background-page.jpg      内页背景(波浪线条)
│   └── h3c-logo-white.png       右上角白 logo
├── L01-cover.html       ← 封面母板(用 .slide.cover 触发大楼背景)
├── L02-section-anchor.html  ← 章节扉页(用 .slide.sec 触发粒子背景)
├── L03-title-body.html  ← 标题 + 正文(最常用 · 80% 内容页)
├── L04-bullet-three.html ← 三点要点
└── L05-kpi-hero.html    ← 大数字 KPI(三个数字)
```

## 3 步用起来

```bash
# 1. 拷贝资产到项目
SKILL=~/.claude/skills/h3c-ppt
PROJ=/path/to/your-project/HTML_Deck

mkdir -p $PROJ/{slides,assets,_tools,_audit}
cp -r $SKILL/templates/h3c-brand $PROJ/assets/
cp $SKILL/templates/base.css $PROJ/assets/

# 2. 拷贝某个母板做新 slide
cp $SKILL/templates/L01-cover.html  $PROJ/slides/00-cover.html
cp $SKILL/templates/L03-title-body.html  $PROJ/slides/P01-info-explosion.html
# ... 按需 cp

# 3. 改 slides/*.html 里的 <h1> / <p> 内容,保留 class 名不动
```

## 五个母板对应的页面类型

| 母板 | 用什么页 | 例子 |
|---|---|---|
| **L01 cover** | 整本第一页 / 章节大封面 | "项目主题 + 一句话定调" |
| **L02 section-anchor** | 章节切换 / 大段落分隔 | "Chapter 02 · XX 主题" |
| **L03 title-body** | 标准内容页(最常用) | "本月哪些品类滞销 · 答案是 X" |
| **L04 bullet-three** | 3 点结构 / 三方对比 | "三个购买理由 / 三大场景" |
| **L05 kpi-hero** | 数字论证 / 数据冲击 | "120 ZB / 28% 增速 / 1000 亿条" |

## 还需要扩展的母板(future)

当前 5 个母板覆盖 ~60% 的页面需求。常见但还没补的:

- L06 双栏对比(A vs B / 之前 vs 之后)
- L07 数据图(单图 + 标题 + source)
- L08 表格(中性灰线表)
- L09 故事场景(头像 + 一段话)
- L10 收束(1 句金句占满屏)

需要的时候按 L01-L05 的写法新增即可。

## 视觉规范基线

所有母板严格遵循 `../reference/design_tokens.md`:
- 字号:严格 6 档(18 / 22 / 36 / 52 / 80 / 144 / 96)
- 字体:中文 PingFang / 数字 Inter / 衬线只在 quote class 内
- 配色:深邃蓝 + H3C 蓝 + 极光绿(token 定义在 `base.css` :root)

改母板时不要改 token 名(--bg-1 / --gx-red 等),只换内容。token 是 audit 脚本的扫描基线。

## audit & build

母板写完的页面,直接进入 `h3c-ppt` skill 的 phase 10(audit)+ phase 12(build):

```bash
bash ~/.claude/skills/h3c-ppt/scripts/run_all.sh /path/to/your-project/HTML_Deck
```
