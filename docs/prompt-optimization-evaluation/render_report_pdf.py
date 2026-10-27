"""Create the portable report from its Markdown and generated PNG figures.

Requires reportlab and a Chinese TrueType font. Run after render_report_figures.py.
Author: Codex (OpenAI) / GPT-6; exact variant/reasoning level unavailable.
"""

import html
import re
from pathlib import Path

from reportlab.lib import colors
from reportlab.lib.enums import TA_LEFT
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (
    Image, KeepTogether, LongTable, Paragraph, SimpleDocTemplate, Spacer, TableStyle,
)

ROOT = Path(__file__).resolve().parent
SOURCE = ROOT / "提升报告-V2.4.5.md"
OUTPUT = ROOT / "pinvou-improvement-report-v2.4.5.pdf"
FONT = "ReportCJK"
pdfmetrics.registerFont(TTFont(FONT, "/usr/share/fonts/truetype/arphic/uming.ttc", subfontIndex=0))
pdfmetrics.registerFontFamily(FONT, normal=FONT, bold=FONT, italic=FONT, boldItalic=FONT)
INK, MUTED, TEAL = "#172B4D", "#526174", "#138578"
styles = getSampleStyleSheet()
for name, size, leading, before, after in [
    ("BodyCJK", 10, 16, 0, 8), ("TitleCJK", 23, 31, 0, 16),
    ("HeadingCJK", 15, 22, 16, 10), ("SubCJK", 12, 18, 10, 7),
    ("CellCJK", 9, 14, 0, 0), ("CodeCJK", 9, 14, 0, 6),
    ("CaptionCJK", 8, 12, 0, 9),
]:
    styles.add(ParagraphStyle(name, fontName=FONT, fontSize=size, leading=leading,
        textColor=colors.HexColor(INK), wordWrap="CJK", spaceBefore=before,
        spaceAfter=after, alignment=TA_LEFT, splitLongWords=True,
        keepWithNext=name in ("TitleCJK", "HeadingCJK", "SubCJK")))


def inline(text):
    text = html.escape(text)
    text = re.sub(r"!\[([^]]*)\]\([^)]+\)", r"\1", text)
    text = re.sub(r"\[([^]]+)\]\(([^)]+)\)",
        lambda m: f'<link href="{m[2]}" color="{TEAL}">{m[1]}</link>'
        if m[2].startswith("https://") else m[1], text)
    text = re.sub(r"\*\*(.+?)\*\*", r"<b>\1</b>", text)
    text = re.sub(r"`([^`]+)`", r'<font color="#305472">\1</font>', text)
    return text


WIDTH = A4[0] - 88


def table(lines):
    data = [[v.strip() for v in row.strip().strip("|").split("|")] for row in lines]
    data = [r for r in data if not all(re.fullmatch(r"[:\- ]+", s) for s in r)]
    n = len(data[0])
    ratios = {2: [.25, .75], 3: [.30, .35, .35], 4: [.27, .20, .20, .33]}.get(n, [1/n]*n)
    cells = [[Paragraph(inline(cell), styles["CellCJK"]) for cell in row] for row in data]
    t = LongTable(cells, colWidths=[WIDTH*r for r in ratios], repeatRows=1, hAlign="LEFT")
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#E5F2EF")),
        ("ROWBACKGROUNDS", (0, 1), (-1, -1), [colors.white, colors.HexColor("#F5F7FA")]),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("TOPPADDING", (0, 0), (-1, -1), 7), ("BOTTOMPADDING", (0, 0), (-1, -1), 7),
        ("LEFTPADDING", (0, 0), (-1, -1), 8), ("RIGHTPADDING", (0, 0), (-1, -1), 8),
        ("LINEBELOW", (0, 0), (-1, 0), .6, colors.HexColor("#B7D7CF")),
    ]))
    return t


def footer(canvas, doc):
    canvas.saveState()
    canvas.setFont(FONT, 8)
    canvas.setFillColor(colors.HexColor(MUTED))
    canvas.drawString(44, 25, "Pinvou V2.4.5 · 2026-10-27 · Codex / GPT-6")
    canvas.drawRightString(A4[0]-44, 25, f"{doc.page}")
    canvas.restoreState()


def image_block(match):
    img = Image(str(ROOT / match[2]))
    height = min(WIDTH*img.imageHeight/img.imageWidth, A4[1]-175)
    img.drawWidth, img.drawHeight = height*img.imageWidth/img.imageHeight, height
    return [img, Paragraph(match[1], styles["CaptionCJK"])]


def build():
    lines = SOURCE.read_text().splitlines()
    story, paragraph = [], []

    def flush():
        if paragraph:
            story.append(Paragraph(inline(" ".join(paragraph)), styles["BodyCJK"]))
            paragraph.clear()

    i = 0
    while i < len(lines):
        line = lines[i]
        if not line.strip():
            flush(); i += 1; continue
        if line.startswith("```"):
            flush(); language = line[3:]; code = []; i += 1
            while i < len(lines) and not lines[i].startswith("```"):
                code.append(lines[i]); i += 1
            if language == "mermaid":
                story.append(Paragraph("共享事实/授权/格式规范 → 按普通模式与代码模式分别约束 → 复用 CodeWhale 会话与工具循环。主根规则沿用自动加载；附加根按目标路径发现规则。", styles["BodyCJK"]))
            else:
                story.append(Paragraph("<br/>".join(html.escape(v) for v in code), styles["CodeCJK"]))
            i += 1; continue
        if line.startswith("|"):
            flush(); rows = []
            while i < len(lines) and lines[i].startswith("|"):
                rows.append(lines[i]); i += 1
            story.extend([table(rows), Spacer(1, 10)]); continue
        image = re.fullmatch(r"!\[([^]]*)\]\(([^)]+)\)", line)
        if image:
            flush(); story.append(KeepTogether(image_block(image)))
            i += 1; continue
        heading = re.match(r"^(#{1,3}) (.*)", line)
        if heading:
            flush(); name = {1: "TitleCJK", 2: "HeadingCJK", 3: "SubCJK"}[len(heading[1])]
            title = Paragraph(inline(heading[2]), styles[name])
            following = i+1
            while following < len(lines) and not lines[following].strip(): following += 1
            image = re.fullmatch(r"!\[([^]]*)\]\(([^)]+)\)", lines[following]) if following < len(lines) else None
            if image:
                story.append(KeepTogether([title, *image_block(image)])); i = following+1
            else:
                story.append(title); i += 1
            continue
        paragraph.append(line); i += 1
    flush()
    doc = SimpleDocTemplate(str(OUTPUT), pagesize=A4, rightMargin=44, leftMargin=44,
        topMargin=40, bottomMargin=45, title="Pinvou 提示词与应用配套优化：图文提升报告 V2.4.5",
        author="Codex (OpenAI) / GPT-6; exact variant/reasoning level unavailable",
        subject="Known-regression evaluation; improvements, regressions and evidence boundaries")
    doc.build(story, onFirstPage=footer, onLaterPages=footer)
    print(OUTPUT.name)


if __name__ == "__main__":
    build()
