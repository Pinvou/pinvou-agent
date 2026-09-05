#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
pinvou_pptx —— spec 驱动的本地 PPT 渲染器（Phase A：10 版式 + 3 主题）

用法：
    python pinvou_pptx.py <spec.json> <out.pptx>

版式：cover / agenda / section / bullets / two_col / kpi / chart / table / image / end
主题：business-blue / tech-dark / gov-red
字体：Noto Sans SC（思源黑体同源，跨平台 + 本机可预览；Phase C 做字体嵌入彻底锁死）
原则：模型只填 layout + 内容；坐标/配色/字体由模板定死 → 稳、不会摆崩；单页失败兜底不崩。
"""
import json
import os
import sys

from pptx import Presentation
from pptx.util import Inches, Pt
from pptx.dml.color import RGBColor
from pptx.enum.text import PP_ALIGN, MSO_ANCHOR
from pptx.enum.shapes import MSO_SHAPE
from pptx.chart.data import CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE, XL_LEGEND_POSITION

SLIDE_W = Inches(13.333)
SLIDE_H = Inches(7.5)
FONT = "Noto Sans SC"  # 思源黑体同源；跨平台

THEMES = {
    "business-blue": {
        "bg": "FFFFFF", "primary": "0B57D0", "primary_dark": "0842A0", "accent": "4A90D9",
        "ink": "1F2A44", "muted": "6B7280", "light": "EAF1FB", "on_primary": "FFFFFF",
        "title_font": FONT, "body_font": FONT,
    },
    "tech-dark": {
        "bg": "0E1116", "primary": "3DDC97", "primary_dark": "1FB67A", "accent": "5AC8FA",
        "ink": "E6EDF3", "muted": "8B98A5", "light": "1B2330", "on_primary": "0E1116",
        "title_font": FONT, "body_font": FONT,
    },
    "gov-red": {
        "bg": "FFFFFF", "primary": "C0282D", "primary_dark": "8E1C20", "accent": "D4AF37",
        "ink": "2B2B2B", "muted": "6B6B6B", "light": "F7EAEA", "on_primary": "FFFFFF",
        "title_font": FONT, "body_font": FONT,
    },
    # ── 新增主题 ──
    "creative-purple": {  # 创意 / 品牌 / 互联网发布会
        "bg": "FFFFFF", "primary": "6D28D9", "primary_dark": "581C9E", "accent": "A78BFA",
        "ink": "2A2342", "muted": "6B7280", "light": "F4EFFC", "on_primary": "FFFFFF",
        "title_font": FONT, "body_font": FONT,
    },
    "fresh-green": {  # 教育 / 环保 / 医疗健康
        "bg": "FFFFFF", "primary": "13795B", "primary_dark": "0E5C45", "accent": "34D399",
        "ink": "1E2B27", "muted": "6B7280", "light": "E7F6F0", "on_primary": "FFFFFF",
        "title_font": FONT, "body_font": FONT,
    },
    "warm-orange": {  # 营销 / 消费 / 活动促销
        "bg": "FFFFFF", "primary": "DA5B1B", "primary_dark": "AE4814", "accent": "F5A623",
        "ink": "33261C", "muted": "6B7280", "light": "FDF0E6", "on_primary": "FFFFFF",
        "title_font": FONT, "body_font": FONT,
    },
    "navy-gold": {  # 金融 / 高端商务 / 年报
        "bg": "FFFFFF", "primary": "1B2A4A", "primary_dark": "0F1B33", "accent": "C8A04A",
        "ink": "1B2333", "muted": "6B7280", "light": "EDF0F6", "on_primary": "FFFFFF",
        "title_font": FONT, "body_font": FONT,
    },
    "minimal-mono": {  # 极简 / 咨询 / 设计感
        "bg": "FFFFFF", "primary": "1A1A1A", "primary_dark": "000000", "accent": "5B6470",
        "ink": "1A1A1A", "muted": "8A8F98", "light": "F3F4F6", "on_primary": "FFFFFF",
        "title_font": FONT, "body_font": FONT,
    },
    "midnight": {  # 深色商务(藏青底,区别于 tech-dark 的绿)
        "bg": "0F1A2E", "primary": "6AA6E0", "primary_dark": "4886C8", "accent": "FFC857",
        "ink": "E8EEF6", "muted": "93A1B3", "light": "1B2A44", "on_primary": "0F1A2E",
        "title_font": FONT, "body_font": FONT,
    },
}
DEFAULT_THEME = "business-blue"

# 模型常传中文主题名或近似词(THEMES 的 key 是英文)→ 归一到合法 key。
THEME_ALIASES = {
    "商务蓝": "business-blue", "商务": "business-blue", "蓝色": "business-blue", "蓝": "business-blue", "blue": "business-blue",
    "科技深色": "tech-dark", "科技黑": "tech-dark", "科技": "tech-dark", "极客": "tech-dark", "tech": "tech-dark", "dark": "tech-dark",
    "政务红": "gov-red", "政务": "gov-red", "党政": "gov-red", "红色": "gov-red", "红": "gov-red", "gov": "gov-red", "red": "gov-red",
    "创意紫": "creative-purple", "创意": "creative-purple", "品牌": "creative-purple", "发布会": "creative-purple", "紫色": "creative-purple", "紫": "creative-purple", "purple": "creative-purple", "creative": "creative-purple",
    "清新绿": "fresh-green", "清新": "fresh-green", "教育": "fresh-green", "环保": "fresh-green", "医疗": "fresh-green", "健康": "fresh-green", "绿色": "fresh-green", "绿": "fresh-green", "green": "fresh-green", "fresh": "fresh-green",
    "暖橙": "warm-orange", "营销": "warm-orange", "活动": "warm-orange", "促销": "warm-orange", "消费": "warm-orange", "橙色": "warm-orange", "橙": "warm-orange", "orange": "warm-orange", "warm": "warm-orange",
    "藏青金": "navy-gold", "金融": "navy-gold", "年报": "navy-gold", "高端": "navy-gold", "藏青": "navy-gold", "navy": "navy-gold", "gold": "navy-gold",
    "极简黑白": "minimal-mono", "极简": "minimal-mono", "黑白": "minimal-mono", "咨询": "minimal-mono", "设计感": "minimal-mono", "minimal": "minimal-mono", "mono": "minimal-mono",
    "午夜蓝": "midnight", "午夜": "midnight", "深色商务": "midnight", "深蓝": "midnight", "midnight": "midnight",
}


def _resolve_theme(name):
    """把模型传来的 theme(可能是中文/近似词/英文)归一到 THEMES 的合法 key;认不出回默认。"""
    if not name:
        return DEFAULT_THEME
    key = str(name).strip()
    if key in THEMES:
        return key
    low = key.lower()
    if low in THEMES:
        return low
    if key in THEME_ALIASES:
        return THEME_ALIASES[key]
    if low in THEME_ALIASES:
        return THEME_ALIASES[low]
    # 兜底:模型传"商务蓝色风格"之类长串 —— 用包含匹配,长别名优先(更具体)。
    for alias in sorted(THEME_ALIASES, key=len, reverse=True):
        if alias in key or alias in low:
            return THEME_ALIASES[alias]
    return DEFAULT_THEME


def C(h):
    return RGBColor.from_string(h)


# ── 基础 helper ─────────────────────────────────────────────────────────────

def rect(slide, x, y, w, h, fill_hex, line_hex=None, line_w=1.0):
    sp = slide.shapes.add_shape(MSO_SHAPE.RECTANGLE, x, y, w, h)
    sp.fill.solid()
    sp.fill.fore_color.rgb = C(fill_hex)
    if line_hex:
        sp.line.color.rgb = C(line_hex)
        sp.line.width = Pt(line_w)
    else:
        sp.line.fill.background()
    sp.shadow.inherit = False
    return sp


def textbox(slide, x, y, w, h, anchor=MSO_ANCHOR.TOP):
    tb = slide.shapes.add_textbox(x, y, w, h)
    tf = tb.text_frame
    tf.word_wrap = True
    tf.vertical_anchor = anchor
    tf.margin_left = 0
    tf.margin_right = 0
    tf.margin_top = 0
    tf.margin_bottom = 0
    return tf


def para(tf, text, size, color_hex, bold=False, align=PP_ALIGN.LEFT,
         font=FONT, space_after=6, line_spacing=1.1, first=False):
    p = tf.paragraphs[0] if first else tf.add_paragraph()
    p.alignment = align
    p.space_after = Pt(space_after)
    p.line_spacing = line_spacing
    r = p.add_run()
    r.text = text
    r.font.size = Pt(size)
    r.font.bold = bold
    r.font.color.rgb = C(color_hex)
    r.font.name = font
    return p


def bg(slide, hex_color):
    rect(slide, 0, 0, SLIDE_W, SLIDE_H, hex_color)


def _title_block(slide, th, title):
    rect(slide, Inches(0.9), Inches(0.85), Inches(0.16), Inches(0.5), th["primary"])
    tf = textbox(slide, Inches(1.2), Inches(0.8), Inches(11.2), Inches(0.8))
    para(tf, title or "", 28, th["ink"], bold=True, font=th["title_font"], first=True)
    rect(slide, Inches(0.9), Inches(1.55), Inches(11.5), Inches(0.025), th["light"])


# ── 版式 ────────────────────────────────────────────────────────────────────

def layout_cover(slide, th, s):
    bg(slide, th["bg"])
    rect(slide, 0, 0, Inches(0.28), SLIDE_H, th["primary"])
    tf = textbox(slide, Inches(1.1), Inches(2.5), Inches(11.0), Inches(2.6), MSO_ANCHOR.MIDDLE)
    para(tf, s.get("title", "未命名演示"), 46, th["ink"], bold=True, font=th["title_font"], first=True, space_after=14)
    if s.get("subtitle"):
        para(tf, s["subtitle"], 22, th["muted"], font=th["body_font"])
    rect(slide, Inches(1.13), Inches(5.25), Inches(1.6), Inches(0.08), th["primary"])
    if s.get("date"):
        tf2 = textbox(slide, Inches(1.1), Inches(6.7), Inches(11), Inches(0.5))
        para(tf2, s["date"], 14, th["muted"], font=th["body_font"], first=True)


def layout_agenda(slide, th, s):
    bg(slide, th["bg"])
    _title_block(slide, th, s.get("title") or "目录")
    items = (s.get("items") or [])[:6]
    y0 = Inches(2.05)
    step = Inches(0.78)
    for i, it in enumerate(items):
        yy = y0 + i * step
        tfn = textbox(slide, Inches(1.2), yy, Inches(1.1), step, MSO_ANCHOR.MIDDLE)
        para(tfn, "%02d" % (i + 1), 30, th["primary"], bold=True, font=th["title_font"], first=True)
        rect(slide, Inches(2.25), yy + Inches(0.12), Inches(0.04), step - Inches(0.28), th["light"])
        tft = textbox(slide, Inches(2.55), yy, Inches(9.3), step, MSO_ANCHOR.MIDDLE)
        para(tft, str(it), 20, th["ink"], font=th["body_font"], first=True)


def layout_section(slide, th, s):
    bg(slide, th["primary"])
    rect(slide, Inches(1.1), Inches(3.05), Inches(0.9), Inches(0.1), th["accent"])
    tf = textbox(slide, Inches(1.1), Inches(3.3), Inches(11), Inches(1.5))
    para(tf, s.get("title", ""), 40, th["on_primary"], bold=True, font=th["title_font"], first=True)


def layout_bullets(slide, th, s):
    bg(slide, th["bg"])
    _title_block(slide, th, s.get("title"))
    tf = textbox(slide, Inches(1.2), Inches(2.0), Inches(11.0), Inches(4.8))
    for i, b in enumerate(s.get("bullets") or []):
        para(tf, "▸  " + str(b), 19, th["ink"], font=th["body_font"], space_after=14, line_spacing=1.2, first=(i == 0))


def layout_two_col(slide, th, s):
    bg(slide, th["bg"])
    _title_block(slide, th, s.get("title"))

    def col(x, ctitle, items):
        if ctitle:
            tf = textbox(slide, x, Inches(2.0), Inches(5.4), Inches(0.6))
            para(tf, ctitle, 18, th["primary"], bold=True, font=th["title_font"], first=True)
        tf2 = textbox(slide, x, Inches(2.7), Inches(5.4), Inches(4.0))
        for i, b in enumerate(items or []):
            para(tf2, "•  " + str(b), 17, th["ink"], font=th["body_font"], space_after=12, line_spacing=1.2, first=(i == 0))

    col(Inches(1.2), s.get("leftTitle"), s.get("left"))
    col(Inches(6.9), s.get("rightTitle"), s.get("right"))


def layout_kpi(slide, th, s):
    bg(slide, th["bg"])
    _title_block(slide, th, s.get("title"))
    items = (s.get("items") or [])[:4]
    n = max(1, len(items))
    gap = Inches(0.4)
    total_w = Inches(11.5)
    card_w = int((total_w - gap * (n - 1)) / n)
    x0, y, h = Inches(0.9), Inches(2.6), Inches(2.4)
    for i, it in enumerate(items):
        x = x0 + i * (card_w + gap)
        rect(slide, x, y, card_w, h, th["light"])
        rect(slide, x, y, card_w, Inches(0.12), th["primary"])
        tfn = textbox(slide, x, y + Inches(0.55), card_w, Inches(1.1), MSO_ANCHOR.MIDDLE)
        para(tfn, str(it.get("num", "")), 40, th["primary"], bold=True, align=PP_ALIGN.CENTER, font=th["title_font"], first=True)
        tfl = textbox(slide, x, y + Inches(1.65), card_w, Inches(0.6), MSO_ANCHOR.MIDDLE)
        para(tfl, str(it.get("label", "")), 15, th["muted"], align=PP_ALIGN.CENTER, font=th["body_font"], first=True)


_CHART_TYPES = {
    "bar": XL_CHART_TYPE.COLUMN_CLUSTERED,
    "line": XL_CHART_TYPE.LINE_MARKERS,
    "pie": XL_CHART_TYPE.PIE,
}


def layout_chart(slide, th, s):
    bg(slide, th["bg"])
    _title_block(slide, th, s.get("title"))
    ctype = _CHART_TYPES.get(s.get("chartType", "bar"), XL_CHART_TYPE.COLUMN_CLUSTERED)
    cd = CategoryChartData()
    cd.categories = s.get("categories") or []
    series = s.get("series") or []
    for ser in series:
        cd.add_series(ser.get("name", ""), tuple(ser.get("data") or []))
    gframe = slide.shapes.add_chart(ctype, Inches(1.0), Inches(1.95), Inches(11.3), Inches(4.85), cd)
    chart = gframe.chart
    chart.has_legend = (len(series) > 1) or (ctype == XL_CHART_TYPE.PIE)
    if chart.has_legend:
        chart.legend.position = XL_LEGEND_POSITION.BOTTOM
        chart.legend.include_in_layout = False
    palette = [th["primary"], th["accent"], th["primary_dark"], th["muted"]]
    try:
        if ctype == XL_CHART_TYPE.PIE:
            for i, pt in enumerate(chart.plots[0].series[0].points):
                pt.format.fill.solid()
                pt.format.fill.fore_color.rgb = C(palette[i % len(palette)])
        elif ctype == XL_CHART_TYPE.LINE_MARKERS:
            for i, ser in enumerate(chart.series):
                ser.format.line.color.rgb = C(palette[i % len(palette)])
                ser.format.line.width = Pt(2.25)
        else:
            for i, ser in enumerate(chart.series):
                ser.format.fill.solid()
                ser.format.fill.fore_color.rgb = C(palette[i % len(palette)])
    except Exception:
        pass
    try:
        chart.font.size = Pt(12)
        chart.font.name = th["body_font"]
        chart.font.color.rgb = C(th["ink"])
    except Exception:
        pass


def _cell(cell, fill_hex, text, size, color_hex, th, bold=False, align=PP_ALIGN.LEFT):
    cell.fill.solid()
    cell.fill.fore_color.rgb = C(fill_hex)
    cell.vertical_anchor = MSO_ANCHOR.MIDDLE
    cell.margin_left = Inches(0.14)
    cell.margin_right = Inches(0.14)
    cell.margin_top = Inches(0.04)
    cell.margin_bottom = Inches(0.04)
    tf = cell.text_frame
    tf.word_wrap = True
    p = tf.paragraphs[0]
    p.alignment = align
    r = p.add_run()
    r.text = text
    r.font.size = Pt(size)
    r.font.bold = bold
    r.font.color.rgb = C(color_hex)
    r.font.name = th["body_font"]


def layout_table(slide, th, s):
    bg(slide, th["bg"])
    _title_block(slide, th, s.get("title"))
    headers = s.get("headers") or []
    rows = s.get("rows") or []
    ncol = len(headers) if headers else (len(rows[0]) if rows else 0)
    nrow = len(rows) + (1 if headers else 0)
    if ncol == 0 or nrow == 0:
        return
    x, y, w = Inches(0.9), Inches(2.0), Inches(11.5)
    h = Inches(min(4.6, 0.62 * nrow))
    tbl = slide.shapes.add_table(nrow, ncol, x, y, w, h).table
    tbl.first_row = False
    tbl.horz_banding = False
    r0 = 0
    if headers:
        for j, htext in enumerate(headers):
            _cell(tbl.cell(0, j), th["primary"], str(htext), 14, th["on_primary"], th,
                  bold=True, align=PP_ALIGN.LEFT if j == 0 else PP_ALIGN.CENTER)
        r0 = 1
    for ri, row in enumerate(rows):
        zebra = th["light"] if ri % 2 == 0 else th["bg"]
        for j in range(ncol):
            val = str(row[j]) if j < len(row) else ""
            _cell(tbl.cell(ri + r0, j), zebra, val, 13, th["ink"], th,
                  align=PP_ALIGN.LEFT if j == 0 else PP_ALIGN.CENTER)


def _img_placeholder(slide, th, x, y, w, h, text):
    rect(slide, x, y, w, h, th["light"], line_hex=th["muted"])
    tf = textbox(slide, x, y, w, h, MSO_ANCHOR.MIDDLE)
    para(tf, text, 14, th["muted"], align=PP_ALIGN.CENTER, font=th["body_font"], first=True)


def layout_image(slide, th, s):
    bg(slide, th["bg"])
    _title_block(slide, th, s.get("title"))
    img = s.get("image")
    bullets = s.get("bullets") or []
    if bullets:
        tf = textbox(slide, Inches(0.9), Inches(2.2), Inches(5.0), Inches(4.4))
        for i, b in enumerate(bullets):
            para(tf, "•  " + str(b), 17, th["ink"], font=th["body_font"], space_after=12, line_spacing=1.25, first=(i == 0))
        ix, iy, iw, ih = Inches(6.3), Inches(2.0), Inches(6.1), Inches(4.3)
    else:
        ix, iy, iw, ih = Inches(2.6), Inches(2.0), Inches(8.1), Inches(4.3)
    if img and os.path.isfile(img):
        try:
            slide.shapes.add_picture(img, ix, iy, width=iw)
        except Exception as e:
            _img_placeholder(slide, th, ix, iy, iw, ih, "图片加载失败: %s" % e)
    else:
        _img_placeholder(slide, th, ix, iy, iw, ih, "（图片缺失）")
    if s.get("caption"):
        tfc = textbox(slide, ix, iy + ih + Inches(0.12), iw, Inches(0.4))
        para(tfc, str(s["caption"]), 12, th["muted"], align=PP_ALIGN.CENTER, font=th["body_font"], first=True)


def layout_end(slide, th, s):
    bg(slide, th["primary"])
    tf = textbox(slide, Inches(1), Inches(3.0), Inches(11.3), Inches(1.6), MSO_ANCHOR.MIDDLE)
    para(tf, s.get("title", "谢谢"), 48, th["on_primary"], bold=True, align=PP_ALIGN.CENTER, font=th["title_font"], first=True)
    if s.get("subtitle"):
        para(tf, s["subtitle"], 18, th["light"], align=PP_ALIGN.CENTER, font=th["body_font"])


LAYOUTS = {
    "cover": layout_cover, "agenda": layout_agenda, "section": layout_section,
    "bullets": layout_bullets, "two_col": layout_two_col, "kpi": layout_kpi,
    "chart": layout_chart, "table": layout_table, "image": layout_image, "end": layout_end,
}


# ── 封面缩略图(方案 A):Pillow 画封面 → 注入 pptx 的 docProps/thumbnail.jpeg ──
def _load_cjk_font(size):
    """跨平台找一个能渲染中文的 TTF;找不到返回 None(宁可不画也别出方块)。"""
    try:
        from PIL import ImageFont
    except Exception:
        return None
    candidates = [
        "C:/Windows/Fonts/msyh.ttc", "C:/Windows/Fonts/msyhl.ttc", "C:/Windows/Fonts/simhei.ttf",  # Win
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",  # Debian/Ubuntu
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",        # Arch/其它
        "/System/Library/Fonts/PingFang.ttc",                       # mac(开发机)
    ]
    for p in candidates:
        if os.path.isfile(p):
            try:
                return ImageFont.truetype(p, size)
            except Exception:
                pass
    return None


def _cover_thumbnail(spec, th):
    """画一张 16:9 封面(bg + 色条 + 标题 + 副标题 + 角标),返回 JPEG bytes;失败/无字体返回 None。"""
    try:
        from PIL import Image, ImageDraw
        from io import BytesIO
    except Exception:
        return None
    tf = _load_cjk_font(76)
    if tf is None:
        return None  # 没中文字体 → 放弃缩略图,避免方块
    sf = _load_cjk_font(34) or tf
    ff = _load_cjk_font(22) or tf
    W, H, M = 1280, 720, 96
    img = Image.new("RGB", (W, H), "#" + th["bg"])
    d = ImageDraw.Draw(img)
    d.rectangle([M, 300, M + 150, 314], fill="#" + th["accent"])  # 色条
    # 标题优先取封面页(layout=cover)的,退回 spec 级,让缩略图跟真封面一致
    slides = spec.get("slides", [])
    cover = next((s for s in slides if s.get("layout") == "cover"), (slides[0] if slides else {}))
    title = spec.get("title") or cover.get("title") or "未命名演示"
    sub = spec.get("subtitle") or cover.get("subtitle") or ""
    d.text((M, 196), str(title)[:32], fill="#" + th["ink"], font=tf)
    if sub:
        d.text((M, 344), str(sub)[:48], fill="#" + th["muted"], font=sf)
    n = len(spec.get("slides", []))
    d.text((M, H - 84), "Pinvou · %d 页 · %s" % (n, spec.get("theme", DEFAULT_THEME)),
           fill="#" + th["muted"], font=ff)
    buf = BytesIO()
    img.save(buf, format="JPEG", quality=86)
    return buf.getvalue()


def _inject_thumbnail(pptx_path, jpeg_bytes):
    """把封面塞进 pptx 包:docProps/thumbnail.jpeg + .rels 关系 + Content_Types(让 PowerPoint/资源管理器也认)。"""
    if not jpeg_bytes:
        return
    import zipfile
    import shutil
    tmp = pptx_path + ".tmp"
    with zipfile.ZipFile(pptx_path, "r") as zin:
        ct = zin.read("[Content_Types].xml").decode("utf-8")
        rels = zin.read("_rels/.rels").decode("utf-8")
        if 'Extension="jpeg"' not in ct:
            ct = ct.replace("</Types>", '<Default Extension="jpeg" ContentType="image/jpeg"/></Types>')
        if "thumbnail.jpeg" not in rels:
            rel = ('<Relationship Id="rIdThumb" '
                   'Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail" '
                   'Target="docProps/thumbnail.jpeg"/>')
            rels = rels.replace("</Relationships>", rel + "</Relationships>")
        with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as zout:
            for n in zin.namelist():
                # 跳过这几个:Content_Types/.rels 改写后单独写;旧 thumbnail 用我们的替换(避免重复名)
                if n in ("[Content_Types].xml", "_rels/.rels", "docProps/thumbnail.jpeg"):
                    continue
                zout.writestr(n, zin.read(n))
            zout.writestr("[Content_Types].xml", ct)
            zout.writestr("_rels/.rels", rels)
            zout.writestr("docProps/thumbnail.jpeg", jpeg_bytes)
    shutil.move(tmp, pptx_path)


def render(spec, out_path):
    th = THEMES.get(spec.get("theme"), THEMES[DEFAULT_THEME])
    prs = Presentation()
    prs.slide_width = SLIDE_W
    prs.slide_height = SLIDE_H
    blank = prs.slide_layouts[6]
    for s in spec.get("slides", []):
        slide = prs.slides.add_slide(blank)
        fn = LAYOUTS.get(s.get("layout"), layout_bullets)
        try:
            fn(slide, th, s)
        except Exception as e:
            bg(slide, th["bg"])
            tf = textbox(slide, Inches(1), Inches(3), Inches(11), Inches(1))
            para(tf, "[本页渲染失败: %s]" % e, 14, "C0282D", first=True)
    prs.save(out_path)
    try:
        _inject_thumbnail(out_path, _cover_thumbnail(spec, th))  # 封面缩略图(方案 A),失败不影响主产物
    except Exception as e:
        print("  (缩略图注入跳过: %s)" % e, file=sys.stderr)  # 别写 stdout,MCP 模式下会污染 JSON-RPC
    return len(spec.get("slides", []))


# ════════════════════════════════════════════════════════════════
#  MCP server 部分(make_pptx 工具 + JSON-RPC over stdio,骨架对齐 obsidian)
#  上面是内联的渲染器(THEMES/版式/封面/render);下面是工具包装。
# ════════════════════════════════════════════════════════════════
import io
import re

# Windows 默认 stdout/stdin 是 GBK，MCP 协议要求 UTF-8（且 stdout 只能有 JSON-RPC）
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")


def _sanitize_filename(name):
    name = str(name or "演示文稿").strip()
    name = re.sub(r'[\\/:*?"<>|\x00-\x1f]', "", name)   # 去非法字符
    name = name.replace("..", "").strip(". ") or "演示文稿"
    return name[:80]


def _safe_join(base, name):
    full = os.path.abspath(os.path.join(base, name))
    base_abs = os.path.abspath(base)
    if not (full == base_abs or full.startswith(base_abs + os.sep)):  # 防 .. 越界
        full = os.path.join(base_abs, "演示文稿.pptx")
    stem, ext = os.path.splitext(full)
    i = 1
    while os.path.exists(full):                          # 重名加序号
        full = "%s(%d)%s" % (stem, i, ext)
        i += 1
    return full


def _coerce_slides(slides):
    """把模型传来的 slides 归一成 list[dict]。
    - list → 过滤出 dict;
    - str → 先严格 json.loads;失败则逐个抠出顶层 {...} 对象各自解析、坏的跳过
      (模型大纲一长常生成非法 JSON:对象提前闭合 / 数组里塞 key 等)。
    把"整份解析失败 → 空 PPT"降级成"最多丢一两页"。"""
    if isinstance(slides, list):
        return [s for s in slides if isinstance(s, dict)]
    if not isinstance(slides, str):
        return []
    s = slides.strip()
    try:
        v = json.loads(s)
        if isinstance(v, list):
            return [x for x in v if isinstance(x, dict)]
        if isinstance(v, dict):
            return [v]
    except Exception:
        pass
    # 尽力抢救:扫顶层平衡的 {...},逐个解析(字符串内的括号不计深度)
    out, depth, start, in_str, esc = [], 0, -1, False, False
    for i, ch in enumerate(s):
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
            continue
        if ch == '"':
            in_str = True
        elif ch == "{":
            if depth == 0:
                start = i
            depth += 1
        elif ch == "}":
            if depth > 0:
                depth -= 1
                if depth == 0 and start >= 0:
                    try:
                        obj = json.loads(s[start:i + 1])
                        if isinstance(obj, dict):
                            out.append(obj)
                    except Exception:
                        pass
                    start = -1
    return out


def make_pptx(theme="business-blue", slides=None, filename="演示文稿"):
    # 模型常把 slides 序列化成 JSON 字符串传入,且大纲一长就容易生成非法 JSON。
    # _coerce_slides 严格失败时逐个抢救顶层 {...}(坏页跳过);彻底空则下面 loud error,
    # 让模型重试 —— 绝不静默产出空 PPT。
    slides = _coerce_slides(slides)
    if not slides:
        return {"ok": False,
                "error": "slides 为空或无法解析为合法 JSON 数组。请重新调用 make_pptx,"
                         "slides 传一个数组,每页一个对象,例如 "
                         "[{\"layout\":\"cover\",\"title\":\"...\"},{\"layout\":\"bullets\",\"title\":\"...\",\"bullets\":[\"...\"]}]。"}
    theme = _resolve_theme(theme)
    # 落盘目录:优先用 app 注入的会话产物目录;否则用用户可写的 ~/.pinvou3 默认产物目录。
    # 绝不用 os.getcwd() —— MCP server 的 cwd 是 app 安装目录(release 下是 Program Files,
    # 写入会因无权限直接失败,且文件也不该落进程序目录)。
    _default_art = os.path.join(os.path.expanduser("~"), ".pinvou3", "sessions", "default", "artifacts")
    art = os.environ.get("PINVOU3_SESSION_ARTIFACTS") or _default_art
    try:
        os.makedirs(art, exist_ok=True)
    except Exception:
        art = os.path.join(os.path.expanduser("~"), ".pinvou3")
        os.makedirs(art, exist_ok=True)
    out = _safe_join(art, _sanitize_filename(filename) + ".pptx")
    try:
        n = render({"theme": theme, "slides": slides}, out)
        return {"ok": True, "path": out, "slides": n, "theme": theme}
    except Exception as e:
        return {"ok": False, "error": str(e)}


TOOL_DEFS = [{
    "name": "make_pptx",
    "description": (
        "本地直出可编辑 PowerPoint(.pptx):输入结构化 deck(主题 + slides 数组),"
        "套主题模板渲染生成,真·可编辑图表,数据不出机。生成后拿到 path,需再调 present_artifact 上卡。"
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "theme": {
                "type": "string",
                "description": ("主题(按 PPT 内容自动选):business-blue/tech-dark/gov-red/creative-purple/"
                                "fresh-green/warm-orange/navy-gold/minimal-mono/midnight。缺省 business-blue"),
            },
            "slides": {
                "type": "array",
                "description": (
                    "deck 的 slides 数组。每页一个对象,`layout` 选版式,并**必须按版式填正文字段**"
                    "(只给 title 会得到空白页!):\n"
                    "- cover: {layout, title, subtitle, date}\n"
                    "- agenda: {layout, title, items:[字符串,...]}\n"
                    "- section: {layout, title}  ← 章节分隔页\n"
                    "- bullets: {layout, title, bullets:[字符串,...]}\n"
                    "- two_col: {layout, title, leftTitle, left:[字符串,...], rightTitle, right:[字符串,...]}\n"
                    "- kpi: {layout, title, items:[{num, label},...]}  ← 最多4个大数字\n"
                    "- chart: {layout, title, chartType:'bar'|'pie'|'line', categories:[字符串,...], series:[{name, data:[数字,...]},...]}\n"
                    "- table: {layout, title, headers:[字符串,...], rows:[[单元格,...],...]}\n"
                    "- image: {layout, title, bullets:[字符串,...], caption, image:<本地图片绝对路径，可选>}\n"
                    "- end: {layout, title, subtitle}  ← 结尾页\n"
                    "正文要有真实内容/数据,不要只放标题。"
                ),
            },
            "filename": {"type": "string", "description": "文件名(不含扩展名),缺省 演示文稿"},
        },
        "required": ["slides"],
    },
}]
DISPATCH = {"make_pptx": make_pptx}


def _send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _result(req_id, result):
    _send({"jsonrpc": "2.0", "id": req_id, "result": result})


def _error(req_id, code, message):
    _send({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}})


def _handle(msg):
    method = msg.get("method")
    req_id = msg.get("id")
    if req_id is None:
        return
    if method == "initialize":
        _result(req_id, {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pinvou3-pptx", "version": "1.0.0"},
        })
    elif method == "tools/list":
        _result(req_id, {"tools": TOOL_DEFS})
    elif method == "tools/call":
        params = msg.get("params") or {}
        name = params.get("name")
        fn = DISPATCH.get(name)
        if fn is None:
            _error(req_id, -32601, "unknown tool: %s" % name)
            return
        args = params.get("arguments") or {}
        result = fn(**args)
        _result(req_id, {"content": [{"type": "text", "text": json.dumps(result, ensure_ascii=False)}]})
    else:
        _error(req_id, -32601, "method not found: %s" % method)


def main():
    for line in sys.stdin:
        line = line.lstrip("﻿").strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        try:
            _handle(msg)
        except Exception as e:
            rid = msg.get("id") if isinstance(msg, dict) else None
            if rid is not None:
                _error(rid, -32603, "internal error: %s" % e)


if __name__ == "__main__":
    main()
