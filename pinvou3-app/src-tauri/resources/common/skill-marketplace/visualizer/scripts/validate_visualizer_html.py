#!/usr/bin/env python3
import argparse
import html.parser
import json
import re
import sys
from pathlib import Path


CHART_JS_UMD = "https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.js"
BANNED_PATTERNS = [
    ("echarts", re.compile(r"echarts", re.I)),
    ("Plotly", re.compile(r"Plotly|cdn\.plot\.ly", re.I)),
    ("ECharts CDN", re.compile(r"cdn\.jsdelivr\.net/npm/echarts", re.I)),
]


class CanvasParser(html.parser.HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.canvases = []
        self._current = None
        self.script_style_blocks = []
        self._text_tag = None
        self._text_parts = []
        self.comments = []

    def handle_starttag(self, tag, attrs):
        tag = tag.lower()
        if tag == "canvas":
            self._current = {"attrs": dict(attrs), "text": ""}
        elif tag in {"script", "style"}:
            self._text_tag = tag
            self._text_parts = []

    def handle_data(self, data):
        if self._current is not None:
            self._current["text"] += data
        if self._text_tag is not None:
            self._text_parts.append(data)

    def handle_endtag(self, tag):
        tag = tag.lower()
        if tag == "canvas" and self._current is not None:
            self.canvases.append(self._current)
            self._current = None
        elif tag == self._text_tag:
            self.script_style_blocks.append((tag, "".join(self._text_parts)))
            self._text_tag = None
            self._text_parts = []

    def handle_comment(self, data):
        self.comments.append(data)


def strip_quoted_content(line):
    out = []
    quote = None
    escaped = False
    for ch in line:
        if quote:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == quote:
                quote = None
            out.append(" ")
        else:
            if ch in {"'", '"', "`"}:
                quote = ch
                out.append(" ")
            else:
                out.append(ch)
    return "".join(out)


def has_line_comment(blocks):
    for _tag, text in blocks:
        for line in text.splitlines():
            stripped = strip_quoted_content(line).strip()
            if "//" in stripped:
                return True
    return False


def validate(path):
    text = path.read_text(encoding="utf-8")
    parser = CanvasParser()
    parser.feed(text)

    errors = []
    warnings = []

    if CHART_JS_UMD not in text:
        errors.append(f"missing Chart.js UMD: {CHART_JS_UMD}")

    for label, pattern in BANNED_PATTERNS:
        if pattern.search(text):
            errors.append(f"banned library or token found: {label}")

    if "<!--" in text or parser.comments:
        errors.append("HTML comments are not allowed")
    if re.search(r"/\*", text):
        errors.append("CSS/JS block comments are not allowed")
    if has_line_comment(parser.script_style_blocks):
        errors.append("JS/CSS line comments are not allowed")

    if not parser.canvases:
        errors.append("no canvas elements found")
    for idx, canvas in enumerate(parser.canvases, start=1):
        attrs = {str(k).lower(): v for k, v in canvas["attrs"].items()}
        if attrs.get("role") != "img":
            errors.append(f"canvas #{idx} missing role=\"img\"")
        aria = (attrs.get("aria-label") or "").strip()
        if len(aria) < 8:
            errors.append(f"canvas #{idx} missing descriptive aria-label")
        fallback = canvas["text"].strip()
        if not fallback:
            errors.append(f"canvas #{idx} missing fallback text")
        if "height" in attrs:
            errors.append(f"canvas #{idx} sets height attribute; set height on wrapper instead")

    if not re.search(r"legend\s*:\s*\{\s*display\s*:\s*false", text):
        errors.append("Chart.js default legend is not explicitly disabled")
    if not re.search(r"class\s*=\s*['\"][^'\"]*legend|id\s*=\s*['\"][^'\"]*legend|custom-legend|legend-item", text, re.I):
        errors.append("custom HTML legend markup not found")

    if re.search(r"linear-gradient|radial-gradient|box-shadow|text-shadow|filter\s*:\s*blur|backdrop-filter|glow|neon", text, re.I):
        errors.append("decorative gradient, shadow, blur, glow, or neon styling found")

    heavy_weights = sorted(set(re.findall(r"font-weight\s*:\s*(?!400\b|500\b)([0-9]{3}|bold|bolder)", text, re.I)))
    if heavy_weights:
        errors.append("font weights outside 400/500 found: " + ", ".join(heavy_weights))

    tiny_fonts = [m.group(1) for m in re.finditer(r"font-size\s*:\s*(\d+(?:\.\d+)?)px", text, re.I) if float(m.group(1)) < 11]
    if tiny_fonts:
        errors.append("font sizes below 11px found: " + ", ".join(sorted(set(tiny_fonts))))

    if re.search(r"[\U0001F300-\U0001FAFF]", text):
        errors.append("emoji characters are not allowed")

    result = {
        "ok": not errors,
        "path": str(path),
        "canvas_count": len(parser.canvases),
        "errors": errors,
        "warnings": warnings,
    }
    return result


def main():
    ap = argparse.ArgumentParser(description="Validate Pinvou visualizer HTML artifacts.")
    ap.add_argument("html", help="Path to the generated .html artifact")
    ap.add_argument("--json", action="store_true", help="Print JSON instead of text")
    args = ap.parse_args()

    path = Path(args.html)
    if not path.is_file():
        result = {"ok": False, "path": str(path), "canvas_count": 0, "errors": ["file not found"], "warnings": []}
    else:
        result = validate(path)

    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        if result["ok"]:
            print(f"OK visualizer artifact: {result['path']} ({result['canvas_count']} canvas)")
        else:
            print(f"FAILED visualizer artifact: {result['path']}")
            for error in result["errors"]:
                print(f"- {error}")

    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
