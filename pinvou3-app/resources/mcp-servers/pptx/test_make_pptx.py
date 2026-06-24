# -*- coding: utf-8 -*-
"""
pptx 工具(make_pptx)测试用例 —— 覆盖本次 PR 的可确定性核心逻辑。

跑法(用自带 python-pptx 的解释器):
    python test_make_pptx.py
全过 → 退出码 0;有失败 → 打印 [FAIL] 明细并退出码 1。

覆盖范围(对应本次 PR 的功能点):
  1. 主题名中→英归一(_resolve_theme):中文/英文/大小写/长串/未知兜底
  2. make_pptx 基本:生成合法 .pptx、可被 python-pptx 打开、页数对
  3. 落盘位置:写进会话产物目录(PINVOU3_SESSION_ARTIFACTS),绝不写进程当前目录
  4. 9 套主题全部能渲染
  5. 封面缩略图注入:.pptx(zip)里有 docProps/thumbnail.jpeg(产物卡封面靠它)
  6. 10 种 layout 全渲染、页数对、文件合法
  7. slides 传成 JSON 字符串也兼容(模型有时这么传)
  8. 回归:不再产生调试落盘文件 ~/_make_pptx_debug.json
  9. MCP 协议冒烟(子进程):initialize / tools/list / tools/call(make_pptx)

注意:测试把 PINVOU3_SESSION_ARTIFACTS 指向临时目录,不污染 ~/.pinvou3。
"""
import json
import os
import subprocess
import sys
import tempfile
import zipfile

# ── 所有产物落到临时目录,别污染真实 ~/.pinvou3 ──────────────────────────
_TMP = tempfile.mkdtemp(prefix="pptx_test_")
os.environ["PINVOU3_SESSION_ARTIFACTS"] = _TMP

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _HERE)
import server  # noqa: E402  (要先设 env / sys.path)

from pptx import Presentation  # noqa: E402  (校验产物合法性)

# ── 迷你测试框架 ─────────────────────────────────────────────────────────
_RESULTS = []


def case(name):
    def deco(fn):
        def run():
            try:
                fn()
                _RESULTS.append((name, True, ""))
                print("[PASS] %s" % name)
            except AssertionError as e:
                _RESULTS.append((name, False, str(e)))
                print("[FAIL] %s -> %s" % (name, e))
            except Exception as e:
                _RESULTS.append((name, False, "异常: %s" % e))
                print("[FAIL] %s -> 异常: %s" % (name, e))
        run._is_case = True
        return run
    return deco


def _deck(*slides):
    return list(slides)


# ── 1. 主题名归一 ────────────────────────────────────────────────────────
@case("1. 主题名中→英归一 _resolve_theme")
def test_resolve_theme():
    cases = {
        "business-blue": "business-blue",
        "商务蓝": "business-blue",
        "商务": "business-blue",
        "科技": "tech-dark",
        "TECH-DARK": "tech-dark",          # 大小写容错
        "政务红": "gov-red",
        "创意紫色风格": "creative-purple",  # 长串包含匹配
        "金融年报": "navy-gold",
        "极简": "minimal-mono",
        "午夜蓝": "midnight",
        "深蓝": "midnight",                # 精确别名优先于「蓝」
        "green": "fresh-green",
        "xyz不存在": "business-blue",       # 未知兜底默认
        "": "business-blue",
        None: "business-blue",
    }
    for inp, want in cases.items():
        got = server._resolve_theme(inp)
        assert got == want, "_resolve_theme(%r) = %r,期望 %r" % (inp, got, want)
    # 9 套主题的 key 都应能被自身解析
    for key in server.THEMES:
        assert server._resolve_theme(key) == key, "主题 key %r 应原样解析" % key


# ── 2. 基本生成 + 合法性 ─────────────────────────────────────────────────
@case("2. make_pptx 生成合法 .pptx、页数对")
def test_make_pptx_basic():
    deck = _deck(
        {"layout": "cover", "title": "测试报告", "subtitle": "子标题", "date": "2026-06-22"},
        {"layout": "bullets", "title": "要点", "bullets": ["第一点", "第二点", "第三点"]},
    )
    r = server.make_pptx(theme="business-blue", slides=deck, filename="test_basic")
    assert r.get("ok") is True, "返回应 ok=True,实际 %r" % r
    assert os.path.isfile(r["path"]), "产物文件应存在: %s" % r["path"]
    assert r["slides"] == 2, "页数应为 2,实际 %r" % r["slides"]
    prs = Presentation(r["path"])  # 能打开 = 合法 pptx
    assert len(prs.slides) == 2, "python-pptx 读到的页数应为 2,实际 %d" % len(prs.slides)


# ── 3. 落盘位置(回归本次修复:绝不写 getcwd/app 目录)──────────────────
@case("3. 落盘进会话产物目录,不写进程当前目录")
def test_output_location():
    r = server.make_pptx(theme="business-blue",
                         slides=[{"layout": "cover", "title": "位置测试"}],
                         filename="test_loc")
    p = os.path.abspath(r["path"])
    assert p.startswith(os.path.abspath(_TMP)), "应落在产物目录 %s 下,实际 %s" % (_TMP, p)
    cwd = os.path.abspath(os.getcwd())
    assert not p.startswith(cwd) or _TMP.startswith(cwd), "不应落进进程当前目录: %s" % p
    assert "target" not in p and "Program Files" not in p, "不应落进构建/安装目录: %s" % p


# ── 4. 9 套主题全渲染 ────────────────────────────────────────────────────
@case("4. 9 套主题全部能渲染")
def test_all_themes():
    deck = [{"layout": "cover", "title": "主题测试"},
            {"layout": "kpi", "title": "数据", "items": [{"num": "99%", "label": "满意度"}]}]
    for i, theme in enumerate(server.THEMES):
        r = server.make_pptx(theme=theme, slides=deck, filename="test_theme_%d" % i)
        assert r.get("ok") is True, "主题 %s 渲染失败: %r" % (theme, r)
        assert r.get("theme") == theme, "主题应为 %s,实际 %r" % (theme, r.get("theme"))


# ── 5. 封面缩略图注入(产物卡封面靠它)──────────────────────────────────
@case("5. 封面缩略图 docProps/thumbnail.jpeg 注入")
def test_cover_thumbnail():
    r = server.make_pptx(theme="navy-gold",
                         slides=[{"layout": "cover", "title": "封面缩略图测试", "subtitle": "x"}],
                         filename="test_thumb")
    with zipfile.ZipFile(r["path"]) as z:
        names = z.namelist()
    assert "docProps/thumbnail.jpeg" in names, "pptx 里应有 docProps/thumbnail.jpeg,实际: %s" % names


# ── 6. 10 种 layout 全渲染 ──────────────────────────────────────────────
@case("6. 10 种 layout 全部渲染、页数对")
def test_all_layouts():
    deck = [
        {"layout": "cover", "title": "封面", "subtitle": "副", "date": "2026"},
        {"layout": "agenda", "title": "目录", "items": ["一", "二", "三"]},
        {"layout": "section", "title": "章节页"},
        {"layout": "bullets", "title": "要点", "bullets": ["a", "b"]},
        {"layout": "two_col", "title": "双栏", "leftTitle": "左", "left": ["L1"],
         "rightTitle": "右", "right": ["R1"]},
        {"layout": "kpi", "title": "指标", "items": [{"num": "100", "label": "x"},
                                                    {"num": "50%", "label": "y"}]},
        {"layout": "chart", "title": "图表", "chartType": "bar",
         "categories": ["Q1", "Q2", "Q3"], "series": [{"name": "营收", "data": [1, 2, 3]}]},
        {"layout": "table", "title": "表格", "headers": ["列1", "列2"],
         "rows": [["a", "b"], ["c", "d"]]},
        {"layout": "image", "title": "图片页", "bullets": ["说明"], "caption": "图注"},
        {"layout": "end", "title": "谢谢", "subtitle": "Q&A"},
    ]
    r = server.make_pptx(theme="creative-purple", slides=deck, filename="test_layouts")
    assert r.get("ok") is True, "全 layout 渲染失败: %r" % r
    assert r["slides"] == 10, "页数应为 10,实际 %r" % r["slides"]
    prs = Presentation(r["path"])
    assert len(prs.slides) == 10, "读到页数应为 10,实际 %d" % len(prs.slides)


# ── 7. slides 传成 JSON 字符串也兼容 ────────────────────────────────────
@case("7. slides 为 JSON 字符串时兼容")
def test_slides_json_string():
    deck = [{"layout": "cover", "title": "字符串入参"},
            {"layout": "bullets", "title": "页2", "bullets": ["x"]}]
    r = server.make_pptx(theme="business-blue", slides=json.dumps(deck, ensure_ascii=False),
                         filename="test_strslides")
    assert r.get("ok") is True, "JSON 字符串 slides 应被兼容: %r" % r
    assert r["slides"] == 2, "页数应为 2,实际 %r" % r["slides"]


# ── 8. 回归:不再产生调试落盘文件 ───────────────────────────────────────
@case("8. 回归:不产生 ~/_make_pptx_debug.json")
def test_no_debug_dump():
    debug = os.path.join(os.path.expanduser("~"), "_make_pptx_debug.json")
    if os.path.exists(debug):
        os.remove(debug)  # 清掉旧版可能残留的,确保是干净判断
    server.make_pptx(theme="business-blue",
                     slides=[{"layout": "cover", "title": "debug 回归"}],
                     filename="test_nodebug")
    assert not os.path.exists(debug), "make_pptx 不应再写调试文件 %s" % debug


# ── 9. MCP 协议冒烟(子进程)─────────────────────────────────────────────
@case("9. MCP 协议:initialize / tools/list / tools/call")
def test_mcp_protocol():
    reqs = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
         "params": {"name": "make_pptx",
                    "arguments": {"theme": "tech-dark", "filename": "test_mcp",
                                  "slides": [{"layout": "cover", "title": "MCP 测试"}]}}},
    ]
    stdin = "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in reqs)
    env = dict(os.environ)
    proc = subprocess.run(
        [sys.executable, os.path.join(_HERE, "server.py")],
        input=stdin.encode("utf-8"),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        timeout=60,
    )
    lines = [l for l in proc.stdout.decode("utf-8").splitlines() if l.strip()]
    resp = {}
    for l in lines:
        try:
            o = json.loads(l)
            resp[o.get("id")] = o
        except Exception:
            pass
    # initialize
    assert resp.get(1, {}).get("result", {}).get("serverInfo", {}).get("name") == "pinvou3-pptx", \
        "initialize 应返回 serverInfo.name=pinvou3-pptx,实际 %r" % resp.get(1)
    # tools/list 含 make_pptx
    tools = resp.get(2, {}).get("result", {}).get("tools", [])
    assert any(t.get("name") == "make_pptx" for t in tools), "tools/list 应含 make_pptx,实际 %r" % tools
    # tools/call 返回 ok 的产物
    content = resp.get(3, {}).get("result", {}).get("content", [])
    assert content, "tools/call 应有 content,实际 %r" % resp.get(3)
    payload = json.loads(content[0]["text"])
    assert payload.get("ok") is True and os.path.isfile(payload["path"]), \
        "tools/call(make_pptx) 应生成文件,实际 %r" % payload


# ── 10. 非法 JSON 抢救 + 彻底空报错(本次修复:模型大纲一长常生成坏 JSON)──
@case("10. 非法 JSON 抢救 / 空则 ok:False")
def test_malformed_json_salvage():
    # 模型常见错:某个对象提前闭合,后面字段悬空在对象外 → 严格 json.loads 失败
    bad = ('[{"layout":"cover","title":"标题页"},'
           '{"layout":"two_col","title":"双栏","leftTitle":"左","left":["L1"]},'  # 提前闭合
           '"rightTitle":"右","right":["R1"]},'                                    # 悬空(非法)
           '{"layout":"bullets","title":"要点","bullets":["a","b"]},'
           '{"layout":"end","title":"谢谢"}]')
    import json as _json
    try:
        _json.loads(bad)
        assert False, "测试数据应该是非法 JSON"
    except ValueError:
        pass
    r = server.make_pptx(theme="business-blue", slides=bad, filename="test_salvage")
    assert r.get("ok") is True, "非法 JSON 应被抢救而非失败: %r" % r
    # 4 个合法顶层对象应被救回(cover/two_col/bullets/end),坏的悬空片段跳过
    assert r["slides"] >= 4, "应至少抢救出 4 页,实际 %r" % r["slides"]
    # 彻底空 / 无法解析 → ok:False(loud error,不静默出空 PPT)
    for empty in ["", "[]", "不是json也不是数组", None]:
        r2 = server.make_pptx(theme="business-blue", slides=empty, filename="test_empty")
        assert r2.get("ok") is False and r2.get("error"), \
            "空/无法解析的 slides 应返回 ok:False+error,实际 %r(输入 %r)" % (r2, empty)


# ── 跑全部 ───────────────────────────────────────────────────────────────
def _main():
    cases = [v for v in list(globals().values()) if callable(v) and getattr(v, "_is_case", False)]
    for c in cases:
        c()
    passed = sum(1 for _, ok, _ in _RESULTS if ok)
    total = len(_RESULTS)
    print("\n==== 结果: %d/%d 通过 ====" % (passed, total))
    if passed != total:
        for name, ok, msg in _RESULTS:
            if not ok:
                print("  FAIL: %s -> %s" % (name, msg))
        sys.exit(1)
    print("产物输出目录(临时): %s" % _TMP)
    sys.exit(0)


if __name__ == "__main__":
    _main()
