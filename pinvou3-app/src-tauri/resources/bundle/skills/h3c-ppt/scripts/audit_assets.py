#!/usr/bin/env python3
"""
H3C Deck Audit · C. 演示截图 / 现场实景图清洁度扫描

检查项:
- 图片文件名扫"五颜六色旧 demo"特征(launcher / nch / 任何项目 .audit/legacy_assets.txt 列出的旧资产)
- 图片像素扫 Gemini PRO logo / Try Gemini Canvas / Report unsafe content / ChatGPT 等水印
- 图片像素 OCR 抽屏区文字,扫:
   · 英文 placeholder(Bread/Milk/Apple/Oil/Egg/Detergent/Fertilizer/Water/Product/Price)
   · AI 中文渲染常见乱码(Saving saving / Trachs / PHP / Shippinng bonds 等)
- 默认色统计扫"黑底霓虹"老风格(平均 brightness < 阈值则警告)

OCR 是可选项:有 tesseract/cnocr 就跑,没有就只查文件名 + 平均色

Usage:
    python3 audit_assets.py <path-to-HTML_Deck>
"""
from __future__ import annotations
import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

try:
    from PIL import Image
    PIL_OK = True
except ImportError:
    PIL_OK = False

# 老旧 demo 文件名特征(项目可在 .audit/legacy_assets.txt 添加更多)
LEGACY_FILENAME_HINTS = [
    "launcher.png",   # 黑底 launcher 旧 demo
    "nch.png", "health.png", "videocall.png",  # emoji 头像系列
    "analytics.png", "procurement.png", "settlement.png",
    "training.png", "subsidy.png", "identify.png",
    "jdshelf.png", "inspection.png", "emergency.png", "policy.png",
]

# 屏幕英文 placeholder 词典
ENGLISH_PLACEHOLDER_WORDS = [
    "Bread", "Milk", "Apple", "Oil", "Egg", "Detergent",
    "Fertilizer", "Water", "Product", "Price", "Local Supplier",
    "Online Wholesaler", "Saving",
]

# AI 中文渲染常见错拼词典
AI_GIBBERISH_WORDS = [
    "Saving saving", "Trachs", "PHP", "Shippinng", "shippinng",
    "wholeale", "wholesaler", "Wholesaler",  # 跟上下文判
]

# AI 平台水印
AI_WATERMARK_PHRASES = [
    "Try Gemini Canvas", "Gemini PRO", "Report unsafe content",
    "ChatGPT", "Made with ChatGPT", "Midjourney --v",
]


class Report:
    def __init__(self): self.passes, self.warns, self.fails = [], [], []
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
    if (p / "assets").is_dir() and (p / "slides").is_dir():
        return p
    if (p.parent / "assets").is_dir():
        return p.parent
    return p


def has_ocr() -> bool:
    return shutil.which("tesseract") is not None


def list_referenced_images(deck_root: Path) -> list:
    """所有 slides/*.html 引用的图片(去重)"""
    refs = set()
    pat = re.compile(r"['\"\(]\s*\.{1,2}/(?:assets/[^'\"\)]+\.(png|jpg|jpeg|webp|svg))", re.I)
    for fp in (deck_root / "slides").glob("*.html"):
        txt = fp.read_text(encoding="utf-8", errors="ignore")
        for m in pat.finditer(txt):
            ref = m.group(0).strip("'\"(")
            if ref.startswith("./"): ref = ref[2:]
            elif ref.startswith("../"): ref = ref[3:]
            refs.add(ref)
    # 也直接列 assets/demo-shots 下所有(可能没在 slides 里被引但仍在仓库)
    for sub in ("demo-shots", "screenshots", "scene-library", "scene-library-v3"):
        d = deck_root / "assets" / sub
        if d.is_dir():
            for f in d.iterdir():
                if f.suffix.lower() in (".png", ".jpg", ".jpeg", ".webp"):
                    refs.add(f"assets/{sub}/{f.name}")
    return sorted(refs)


def audit_legacy_filenames(deck_root: Path, refs: list, rep: Report):
    """文件名命中老旧资产黑名单"""
    user_hints = []
    lst_file = deck_root / ".audit" / "legacy_assets.txt"
    if lst_file.is_file():
        user_hints = [l.strip() for l in lst_file.read_text().split("\n") if l.strip() and not l.startswith("#")]
    hints = LEGACY_FILENAME_HINTS + user_hints

    # 只有在仍然被 slides 引用 且 不在 v4/ 等新版子目录下 才算违规
    bad = []
    for ref in refs:
        for h in hints:
            if ref.endswith("/" + h) and "/v4/" not in ref and "/v5/" not in ref:
                bad.append(ref)
                break
    if not bad:
        rep.ok(f"演示截图文件名 · 0 老旧黑名单命中(扫 {len(hints)} 关键词)")
    for ref in bad:
        rep.warn(f"LEGACY-IMG  仍引用旧 demo 截图  {ref}  ← 看 .audit/legacy_assets.txt 替换为新版本")


def audit_average_brightness(deck_root: Path, refs: list, rep: Report):
    """平均亮度 < 阈值的截图,可能是黑底霓虹老风格"""
    if not PIL_OK:
        rep.ok("PIL 未装 · 跳过平均色统计(pip install Pillow 即可启用)")
        return
    BRIGHT_THRESHOLD = 60  # 0-255,< 60 黑底
    DEMO_HINT = re.compile(r"(demo-shot|launcher)", re.I)
    bad = []
    checked = 0
    for ref in refs:
        if not DEMO_HINT.search(ref): continue
        p = deck_root / ref
        if not p.is_file() or p.suffix.lower() == ".svg": continue
        try:
            im = Image.open(p).convert("L").resize((100, 60))
            mean = sum(im.getdata()) / (100 * 60)
        except Exception:
            continue
        checked += 1
        if mean < BRIGHT_THRESHOLD:
            bad.append((ref, mean))
    if checked == 0:
        rep.ok("演示截图平均色 · 无可扫(无 demo-shots/launcher 系图)")
    elif not bad:
        rep.ok(f"演示截图平均色 · {checked} 张全部 ≥ 浅色(brightness ≥ {BRIGHT_THRESHOLD})")
    else:
        for ref, m in bad:
            rep.warn(f"DARK-DEMO  {ref}  平均亮度 {m:.0f}/255 (< {BRIGHT_THRESHOLD})  ← 可能是旧黑底风,确认是否需替换浅版")


def ocr_text(p: Path) -> str:
    try:
        out = subprocess.run(
            ["tesseract", str(p), "-", "-l", "chi_sim+eng"],
            capture_output=True, text=True, timeout=30
        )
        return out.stdout
    except Exception:
        return ""


def audit_ocr(deck_root: Path, refs: list, rep: Report):
    """OCR 扫英文 placeholder + AI 乱码 + 水印"""
    if not has_ocr():
        rep.ok("tesseract 未装 · 跳过 OCR 扫描(apt install tesseract-ocr tesseract-ocr-chi-sim 启用)")
        return
    DEMO_HINT = re.compile(r"(demo-shot|scene-library|launcher)", re.I)
    n_scan = 0
    bad_eng = []
    bad_gib = []
    bad_wm = []
    for ref in refs:
        if not DEMO_HINT.search(ref): continue
        p = deck_root / ref
        if not p.is_file() or p.suffix.lower() == ".svg": continue
        text = ocr_text(p)
        if not text.strip(): continue
        n_scan += 1
        for w in ENGLISH_PLACEHOLDER_WORDS:
            if w in text:
                bad_eng.append((ref, w)); break
        for w in AI_GIBBERISH_WORDS:
            if w in text:
                bad_gib.append((ref, w)); break
        for w in AI_WATERMARK_PHRASES:
            if w in text:
                bad_wm.append((ref, w)); break

    if n_scan == 0:
        rep.ok("OCR 无 demo / scene 图可扫")
        return
    if not bad_eng: rep.ok(f"OCR 英文 placeholder · {n_scan} 张图 0 命中")
    for ref, w in bad_eng:
        rep.warn(f"ENG-PLACEHOLDER  {ref}  屏内出现英文「{w}」  ← P 成中文 / 让 PIL 重贴")
    if not bad_gib: rep.ok(f"OCR AI 中文乱码 · {n_scan} 张图 0 命中")
    for ref, w in bad_gib:
        rep.fail(f"AI-GIBBERISH  {ref}  屏内出现错拼「{w}」  ← 模型渲中文不可信,用 PIL 直贴中文")
    if not bad_wm: rep.ok(f"OCR AI 水印 · {n_scan} 张图 0 命中")
    for ref, w in bad_wm:
        rep.fail(f"AI-WATERMARK   {ref}  含「{w}」水印  ← 用矩形覆盖 + PIL 重画或重导")


def main():
    ap = argparse.ArgumentParser(description="H3C Deck Audit · 截图清洁度扫描")
    ap.add_argument("path", help="HTML_Deck 根目录")
    args = ap.parse_args()

    deck_root = find_deck_root(Path(args.path).resolve())
    print(f"deck root: {deck_root}")
    print(f"OCR 可用: {'yes (tesseract)' if has_ocr() else 'no'}")
    print(f"PIL 可用: {'yes' if PIL_OK else 'no'}")

    refs = list_referenced_images(deck_root)
    print(f"扫描 {len(refs)} 张资源图")

    rep = Report()
    audit_legacy_filenames(deck_root, refs, rep)
    audit_average_brightness(deck_root, refs, rep)
    audit_ocr(deck_root, refs, rep)
    sys.exit(rep.render())


if __name__ == "__main__":
    main()
