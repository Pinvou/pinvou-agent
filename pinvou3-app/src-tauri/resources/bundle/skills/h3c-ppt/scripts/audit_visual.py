#!/usr/bin/env python3
"""
H3C Deck Audit · A. 视觉规范扫描
扫 slides/*.html 的字号、字体、颜色 token 偏离

Usage:
    python3 audit_visual.py <path-to-HTML_Deck-or-slides-dir>

Exit code:
    0 = 全过
    1 = 有 WARN
    2 = 有 FAIL
"""
from __future__ import annotations
import argparse
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

# ============ H3C 蓝版基线规范(默认值;脚本会优先从 base.css 读 token) ============
ALLOWED_FONT_SIZES = {18, 22, 36, 52, 80, 96, 144}  # px
FONT_SIZE_FLOOR = 18       # 死线
FONT_SIZE_CEIL = 200       # KPI 例外
LEVELS_PER_PAGE_LIMIT = 3  # 一页内容字号档(KPI/封面/章节锚不算)

# 字体黑名单(整套 0 容忍)
FONT_BLACKLIST = re.compile(
    r"(?<![A-Za-z])(宋体|SimSun|FangSong|仿宋|楷体|KaiTi|Songti)(?![A-Za-z])"
)

# 字体白名单(中文 + 数字英文 + monospace)
FONT_WHITELIST = {
    "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", "Noto Sans SC",
    "Inter", "SF Pro Display", "SF Pro", "Helvetica Neue", "system-ui",
    "Source Han Serif SC", "Noto Serif CJK SC", "Noto Serif SC",
    "JetBrains Mono", "SF Mono", "Consolas", "monospace",
    "sans-serif", "serif",
    "-apple-system", "BlinkMacSystemFont",
}

# 衬线只允许的 selector 关键词
SERIF_ALLOWED_SELECTOR_HINTS = re.compile(r"\.[a-zA-Z0-9_-]*(quote|cite)[a-zA-Z0-9_-]*")

# KPI 类(允许超大字号的 class 名提示)· sec-num/sec-bignum 等章节扉页大装饰数字也豁免
KPI_HINTS = re.compile(r"\.(kpi|hero|cover|section|sec-num|sec-bignum)", re.I)

# ============ 颜色:常用 H3C 蓝版 token 名(读 base.css 为准) ============
EXPECTED_COLOR_TOKENS = {
    "--bg-1", "--bg-2", "--bg-3", "--bg-line",
    "--gx-red", "--gx-red-deep", "--gold", "--gold-bright", "--gold-deep",
    "--t-1", "--t-2", "--t-3", "--t-4",
}

# 禁用色相(霓虹/品红/紫等)
COLOR_BANNED = re.compile(r"#(ff00ff|f0f|ff0080|ff1493|c71585|9400d3|8a2be2|ee82ee)\b", re.I)


# ============================================================
# 工具函数
# ============================================================

def find_slides_dir(root: Path) -> Path:
    """支持传 HTML_Deck/ 或 HTML_Deck/slides/ 都能定位到 slides 目录"""
    if (root / "slides").is_dir():
        return root / "slides"
    if root.name == "slides":
        return root
    # 找一找
    for p in root.rglob("slides"):
        if p.is_dir() and any(p.glob("*.html")):
            return p
    return root


def read_base_css_tokens(deck_root: Path) -> dict:
    """从 base.css 提取 --fs-* 字号 token 的实际值"""
    css = deck_root / "assets" / "base.css"
    if not css.is_file():
        # 也试一试 root/base.css
        for p in [deck_root / "base.css", deck_root / "assets/base.css"]:
            if p.is_file():
                css = p; break
    if not css.is_file():
        return {}
    txt = css.read_text(encoding="utf-8")
    out = {}
    for m in re.finditer(r"--fs-([\w-]+):\s*(\d+)px", txt):
        out[m.group(1)] = int(m.group(2))
    return out


# ============================================================
# Reporters
# ============================================================

class Report:
    def __init__(self):
        self.passes = []
        self.warns = []
        self.fails = []

    def ok(self, msg): self.passes.append(msg)
    def warn(self, msg): self.warns.append(msg)
    def fail(self, msg): self.fails.append(msg)

    def render(self):
        print()
        print("=" * 60)
        print(f" ✅ PASS · 已通过 ({len(self.passes)} 项)")
        print("=" * 60)
        for m in self.passes:
            print(f"  {m}")
        print()
        print("=" * 60)
        print(f" ⚠️  WARN · 警告级 ({len(self.warns)} 项)")
        print("=" * 60)
        for m in self.warns:
            print(f"  {m}")
        print()
        print("=" * 60)
        print(f" ❌ FAIL · 死线违规 ({len(self.fails)} 项)")
        print("=" * 60)
        for m in self.fails:
            print(f"  {m}")
        print()
        return 2 if self.fails else (1 if self.warns else 0)


# ============================================================
# Audits
# ============================================================

def audit_font_sizes(slides: list, allowed: set, rep: Report):
    """扫所有 font-size,统计违规"""
    px_pat = re.compile(r"font-size:\s*(\d+(?:\.\d+)?)px", re.I)

    total_uses = Counter()           # px → 用量
    bad_files = defaultdict(list)    # file → [(line, value, css_around)]
    sub_floor = []                   # < FONT_SIZE_FLOOR 的死线
    over_ceil = []                   # > FONT_SIZE_CEIL 的死线

    for fp in slides:
        lines = fp.read_text(encoding="utf-8").split("\n")
        for i, line in enumerate(lines, 1):
            for m in px_pat.finditer(line):
                v = float(m.group(1))
                total_uses[v] += 1
                if v < FONT_SIZE_FLOOR:
                    sub_floor.append((fp.name, i, v, line.strip()[:120]))
                elif v > FONT_SIZE_CEIL:
                    # 看是否 KPI class — 同行命中或倒序回溯 5 行内 selector 命中都豁免
                    is_kpi = bool(KPI_HINTS.search(line))
                    if not is_kpi:
                        for j in range(i - 2, max(i - 7, -1), -1):  # 从近到远倒序
                            prev = lines[j]
                            if KPI_HINTS.search(prev):
                                is_kpi = True; break
                            if "}" in prev: break  # 越过 selector 边界
                    if not is_kpi:
                        over_ceil.append((fp.name, i, v, line.strip()[:120]))
                elif v not in allowed:
                    # 偏离 6 档但还在死线内
                    bad_files[fp.name].append((i, v, line.strip()[:120]))

    # 总览
    if total_uses:
        spec = sorted(allowed)
        in_spec = sum(c for v, c in total_uses.items() if v in allowed)
        out_spec = sum(c for v, c in total_uses.items() if v not in allowed)
        rep.ok(f"font-size 共 {sum(total_uses.values())} 处 · 合规 {in_spec} · 偏离 {out_spec}")

    # 死线
    for fn, ln, v, ctx in sub_floor:
        rep.fail(f"FONT-SIZE FLOOR  {fn}:{ln}  {v}px < {FONT_SIZE_FLOOR}px  ← 必修(死线)")
    for fn, ln, v, ctx in over_ceil:
        rep.fail(f"FONT-SIZE CEIL   {fn}:{ln}  {v}px > {FONT_SIZE_CEIL}px (非 KPI class)  ← 必修")

    # 偏离 6 档
    for fn, items in bad_files.items():
        for ln, v, ctx in items:
            nearest = min(allowed, key=lambda a: abs(a - v))
            rep.warn(f"FONT-SIZE OFF    {fn}:{ln}  {v}px ∉ {sorted(allowed)}  ← 建议改 {nearest}px")


def audit_levels_per_page(slides: list, allowed: set, rep: Report):
    """统计每页内容字号档数(剔除 KPI)"""
    px_pat = re.compile(r"font-size:\s*(\d+)px", re.I)
    KPI_VALUES = {96, 144}  # 默认 KPI 档不计入"内容档数"

    over = []
    for fp in slides:
        sizes = set()
        for m in px_pat.finditer(fp.read_text(encoding="utf-8")):
            v = int(m.group(1))
            if v in allowed and v not in KPI_VALUES:
                sizes.add(v)
        if len(sizes) > LEVELS_PER_PAGE_LIMIT:
            over.append((fp.name, len(sizes), sorted(sizes)))

    if not over:
        rep.ok(f"字号档数 ≤ {LEVELS_PER_PAGE_LIMIT}/页 · 全部合规")
    for fn, n, sizes in over:
        rep.warn(f"LEVELS    {fn}  内容档 {n} 级(超过 {LEVELS_PER_PAGE_LIMIT}): {sizes}  ← 建议拆页")


def audit_font_blacklist(slides: list, rep: Report):
    """0 容忍宋体/楷体/仿宋"""
    hits = []
    for fp in slides:
        txt = fp.read_text(encoding="utf-8")
        for m in FONT_BLACKLIST.finditer(txt):
            ln = txt[:m.start()].count("\n") + 1
            hits.append((fp.name, ln, m.group(1)))

    if not hits:
        rep.ok("字体黑名单(宋体/SimSun/FangSong/仿宋/楷体/KaiTi/Songti) · 0 命中 ✓")
    else:
        for fn, ln, kw in hits:
            rep.fail(f"FONT BLACKLIST {fn}:{ln}  含禁用字体 「{kw}」  ← 必修(死线)")


def audit_serif_scope(slides: list, rep: Report):
    """衬线只允许在 .quote / [class*=quote] / [class*=-cite] 内"""
    serif_pat = re.compile(r'font-family:\s*"?(Source Han Serif|Noto Serif)[^;}]+', re.I)
    out_of_scope = []

    for fp in slides:
        lines = fp.read_text(encoding="utf-8").split("\n")
        # 找 selector 块,记录每个 font-family 行所属的 selector
        cur_selector = ""
        for i, line in enumerate(lines, 1):
            sel_match = re.match(r"\s*(\.[a-zA-Z0-9_\-\s>+~,.\:\[\]\"=*]+)\s*\{", line)
            if sel_match:
                cur_selector = sel_match.group(1).strip()
            if serif_pat.search(line):
                # 检查 selector 是否含 quote 或 cite
                if not SERIF_ALLOWED_SELECTOR_HINTS.search(cur_selector):
                    # 也检查 line 自身(单行 selector { font-family: serif })
                    if not SERIF_ALLOWED_SELECTOR_HINTS.search(line):
                        out_of_scope.append((fp.name, i, cur_selector or "?", line.strip()[:120]))

    if not out_of_scope:
        rep.ok("衬线限定 · 仅 .quote / [class*=quote] / [class*=-cite] 内使用 ✓")
    else:
        for fn, ln, sel, ctx in out_of_scope:
            rep.warn(f"SERIF OUT-OF-SCOPE {fn}:{ln}  selector={sel}  ← 改 sans 或 rename class 含 quote")


def audit_decimal_px(slides: list, rep: Report):
    """小数 px 是隐藏破死线的高发地(13.5px 等)"""
    pat = re.compile(r"font-size:\s*(\d+\.\d+)px", re.I)
    hits = []
    for fp in slides:
        txt = fp.read_text(encoding="utf-8")
        for m in pat.finditer(txt):
            ln = txt[:m.start()].count("\n") + 1
            v = float(m.group(1))
            hits.append((fp.name, ln, v))
    if not hits:
        rep.ok("小数像素 font-size · 0 命中 ✓")
    else:
        for fn, ln, v in hits:
            level = "FAIL" if v < FONT_SIZE_FLOOR else "WARN"
            (rep.fail if level == "FAIL" else rep.warn)(
                f"FONT-SIZE DECIMAL {fn}:{ln}  {v}px (小数像素,隐藏违规)  ← 改成整数 px 并就近规整"
            )


def audit_color_banned(slides: list, rep: Report):
    """禁用色相"""
    hits = []
    for fp in slides:
        txt = fp.read_text(encoding="utf-8")
        for m in COLOR_BANNED.finditer(txt):
            ln = txt[:m.start()].count("\n") + 1
            hits.append((fp.name, ln, m.group(0)))
    if not hits:
        rep.ok("配色禁用色相(品红/紫/霓虹) · 0 命中 ✓")
    else:
        for fn, ln, c in hits:
            rep.warn(f"COLOR BANNED {fn}:{ln}  {c}  ← 改用品牌 token")


# ============================================================
# Main
# ============================================================

def main():
    ap = argparse.ArgumentParser(description="H3C Deck Audit · 视觉规范扫描")
    ap.add_argument("path", help="HTML_Deck 根目录或 slides 目录")
    args = ap.parse_args()

    root = Path(args.path).resolve()
    slides_dir = find_slides_dir(root)
    if not slides_dir.is_dir():
        print(f"❌ 找不到 slides 目录(从 {root} 起)", file=sys.stderr)
        sys.exit(2)
    slides = sorted(slides_dir.glob("*.html"))
    if not slides:
        print(f"❌ {slides_dir} 没有 .html", file=sys.stderr)
        sys.exit(2)

    print(f"目标: {slides_dir}  ({len(slides)} 张 slides)")

    # 优先用 base.css 的 --fs-* 实际值;读不到回落默认
    deck_root = root if (root / "assets/base.css").exists() or (root / "base.css").exists() else slides_dir.parent
    css_tokens = read_base_css_tokens(deck_root)
    allowed = set(css_tokens.values()) if css_tokens else ALLOWED_FONT_SIZES
    print(f"允许字号档: {sorted(allowed)}px (来源: {'base.css' if css_tokens else '默认 H3C 6 档'})")

    rep = Report()
    audit_font_sizes(slides, allowed, rep)
    audit_decimal_px(slides, rep)
    audit_levels_per_page(slides, allowed, rep)
    audit_font_blacklist(slides, rep)
    audit_serif_scope(slides, rep)
    audit_color_banned(slides, rep)

    sys.exit(rep.render())


if __name__ == "__main__":
    main()
