#!/usr/bin/env python3
"""
h3c-ppt skill · D. 浏览器渲染审查
启 chromium-headless + CDP,真实渲染每张 slide 1920×1080,然后注入 JS 提取
DOM 信息检查 4 项:
  (a) 文本溢出 slide 边界(>1920 宽 / >1080 高)
  (b) 文本撞右下页码禁区(right:0-220 bottom:0-80)
  (c) 文本撞右上 H3C logo 禁区(right:0-200 top:0-80)
  (d) 渲染后 computed font-size < 18px(catches em/% 等 grep 抓不到的)

输出 <DECK_ROOT>/_audit/render_audit.md

依赖:chromium-browser(已安装) · 启 HTTP server 服务 slide 文件

用法:
    python3 audit_render.py /abs/path/to/HTML_Deck [--port 18991]
"""
from __future__ import annotations
import argparse
import http.server
import json
import os
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

# 禁区定义(slide 1920×1080 内,从 right/top/bottom 起算)
PAGENUM_ZONE = {"right_x": 0, "right_w": 220, "bottom_y": 0, "bottom_h": 80}
LOGO_ZONE    = {"right_x": 0, "right_w": 200, "top_y": 0, "top_h": 80}
SLIDE_W = 1920
SLIDE_H = 1080
FONT_FLOOR = 18  # px


# JS payload · 注入到每张 slide,返回结构化 JSON
INSPECT_JS = r"""
(() => {
  const SLIDE_W = 1920, SLIDE_H = 1080;
  const PAGENUM = {x0: SLIDE_W - 220, y0: SLIDE_H - 80, x1: SLIDE_W, y1: SLIDE_H};
  const LOGO    = {x0: SLIDE_W - 200, y0: 0,            x1: SLIDE_W, y1: 80};
  const FLOOR = 18;

  // 找 .slide 容器(我们的母板根)
  const slide = document.querySelector('.slide');
  if (!slide) return JSON.stringify({error: "no .slide root found"});

  const slideBox = slide.getBoundingClientRect();
  const sx = slideBox.left, sy = slideBox.top;

  function inZone(rect, z) {
    return rect.right > z.x0 && rect.left < z.x1 && rect.bottom > z.y0 && rect.top < z.y1;
  }

  function ancestorMatches(el, regex) {
    let p = el;
    while (p && p !== slide) {
      const c = (typeof p.className === 'string' ? p.className : (p.className?.baseVal || ''));
      if (regex.test(c)) return true;
      p = p.parentElement;
    }
    return false;
  }

  const findings = {
    overflow: [],
    pagenum_collision: [],
    logo_collision: [],
    sub_floor_font: [],
  };

  // 遍历所有可见文本元素
  const all = slide.querySelectorAll('*');
  for (const el of all) {
    // 跳过空、display:none、隐藏元素
    const cs = window.getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden') continue;
    const text = (el.textContent || '').trim();
    if (!text) continue;
    // 跳过 SVG 内文本(font-size 是 SVG 坐标系,不是屏幕 px)
    if (el.ownerSVGElement || el.tagName === 'text' || el.tagName === 'tspan') continue;
    // 只看叶子文本节点的父元素(避免 .slide 整个被列)
    if (el.children.length > 0) {
      // 看看是不是有自己的直接文本
      let hasOwnText = false;
      for (const n of el.childNodes) {
        if (n.nodeType === 3 && n.textContent.trim()) { hasOwnText = true; break; }
      }
      if (!hasOwnText) continue;
    }

    const rect = el.getBoundingClientRect();
    // 转换为 slide 内坐标(slide 左上 = 0,0)
    const local = {
      left: rect.left - sx,
      top: rect.top - sy,
      right: rect.right - sx,
      bottom: rect.bottom - sy,
    };
    const fs = parseFloat(cs.fontSize);
    const tag = el.tagName.toLowerCase();
    const cls = el.className || '';
    const snippet = text.slice(0, 60);
    const ctx = `<${tag}${cls ? ' class="' + (typeof cls === 'string' ? cls : cls.baseVal || '') + '"' : ''}> ${snippet}`;

    // (a) 溢出:right > SLIDE_W 或 bottom > SLIDE_H 或 left < 0 或 top < 0
    if (local.right > SLIDE_W + 1 || local.bottom > SLIDE_H + 1 || local.left < -1 || local.top < -1) {
      findings.overflow.push({
        ctx, fs,
        local: {l: Math.round(local.left), t: Math.round(local.top), r: Math.round(local.right), b: Math.round(local.bottom)},
        slide: `${SLIDE_W}x${SLIDE_H}`
      });
    }
    // (b) 撞页码区 — 排除已知页码 / 页脚类(支持多种命名:page-pagenum/slide-pagenum/pg-pagenum/sec-footer/page-footer)
    if (inZone(local, PAGENUM)) {
      if (!ancestorMatches(el, /pagenum|page-num|sec-footer|page-footer|slide-footer/i)) {
        findings.pagenum_collision.push({
          ctx, fs,
          local: {l: Math.round(local.left), t: Math.round(local.top), r: Math.round(local.right), b: Math.round(local.bottom)}
        });
      }
    }
    // (c) 撞 logo 区 — 排除 eyebrow / header / 任何已经在顶部的 page-num / pagenum
    if (inZone(local, LOGO)) {
      if (!ancestorMatches(el, /eyebrow|header|pagenum|page-num/i)) {
        findings.logo_collision.push({
          ctx, fs,
          local: {l: Math.round(local.left), t: Math.round(local.top), r: Math.round(local.right), b: Math.round(local.bottom)}
        });
      }
    }
    // (d) 字号 < FLOOR(渲染后 computed)
    if (fs > 0 && fs < FLOOR) {
      findings.sub_floor_font.push({ctx, fs: Math.round(fs * 10) / 10});
    }
  }

  return JSON.stringify(findings);
})()
"""


# ============================================================
# CDP HTTP API helpers(无需第三方库)
# ============================================================

def cdp_get(port: int, path: str, timeout: int = 5):
    url = f"http://127.0.0.1:{port}{path}"
    return json.loads(urllib.request.urlopen(url, timeout=timeout).read())


def cdp_send_ws(ws_url: str, method: str, params: dict | None = None, msg_id: int = 1, timeout: int = 30):
    """通过 websocket 发 CDP 命令 · 不依赖第三方 ws 库,用最小手写客户端"""
    import socket, ssl, base64, struct, hashlib, os as _os
    from urllib.parse import urlparse

    p = urlparse(ws_url)
    host, port = p.hostname, p.port or (443 if p.scheme == "wss" else 80)
    s = socket.create_connection((host, port), timeout=timeout)
    if p.scheme == "wss":
        s = ssl.wrap_socket(s)

    # WebSocket handshake
    key = base64.b64encode(_os.urandom(16)).decode()
    req = (
        f"GET {p.path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        f"Upgrade: websocket\r\n"
        f"Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        f"Sec-WebSocket-Version: 13\r\n\r\n"
    )
    s.sendall(req.encode())
    # read handshake response
    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = s.recv(4096)
        if not chunk: break
        buf += chunk

    # send CDP message (text frame)
    payload = json.dumps({"id": msg_id, "method": method, "params": params or {}}).encode()
    frame = bytearray()
    frame.append(0x81)  # FIN + text
    mask_bit = 0x80
    plen = len(payload)
    if plen < 126:
        frame.append(mask_bit | plen)
    elif plen < 65536:
        frame.append(mask_bit | 126)
        frame += struct.pack(">H", plen)
    else:
        frame.append(mask_bit | 127)
        frame += struct.pack(">Q", plen)
    mask = _os.urandom(4)
    frame += mask
    frame += bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    s.sendall(frame)

    # read response frames until we get the matching id
    def recv_exactly(n):
        b = b""
        while len(b) < n:
            chunk = s.recv(n - len(b))
            if not chunk: raise IOError("conn closed")
            b += chunk
        return b

    deadline = time.time() + timeout
    while time.time() < deadline:
        h = recv_exactly(2)
        if not h: break
        b1, b2 = h[0], h[1]
        opcode = b1 & 0x0f
        masked = bool(b2 & 0x80)
        plen = b2 & 0x7f
        if plen == 126:
            plen = struct.unpack(">H", recv_exactly(2))[0]
        elif plen == 127:
            plen = struct.unpack(">Q", recv_exactly(8))[0]
        if masked:
            mask = recv_exactly(4)
        data = recv_exactly(plen)
        if masked:
            data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        if opcode == 0x1:  # text
            try:
                msg = json.loads(data.decode())
            except Exception:
                continue
            if msg.get("id") == msg_id:
                s.close()
                return msg
            # 忽略事件 (Page.frameNavigated 等)
        elif opcode == 0x8:  # close
            break
    s.close()
    raise TimeoutError(f"CDP {method} timed out")


# ============================================================
# Chromium launch + per-slide render
# ============================================================

def find_slides(deck_root: Path) -> list:
    sd = deck_root / "slides"
    return sorted(sd.glob("*.html"))


def start_http_server(deck_root: Path, port: int):
    """在 deck_root 起 http server"""
    os.chdir(deck_root)
    handler = http.server.SimpleHTTPRequestHandler
    # 禁日志
    handler.log_message = lambda *a, **k: None
    httpd = socketserver.TCPServer(("127.0.0.1", port), handler)
    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    return httpd


def start_chromium(debug_port: int, user_data_dir: Path):
    """启 chromium-headless · 开 remote-debugging-port"""
    cmd = [
        "chromium-browser",
        "--headless=new",
        "--no-sandbox",
        "--disable-gpu",
        "--disable-extensions",
        "--disable-default-apps",
        "--disable-component-extensions-with-background-pages",
        "--no-first-run",
        f"--user-data-dir={user_data_dir}",
        f"--remote-debugging-port={debug_port}",
        "--window-size=1920,1080",
        "--hide-scrollbars",
        "about:blank",
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    # 等 CDP 就绪 + 挑 about:blank tab(跳过 extension 后台页)
    deadline = time.time() + 15
    while time.time() < deadline:
        try:
            tabs = cdp_get(debug_port, "/json")
            for t in tabs:
                url = t.get("url", "")
                # 排除 chrome-extension / devtools 自身,只要可导航的 page
                if t.get("type") == "page" and not url.startswith("chrome-extension://") and not url.startswith("devtools://"):
                    return proc, t["webSocketDebuggerUrl"]
        except Exception:
            pass
        time.sleep(0.3)
    proc.terminate()
    raise RuntimeError("chromium CDP 未就绪 / 找不到可用 page tab")


def audit_one_slide(ws_url: str, slide_url: str, msg_id_start: int = 100) -> dict:
    """单 slide 渲染 + JS 注入 + 取结果"""
    mid = msg_id_start
    # 1) navigate
    cdp_send_ws(ws_url, "Page.navigate", {"url": slide_url}, msg_id=mid); mid += 1
    # 2) 轮询等 .slide DOM 出现 + computed style 稳定(最多 8 秒)
    deadline = time.time() + 8
    ready = False
    while time.time() < deadline:
        time.sleep(0.5)
        resp = cdp_send_ws(ws_url, "Runtime.evaluate", {
            "expression": "document.querySelector('.slide') ? document.querySelector('.slide').getBoundingClientRect().width : 0",
            "returnByValue": True,
        }, msg_id=mid, timeout=10); mid += 1
        v = resp.get("result", {}).get("result", {}).get("value", 0)
        if v and v > 100:  # .slide 已渲染且有宽度
            ready = True
            break
    if not ready:
        return {"error": ".slide DOM 未在 8s 内就绪"}
    # 3) Runtime.evaluate 注入 INSPECT_JS
    resp = cdp_send_ws(ws_url, "Runtime.evaluate", {
        "expression": INSPECT_JS,
        "returnByValue": True,
        "awaitPromise": False,
    }, msg_id=mid, timeout=20); mid += 1
    result = resp.get("result", {}).get("result", {})
    if result.get("type") == "string":
        try:
            return json.loads(result["value"])
        except Exception as e:
            return {"error": f"json parse fail: {e}", "raw": result.get("value", "")[:200]}
    return {"error": "no string result", "raw": str(result)[:200]}


# ============================================================
# Report
# ============================================================

def render_report(deck_root: Path, results: list) -> Path:
    audit_dir = deck_root / "_audit"
    audit_dir.mkdir(exist_ok=True)
    out = audit_dir / "render_audit.md"

    n = len(results)
    n_fail = sum(1 for r in results if any(r.get("findings", {}).get(k) for k in
                 ["overflow", "pagenum_collision", "logo_collision", "sub_floor_font"]))
    n_pass = n - n_fail

    lines = []
    lines.append(f"# Render Audit · 浏览器渲染审查\n")
    lines.append(f"- 渲染时间: {time.strftime('%Y-%m-%d %H:%M:%S')}")
    lines.append(f"- 总 slide 数: {n}")
    lines.append(f"- ✅ PASS: {n_pass}  /  ⚠️/❌ FAIL: {n_fail}")
    lines.append(f"- 检查项: 文本溢出 · 撞页码区 · 撞 logo 区 · 渲染后字号 < {FONT_FLOOR}px\n")
    lines.append("---\n")

    for r in results:
        slide = r["slide"]
        f = r.get("findings", {})
        if r.get("error"):
            lines.append(f"## ❌ {slide}\n")
            lines.append(f"- 错误: {r['error']}\n")
            continue
        clean = not any(f.get(k) for k in ["overflow", "pagenum_collision", "logo_collision", "sub_floor_font"])
        icon = "✅" if clean else "❌"
        lines.append(f"## {icon} {slide}\n")
        if clean:
            lines.append("- 无问题\n")
        else:
            if f.get("overflow"):
                lines.append(f"- ❌ **溢出 slide 边界 ({len(f['overflow'])} 处)**:")
                for it in f["overflow"][:3]:
                    lines.append(f"  - `{it['ctx']}` @ ({it['local']['l']},{it['local']['t']}) → ({it['local']['r']},{it['local']['b']}) · fs={it['fs']:.0f}")
            if f.get("pagenum_collision"):
                lines.append(f"- ⚠️ **撞页码禁区 right:0-220 bottom:0-80 ({len(f['pagenum_collision'])} 处)**:")
                for it in f["pagenum_collision"][:3]:
                    lines.append(f"  - `{it['ctx']}` @ ({it['local']['l']},{it['local']['t']}) → ({it['local']['r']},{it['local']['b']}) · fs={it['fs']:.0f}")
            if f.get("logo_collision"):
                lines.append(f"- ⚠️ **撞 logo 禁区 right:0-200 top:0-80 ({len(f['logo_collision'])} 处)**:")
                for it in f["logo_collision"][:3]:
                    lines.append(f"  - `{it['ctx']}` @ ({it['local']['l']},{it['local']['t']}) → ({it['local']['r']},{it['local']['b']}) · fs={it['fs']:.0f}")
            if f.get("sub_floor_font"):
                lines.append(f"- ❌ **渲染后字号 < {FONT_FLOOR}px ({len(f['sub_floor_font'])} 处)**:")
                for it in f["sub_floor_font"][:5]:
                    lines.append(f"  - `{it['ctx']}` · fs={it['fs']}px")
        lines.append("")

    out.write_text("\n".join(lines), encoding="utf-8")
    return out


# ============================================================
# Main
# ============================================================

def main():
    ap = argparse.ArgumentParser(description="h3c-ppt · browser-rendered audit")
    ap.add_argument("deck_root", help="HTML_Deck root directory")
    ap.add_argument("--port", type=int, default=18991, help="HTTP server port (default 18991)")
    ap.add_argument("--debug-port", type=int, default=19222, help="chromium CDP port (default 19222)")
    args = ap.parse_args()

    deck_root = Path(args.deck_root).resolve()
    if not (deck_root / "slides").is_dir() or not (deck_root / "index.html").is_file():
        print(f"❌ {deck_root} 不像 HTML_Deck 根", file=sys.stderr); sys.exit(2)

    slides = find_slides(deck_root)
    if not slides:
        print(f"❌ 无 slides", file=sys.stderr); sys.exit(2)
    print(f"📊 待审 {len(slides)} 张 slides")

    # 1) HTTP server(snap chromium 不能直读 file://)
    httpd = start_http_server(deck_root, args.port)
    print(f"🌐 HTTP server: http://127.0.0.1:{args.port}")

    # 2) chromium-headless · snap 沙盒能写的临时 dir
    snap_tmp = Path.home() / "snap" / "chromium" / "common" / "h3c-audit"
    snap_tmp.mkdir(parents=True, exist_ok=True)
    user_data_dir = Path(tempfile.mkdtemp(prefix="cdp-", dir=snap_tmp))
    print(f"🚀 启 chromium · CDP={args.debug_port}")

    chromium_proc = None
    try:
        chromium_proc, ws_url = start_chromium(args.debug_port, user_data_dir)

        results = []
        for i, slide_path in enumerate(slides, 1):
            slide_url = f"http://127.0.0.1:{args.port}/slides/{slide_path.name}"
            print(f"  [{i}/{len(slides)}] {slide_path.name}", end=" ... ", flush=True)
            try:
                findings = audit_one_slide(ws_url, slide_url)
                results.append({"slide": slide_path.name, "findings": findings})
                if findings.get("error"):
                    print(f"❌ {findings['error']}")
                else:
                    n_iss = sum(len(findings.get(k, [])) for k in ["overflow", "pagenum_collision", "logo_collision", "sub_floor_font"])
                    print("✅" if n_iss == 0 else f"⚠️ {n_iss} issues")
            except Exception as e:
                results.append({"slide": slide_path.name, "error": str(e)})
                print(f"❌ {e}")

        # 3) report
        report = render_report(deck_root, results)
        print(f"\n📄 报告: {report}")

        # exit code
        n_fail = sum(1 for r in results
                     if r.get("error") or
                     any(r.get("findings", {}).get(k) for k in
                         ["overflow", "pagenum_collision", "logo_collision", "sub_floor_font"]))
        sys.exit(0 if n_fail == 0 else (2 if any(r.get("findings", {}).get("overflow") or r.get("findings", {}).get("sub_floor_font") for r in results) else 1))
    finally:
        if chromium_proc:
            chromium_proc.terminate()
            try: chromium_proc.wait(timeout=5)
            except: chromium_proc.kill()
        httpd.shutdown()
        try: shutil.rmtree(user_data_dir, ignore_errors=True)
        except: pass


if __name__ == "__main__":
    main()
