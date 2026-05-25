#!/usr/bin/env python3
"""
把 <DECK_ROOT> 复刻为 <DECK_ROOT>_inline/,所有 <img src> 和 url(...) 引用的
图片改成 base64 data URI 内联。

用法:
    python3 inline_images.py /abs/path/to/HTML_Deck

输出: 同级 <DECK_ROOT>_inline/ 目录,每个 slides/*.html 自包含。
"""
from __future__ import annotations
import argparse
import base64
import io
import mimetypes
import re
import shutil
import sys
from pathlib import Path

from PIL import Image  # type: ignore

# 路径在 main() 里根据命令行参数解析,不再从 __file__ 推断
ROOT: Path = None
DST: Path = None

MIME = {
    ".png":  "image/png",
    ".jpg":  "image/jpeg",
    ".jpeg": "image/jpeg",
    ".gif":  "image/gif",
    ".webp": "image/webp",
    ".svg":  "image/svg+xml",
}

# 压缩阈值: > 此尺寸的图启用压缩
COMPRESS_THRESHOLD_BYTES = 50 * 1024      # 50 KB
MAX_DIM = 960                             # 长边最大像素 (v3 blue final 压档)
JPG_QUALITY = 62
WEBP_QUALITY = 65


def compress_image(path: Path) -> tuple[bytes, str]:
    """大图先压再 base64. 返回 (bytes, mime)."""
    raw = path.read_bytes()
    ext = path.suffix.lower()
    # SVG / GIF 不压(SVG 是文本,GIF 动图压坏)
    if ext in (".svg", ".gif") or len(raw) <= COMPRESS_THRESHOLD_BYTES:
        return raw, MIME.get(ext, "application/octet-stream")
    try:
        im = Image.open(io.BytesIO(raw))
        im.load()
    except Exception as e:
        print(f"  ⚠ {path.name} 无法解码 PIL ({e}), 用原图", file=sys.stderr)
        return raw, MIME.get(ext, "application/octet-stream")

    # resize 到长边 MAX_DIM
    w, h = im.size
    if max(w, h) > MAX_DIM:
        ratio = MAX_DIM / max(w, h)
        nw, nh = int(w * ratio), int(h * ratio)
        im = im.resize((nw, nh), Image.LANCZOS)

    buf = io.BytesIO()
    has_alpha = (im.mode in ("RGBA", "LA")) or (im.mode == "P" and "transparency" in im.info)
    if ext == ".png" and has_alpha:
        # 透明 PNG 保 PNG (palette + optimize)
        if im.mode != "RGBA":
            im = im.convert("RGBA")
        # 转 P 调色板省体积 (256 色) — 但有 alpha 时用 RGBA
        im.save(buf, format="PNG", optimize=True)
        mime = "image/png"
    else:
        # 不透明: 一律转 JPEG q85
        if im.mode != "RGB":
            im = im.convert("RGB")
        im.save(buf, format="JPEG", quality=JPG_QUALITY, optimize=True, progressive=True)
        mime = "image/jpeg"
    out = buf.getvalue()
    # 防止压完反而更大
    if len(out) >= len(raw):
        return raw, MIME.get(ext, "application/octet-stream")
    print(f"  ↓ {path.name}: {len(raw)/1024:.0f} KB → {len(out)/1024:.0f} KB ({100*len(out)/len(raw):.0f}%)")
    return out, mime

# src="../assets/..."  src='./assets/...'
IMG_RE = re.compile(r'''(?P<attr>src\s*=\s*)["'](?P<u>[^"']+\.(?:png|jpg|jpeg|gif|webp|svg))["']''', re.I)
# url(../assets/foo.png) / url("...") / url('...')
URL_RE = re.compile(r'''url\(\s*["']?(?P<u>[^)"']+\.(?:png|jpg|jpeg|gif|webp|svg))["']?\s*\)''', re.I)


def to_data_uri(path: Path) -> str | None:
    if not path.exists():
        return None
    b, mime = compress_image(path)
    enc = base64.b64encode(b).decode("ascii")
    return f"data:{mime};base64,{enc}"


def resolve_url(html_file: Path, url: str) -> Path | None:
    """url 可能是 ../assets/x.png 或 assets/x.png (相对 html_file 所在目录)"""
    if url.startswith(("http://", "https://", "data:")):
        return None
    # 去掉锚和 query
    url = url.split("?")[0].split("#")[0]
    target = (html_file.parent / url).resolve()
    if target.exists():
        return target
    # 兜底:从 ROOT 解析
    target = (ROOT / url.lstrip("/")).resolve()
    if target.exists():
        return target
    return None


def inline_html(src: Path, dst: Path, cache: dict[Path, str]) -> tuple[int, int, list[str]]:
    text = src.read_text(encoding="utf-8")
    hit, miss = 0, 0
    missing: list[str] = []

    def repl_img(m: re.Match) -> str:
        nonlocal hit, miss
        u = m.group("u")
        p = resolve_url(src, u)
        if p is None:
            miss += 1
            missing.append(u)
            return m.group(0)
        if p not in cache:
            uri = to_data_uri(p)
            if uri is None:
                miss += 1
                missing.append(u)
                return m.group(0)
            cache[p] = uri
        hit += 1
        return f'{m.group("attr")}"{cache[p]}"'

    def repl_url(m: re.Match) -> str:
        nonlocal hit, miss
        u = m.group("u")
        p = resolve_url(src, u)
        if p is None:
            miss += 1
            missing.append(u)
            return m.group(0)
        if p not in cache:
            uri = to_data_uri(p)
            if uri is None:
                miss += 1
                missing.append(u)
                return m.group(0)
            cache[p] = uri
        hit += 1
        return f'url("{cache[p]}")'

    text = IMG_RE.sub(repl_img, text)
    text = URL_RE.sub(repl_url, text)

    # 同时把外部 stylesheet link 改成 inline <style>
    # <link rel="stylesheet" href="../assets/base.css" /> 或 href="assets/base.css"
    link_re = re.compile(r'<link[^>]*rel=["\']stylesheet["\'][^>]*href=["\']([^"\']+\.css)["\'][^>]*/?>', re.I)
    def repl_link(m: re.Match) -> str:
        nonlocal hit, miss
        href = m.group(1)
        if href.startswith(("http://", "https://")):
            return m.group(0)
        p = resolve_url(src, href)
        if p is None:
            miss += 1
            missing.append(href)
            return m.group(0)
        css = p.read_text(encoding="utf-8")
        # CSS 内的 url(...) 也要替换,以 CSS 文件为相对参考
        def repl_css_url(mm: re.Match) -> str:
            nonlocal hit, miss
            uu = mm.group("u")
            pp = resolve_url(p, uu)
            if pp is None:
                miss += 1
                missing.append(uu)
                return mm.group(0)
            if pp not in cache:
                ui = to_data_uri(pp)
                if ui is None:
                    miss += 1
                    return mm.group(0)
                cache[pp] = ui
            hit += 1
            return f'url("{cache[pp]}")'
        css = URL_RE.sub(repl_css_url, css)
        hit += 1
        return f'<style data-from="{href}">\n{css}\n</style>'
    text = link_re.sub(repl_link, text)

    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_text(text, encoding="utf-8")
    return hit, miss, missing


def main():
    global ROOT, DST
    ap = argparse.ArgumentParser(description="Inline all assets into per-slide HTML")
    ap.add_argument("deck_root", help="HTML_Deck root directory (has slides/, assets/, index.html)")
    args = ap.parse_args()

    ROOT = Path(args.deck_root).resolve()
    if not (ROOT / "index.html").is_file() or not (ROOT / "slides").is_dir():
        print(f"❌ {ROOT} 不像是 HTML_Deck 根(缺 index.html 或 slides/)", file=sys.stderr)
        sys.exit(2)
    # 输出到同级 <name>_inline/
    DST = ROOT.parent / f"{ROOT.name}_inline"

    if DST.exists():
        print(f"清理旧目录: {DST}")
        shutil.rmtree(DST)
    DST.mkdir(parents=True)

    # 复刻 DESIGN.md 等非 html 文件
    for f in ROOT.iterdir():
        if f.is_file() and f.suffix not in (".html",):
            shutil.copy2(f, DST / f.name)

    cache: dict[Path, str] = {}
    total_hit = total_miss = 0
    all_missing: list[str] = []

    # index.html
    print(f"处理 index.html ...")
    h, m, miss = inline_html(ROOT / "index.html", DST / "index.html", cache)
    total_hit += h; total_miss += m; all_missing.extend(miss)

    # slides/*.html
    slides = sorted((ROOT / "slides").glob("*.html"))
    print(f"处理 {len(slides)} slides ...")
    for s in slides:
        h, m, miss = inline_html(s, DST / "slides" / s.name, cache)
        total_hit += h; total_miss += m; all_missing.extend(miss)

    print(f"\n=== 完成 ===")
    print(f"内联成功: {total_hit} 处")
    print(f"未命中:   {total_miss} 处")
    if all_missing:
        uniq = sorted(set(all_missing))
        print(f"缺失文件(去重 {len(uniq)} 个):")
        for u in uniq[:30]:
            print(f"  - {u}")
        if len(uniq) > 30:
            print(f"  ... 还有 {len(uniq) - 30}")

    # 体积统计
    total = 0
    for f in DST.rglob("*.html"):
        total += f.stat().st_size
    print(f"\nHTML_Deck_inline/ 总体积: {total / 1024 / 1024:.1f} MB")


if __name__ == "__main__":
    main()
