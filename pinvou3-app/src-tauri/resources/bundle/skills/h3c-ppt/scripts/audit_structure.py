#!/usr/bin/env python3
"""
H3C Deck Audit · B. 结构与连贯性扫描
- 右下页码 X/Y 一致性
- 左上 CH 章节号按播放顺序连贯
- .page-footer right 给页码让位
- index.html 启动逻辑(无 hash 默认 go(0))
- 旧水印/旧 slogan 黑名单(项目自填 .audit/legacy_slogans.txt)

Usage:
    python3 audit_structure.py <path-to-HTML_Deck>
"""
from __future__ import annotations
import argparse
import re
import sys
from pathlib import Path

PAGENUM_PAT = re.compile(r'<span class="cur">(\d+)</span>\s*/\s*(\d+)')
CH_PAT = re.compile(r"CH\.(\d{1,3})")
SLIDES_ARRAY_PAT = re.compile(r'(?:const|let|var)\s+SLIDES\s*=\s*\[(.+?)\];', re.S)
SLIDE_ITEM_PAT = re.compile(r'[\"\']slides/([\w\-]+\.html)[\"\']')
PAGE_FOOTER_RIGHT_PAT = re.compile(r"\.page-footer\s*\{[^}]*right:\s*(\d+)px", re.S)
GO_DEFAULT_PAT = re.compile(r"go\s*\(\s*m\s*\?\s*parseInt|else\s*go\s*\(\s*0\s*\)")

PAGE_PAGENUM_DEFAULTS_RIGHT = 240  # .page-footer 必须 right ≥ 此值


class Report:
    def __init__(self):
        self.passes, self.warns, self.fails = [], [], []
    def ok(self, m): self.passes.append(m)
    def warn(self, m): self.warns.append(m)
    def fail(self, m): self.fails.append(m)
    def render(self):
        print()
        print("=" * 60); print(f" ✅ PASS · 已通过 ({len(self.passes)} 项)"); print("=" * 60)
        for m in self.passes: print(f"  {m}")
        print()
        print("=" * 60); print(f" ⚠️  WARN · 警告级 ({len(self.warns)} 项)"); print("=" * 60)
        for m in self.warns: print(f"  {m}")
        print()
        print("=" * 60); print(f" ❌ FAIL · 死线违规 ({len(self.fails)} 项)"); print("=" * 60)
        for m in self.fails: print(f"  {m}")
        print()
        return 2 if self.fails else (1 if self.warns else 0)


def find_deck_root(p: Path) -> Path:
    """传 HTML_Deck 或 HTML_Deck/slides 都能定位 deck root"""
    if (p / "index.html").is_file() and (p / "slides").is_dir():
        return p
    if p.name == "slides" and (p.parent / "index.html").is_file():
        return p.parent
    # 向上找 1 层
    if (p.parent / "index.html").is_file() and (p.parent / "slides").is_dir():
        return p.parent
    return p


def read_slides_order(deck_root: Path) -> list:
    """从 index.html 提取 SLIDES 数组"""
    idx = deck_root / "index.html"
    if not idx.is_file():
        return []
    txt = idx.read_text(encoding="utf-8")
    m = SLIDES_ARRAY_PAT.search(txt)
    if not m:
        return []
    return SLIDE_ITEM_PAT.findall(m.group(1))


# ============================================================

def audit_pagenum(deck_root: Path, slides_order: list, rep: Report):
    if not slides_order:
        rep.warn("无法定位 index.html SLIDES 数组,跳过页码 X/Y 校验")
        return
    total = len(slides_order)
    expected = {fn: i + 1 for i, fn in enumerate(slides_order)}
    bad = []
    no_pagenum = []
    for fn in slides_order:
        p = deck_root / "slides" / fn
        if not p.is_file():
            rep.fail(f"PAGENUM MISSING-FILE  {fn}  ← SLIDES 引用但文件不存在")
            continue
        txt = p.read_text(encoding="utf-8")
        m = PAGENUM_PAT.search(txt)
        if not m:
            no_pagenum.append(fn)
            continue
        cur, tot = int(m.group(1)), int(m.group(2))
        if cur != expected[fn] or tot != total:
            bad.append((fn, cur, tot, expected[fn], total))
    if not bad:
        rep.ok(f"页码 X/Y · {total - len(no_pagenum)}/{total} 页全合规")
    for fn, cur, tot, exp, t in bad:
        rep.fail(f"PAGENUM MISMATCH  {fn}  实际 {cur}/{tot}  期望 {exp}/{t}  ← 改 .page-pagenum 内 cur 数字与分母")
    if no_pagenum:
        rep.ok(f"无页码页(封面/章节/收尾,设计意图,跳过)· {len(no_pagenum)} 页:{', '.join(no_pagenum[:5])}{'...' if len(no_pagenum) > 5 else ''}")


def audit_ch_sequence(deck_root: Path, slides_order: list, rep: Report):
    """scene 页 CH 必须 = 它前面那张主页(非 scene)的 CH"""
    if not slides_order:
        return
    bad = []
    last_main_ch = None
    for fn in slides_order:
        p = deck_root / "slides" / fn
        if not p.is_file(): continue
        txt = p.read_text(encoding="utf-8")
        m = CH_PAT.search(txt)
        if not m:
            continue
        cur_ch = int(m.group(1))
        is_scene = "scene" in fn.lower()
        if is_scene:
            if last_main_ch is not None and cur_ch != last_main_ch:
                bad.append((fn, cur_ch, last_main_ch))
        else:
            last_main_ch = cur_ch
    if not bad:
        rep.ok("CH 章节号 · scene 页 ↔ 主页 全部一致")
    for fn, cur, expected in bad:
        rep.fail(f"CH MISMATCH  {fn}  CH.{cur:02d}  ← 应 CH.{expected:02d}(跟随前一张主页)")


def audit_page_footer_right(deck_root: Path, slides_order: list, rep: Report):
    bad = []
    for fn in slides_order or [p.name for p in (deck_root / "slides").glob("*.html")]:
        p = deck_root / "slides" / fn
        if not p.is_file(): continue
        txt = p.read_text(encoding="utf-8")
        for m in PAGE_FOOTER_RIGHT_PAT.finditer(txt):
            v = int(m.group(1))
            if v < PAGE_PAGENUM_DEFAULTS_RIGHT:
                bad.append((fn, v))
                break
    # 也看 base.css 是否有全局 override
    css = deck_root / "assets" / "base.css"
    has_global_override = False
    if css.is_file():
        css_txt = css.read_text(encoding="utf-8")
        if re.search(r"\.slide\s+\.page-footer\s*\{[^}]*right:\s*\d+px[^}]*!important", css_txt):
            has_global_override = True
    if has_global_override:
        rep.ok(f".page-footer right · base.css 有全局 !important override(覆盖 {len(bad)} 个 inline 旧值)")
    else:
        if not bad:
            rep.ok(f".page-footer right · 全部 ≥{PAGE_PAGENUM_DEFAULTS_RIGHT}px,给页码让位")
        for fn, v in bad:
            rep.warn(f"FOOTER-RIGHT  {fn}  right:{v}px (< {PAGE_PAGENUM_DEFAULTS_RIGHT})  ← 改 right:240px,或在 base.css 加全局 !important override")


def audit_index_init(deck_root: Path, rep: Report):
    """index.html 必须有 'no hash → go(0)' 默认调用,否则首屏 pageInfo 卡住"""
    idx = deck_root / "index.html"
    if not idx.is_file():
        rep.warn("找不到 index.html,跳过启动逻辑校验")
        return
    txt = idx.read_text(encoding="utf-8")
    # 匹配三种合规写法之一:
    # 1) `go(m ? parseInt(m[1]) - 1 : 0)`
    # 2) `if (m) go(...) else go(0)`
    # 3) `if (m) go(...); go(0);`
    safe = bool(GO_DEFAULT_PAT.search(txt))
    if not safe:
        # 看是否只有 if (m) go(...) 没默认
        if re.search(r"if\s*\(\s*m\s*\)\s*go\s*\(", txt):
            rep.fail("INDEX INIT  index.html 启动逻辑只在 hash 存在时 go(),无 hash 默认未触发 go(0)  ← 首屏 pageInfo 会卡占位符,补 else 或三元")
        else:
            rep.warn("INDEX INIT  无法确认 index.html 启动逻辑,人工 review 一下 go(0) 是否被默认调用")
    else:
        rep.ok("index.html 启动逻辑 · 无 hash 默认 go(0) ✓")


def audit_legacy_slogans(deck_root: Path, slides_order: list, rep: Report):
    """读 .audit/legacy_slogans.txt(项目维护),扫已被删的旧水印是否回流"""
    lst_file = deck_root / ".audit" / "legacy_slogans.txt"
    if not lst_file.is_file():
        rep.ok("旧水印黑名单未配置 · 跳过(可在 <DECK_ROOT>/.audit/legacy_slogans.txt 维护一行一条)")
        return
    slogans = [l.strip() for l in lst_file.read_text(encoding="utf-8").split("\n") if l.strip() and not l.startswith("#")]
    if not slogans:
        rep.ok("旧水印黑名单为空 · 跳过")
        return
    bad = []
    targets = [deck_root / "assets" / "base.css"] + [deck_root / "slides" / fn for fn in (slides_order or [])]
    for p in targets:
        if not p.is_file(): continue
        txt = p.read_text(encoding="utf-8")
        for s in slogans:
            if s in txt:
                bad.append((p.name, s))
    if not bad:
        rep.ok(f"旧水印黑名单 · 全本 0 命中(共扫 {len(slogans)} 条)")
    for fn, s in bad[:20]:
        rep.fail(f"LEGACY SLOGAN  {fn}  含已弃用文案 「{s[:40]}」  ← 删")


# ============================================================

def main():
    ap = argparse.ArgumentParser(description="H3C Deck Audit · 结构连贯扫描")
    ap.add_argument("path", help="HTML_Deck 根目录")
    args = ap.parse_args()

    deck_root = find_deck_root(Path(args.path).resolve())
    print(f"deck root: {deck_root}")

    slides_order = read_slides_order(deck_root)
    if slides_order:
        print(f"SLIDES 顺序: 共 {len(slides_order)} 张")
    else:
        print("⚠️  无法读 SLIDES 数组(index.html 缺或格式异常),部分检查跳过")

    rep = Report()
    audit_pagenum(deck_root, slides_order, rep)
    audit_ch_sequence(deck_root, slides_order, rep)
    audit_page_footer_right(deck_root, slides_order, rep)
    audit_index_init(deck_root, rep)
    audit_legacy_slogans(deck_root, slides_order, rep)
    sys.exit(rep.render())


if __name__ == "__main__":
    main()
