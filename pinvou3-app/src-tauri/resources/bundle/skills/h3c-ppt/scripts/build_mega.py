#!/usr/bin/env python3
"""
<DECK_ROOT>_inline/slides/*.html → 合并成 1 个 mega.html

策略:
  - 读 inline 目录(图已 base64 内联)的所有 slides
  - 每个 slide HTML 整体 base64 编码,放进 mega.html 的 SLIDES_B64 数组
  - iframe.src = "data:text/html;charset=utf-8;base64," + SLIDES_B64[i]
  - 浏览器原生 decode,CSS 完全隔离,翻页瞬间

用法:
    python3 build_mega.py /abs/path/to/HTML_Deck

输出: <DECK_ROOT>_inline/mega.html (单文件,client 双击即看)
"""
from __future__ import annotations
import argparse
import base64
import re
import sys
from pathlib import Path


def main():
    ap = argparse.ArgumentParser(description="Build single-file mega.html from inline slides")
    ap.add_argument("deck_root", help="HTML_Deck root directory (the original, not _inline)")
    args = ap.parse_args()

    deck_root = Path(args.deck_root).resolve()
    inline_dir = deck_root.parent / f"{deck_root.name}_inline"
    out = inline_dir / "mega.html"

    if not inline_dir.exists():
        print(f"❌ {inline_dir} 不存在,先跑 inline_images.py", file=sys.stderr)
        sys.exit(1)

    idx_text = (inline_dir / "index.html").read_text(encoding="utf-8")

    # 提取 SLIDES 数组顺序
    m = re.search(r'(?:const|let|var)\s+SLIDES\s*=\s*\[(.+?)\];', idx_text, re.S)
    if not m:
        print("❌ index.html 找不到 SLIDES 数组", file=sys.stderr)
        sys.exit(1)
    items = re.findall(r'[\"\']([\w\-/]+\.html)[\"\']', m.group(1))
    print(f"📊 SLIDES 顺序共 {len(items)} 张")

    # 编码每张 slide
    slides_b64 = []
    total_bytes = 0
    for rel in items:
        p = inline_dir / rel
        if not p.exists():
            print(f"  ⚠ 缺 {rel}, skip")
            continue
        raw = p.read_text(encoding="utf-8").encode("utf-8")
        total_bytes += len(raw)
        slides_b64.append(base64.b64encode(raw).decode("ascii"))
    print(f"📦 总 inline HTML: {total_bytes/1024/1024:.1f} MB → base64 {sum(len(b) for b in slides_b64)/1024/1024:.1f} MB")

    # 替换 SLIDES 数组为 base64
    new_array = "const SLIDES_B64 = [\n" + ",\n".join(f'"{b}"' for b in slides_b64) + "\n];"
    new_text = re.sub(r'(?:const|let|var)\s+SLIDES\s*=\s*\[(.+?)\];', new_array, idx_text, count=1, flags=re.S)

    # 兼容垫片:把 base64 映射为 data URI
    shim = """
// mega.html 兼容垫片: 把 base64 数组映射为 data URI 数组
const SLIDES = SLIDES_B64.map(b => "data:text/html;charset=utf-8;base64," + b);
"""
    new_text = new_text.replace(new_array, new_array + "\n" + shim, 1)

    out.write_text(new_text, encoding="utf-8")
    size = out.stat().st_size
    print(f"\n✅ {out}  {size/1024/1024:.1f} MB")


if __name__ == "__main__":
    main()
