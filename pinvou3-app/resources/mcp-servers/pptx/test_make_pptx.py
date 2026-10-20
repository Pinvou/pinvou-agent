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
  10. malformed-JSON salvage / fully empty loud error
  11. slides inputSchema.items structure pinned (arrays must carry concretely typed items)
  12. explicit-null optional fields are safe (DeepSeek strict mode rewrites every property required+nullable)
  13. kpi numeric 0 / agenda dict items (allowed by the schema anyOf) render as data, not swallowed by `or`
  14. null array elements (salvage path bypasses the schema) fall back instead of leaking str(None) or error pages
  15. non-string salvage elements are dropped (reprs never render, no phantom ordinals)

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


# ── 11. slides schema must carry structured items (regression: k3 once completed an itemless array to []) ──
@case("11. slides inputSchema.items structure pinned (arrays must carry concretely typed items)")
def test_slides_schema_items_pin():
    schema = server.TOOL_DEFS[0]["inputSchema"]["properties"]["slides"]
    assert schema.get("type") == "array", "slides must be an array, got %r" % schema.get("type")
    items = schema.get("items")
    assert isinstance(items, dict) and items, "slides must carry non-empty items (Kimi-channel decoders complete an itemless array to []), got %r" % items
    assert items.get("type") == "object", "items must be an object, got %r" % items.get("type")
    assert items.get("required") == ["layout"], "items.required must be ['layout'], got %r" % items.get("required")
    # The layout enum must match the LAYOUTS render table exactly (a new
    # layout added without updating the enum must turn red here).
    props = items.get("properties") or {}
    assert set(props.get("layout", {}).get("enum") or []) == set(server.LAYOUTS), \
        "layout.enum must match LAYOUTS exactly, got %r vs %r" % (props.get("layout", {}).get("enum"), sorted(server.LAYOUTS))
    # The chartType enum must match the _CHART_TYPES render table exactly.
    assert set(props.get("chartType", {}).get("enum") or []) == set(server._CHART_TYPES), \
        "chartType.enum must match _CHART_TYPES exactly, got %r vs %r" % (props.get("chartType", {}).get("enum"), sorted(server._CHART_TYPES))
    # Property-name set pinned: a dropped field (the model loses that field's
    # guidance) and an unregistered extra field must both turn red.
    expected = {"layout", "title", "subtitle", "date", "items", "bullets", "leftTitle", "left",
                "rightTitle", "right", "chartType", "categories", "series", "headers", "rows",
                "caption", "image"}
    assert set(props) == expected, \
        "slides.items.properties must be exactly the %d declared fields, diff %r" % (len(expected), set(props) ^ expected)
    # Every nested array must carry concretely typed items (guard class:
    # reintroducing an itemless array must turn red here).
    def _assert_typed(node, path):
        if node.get("type") != "array":
            return
        sub = node.get("items")
        assert isinstance(sub, dict) and sub, "%s is missing items" % path
        if sub.get("type") in ("array", "object", "string", "number"):
            if sub.get("type") == "array":
                _assert_typed(sub, path + ".items")
        elif "anyOf" in sub:
            for i, br in enumerate(sub["anyOf"]):
                assert br.get("type"), "%s.anyOf[%d] lacks a concrete type" % (path, i)
        else:
            assert False, "%s.items lacks a concrete type: %r" % (path, sub.get("type"))
    for key in ("bullets", "left", "right", "categories", "headers", "rows", "items", "series"):
        _assert_typed(props[key], "slides.items.properties.%s" % key)
    # series.data's numeric array and rows' 2-D array are the two deepest
    # spots; pin their shapes separately. name stays optional: single-series
    # charts hide the legend, so a forced invented name is friction, and the
    # renderer falls back to "" either way.
    series_items = props["series"]["items"]
    assert series_items.get("required") == ["data"], "series items.required must be ['data'], got %r" % series_items.get("required")
    assert series_items["properties"]["data"]["items"] == {"type": "number"}, "series.data items must be number"
    assert props["rows"]["items"]["type"] == "array" and props["rows"]["items"]["items"] == {"type": "string"}, \
        "rows must be a 2-D string array, got %r" % props["rows"]
    # The items anyOf dict branch must not require num: agenda pages draw
    # their own ordinals, so a label-only dict is a legal entry.
    items_anyof = props["items"]["items"]["anyOf"]
    dict_branch = next(br for br in items_anyof if br.get("type") == "object")
    assert "required" not in dict_branch, "items dict branch must not require num, got %r" % dict_branch.get("required")


# ── 12. explicit-null optional-field safety (DeepSeek strict rewrites every
#        property required+nullable, so the model explicitly sends
#        "field": null for unused fields; the renderer must fall back to
#        defaults instead of blowing up the page) ──
@case("12. explicit-null optional fields render with no error placeholder pages and no literal None")
def test_explicit_null_fields_render():
    deck = _deck(
        {"layout": "cover", "title": None, "subtitle": None, "date": None},
        {"layout": "agenda", "title": None, "items": None},
        {"layout": "section", "title": None},
        {"layout": "bullets", "title": None, "bullets": None},
        {"layout": "two_col", "title": None, "leftTitle": None, "left": None,
         "rightTitle": None, "right": None},
        {"layout": "kpi", "title": None, "items": [{"num": None, "label": None}, "68%"]},
        {"layout": "chart", "title": None, "chartType": None, "categories": None, "series": None},
        {"layout": "table", "title": None, "headers": None, "rows": None},
        {"layout": "image", "title": None, "bullets": None, "caption": None, "image": None},
        {"layout": "end", "title": None, "subtitle": None},
    )
    r = server.make_pptx(theme="business-blue", slides=deck, filename="test_null_fields")
    assert r.get("ok") is True, "all-null optional fields must render ok=True, got %r" % r
    assert r["slides"] == 10, "slide count must be 10, got %r" % r["slides"]
    prs = Presentation(r["path"])
    texts = [shape.text_frame.text
             for slide in prs.slides for shape in slide.shapes if shape.has_text_frame]
    joined = "\n".join(texts)
    assert "本页渲染失败" not in joined, "no page may render as a failure: %s" % joined[:300]
    assert "None" not in joined, "null must not render as a literal None: %r" % [t for t in texts if "None" in t]
    # A null title must fall back to each layout's default title; a kpi
    # string element (allowed by the schema anyOf) renders as the big number.
    assert "未命名演示" in joined and "谢谢" in joined, "cover/end null titles must fall back to the default titles"
    assert "68%" in joined, "kpi string elements must render as the big number"


# ── 13. numeric 0 and dict-form items: the items anyOf allows {num,label},
#        and num/label fall back on None only — a numeric 0 is valid data
#        that the `or` idiom would swallow into a blank card; an agenda dict
#        element must not render as a dict repr either. ──
@case("13. kpi numeric 0 and agenda dict items render as data")
def test_falsy_and_dict_items_render():
    deck = _deck(
        {"layout": "kpi", "title": "指标",
         "items": [{"num": 0, "label": "增长"}, {"num": 5, "label": None}]},
        {"layout": "agenda", "title": None,
         "items": [{"num": "68%", "label": "满意度"}, "前言"]},
    )
    r = server.make_pptx(theme="business-blue", slides=deck, filename="test_falsy_items")
    assert r.get("ok") is True, "numeric 0 / dict items must render ok=True, got %r" % r
    assert r["slides"] == 2, "slide count must be 2, got %r" % r["slides"]
    prs = Presentation(r["path"])
    texts = [shape.text_frame.text
             for slide in prs.slides for shape in slide.shapes if shape.has_text_frame]
    joined = "\n".join(texts)
    assert "本页渲染失败" not in joined, "no page may render as a failure: %s" % joined[:300]
    # A numeric 0 must render as its own "0" text box (agenda ordinals are
    # "01"/"02", which do not count).
    assert "0" in texts, "kpi numeric 0 must not be swallowed by `or` into a blank card, got text frames %r" % texts
    assert "5" in texts, "kpi numeric 5 must render normally, got text frames %r" % texts
    # An agenda dict element renders as "num label", never a dict repr.
    assert "满意度" in joined and "68%" in joined, "agenda dict items must render num/label: %r" % texts
    # Pin the exact rendered pair instead of a brace/apostrophe ban (a
    # legitimate fixture could contain either); the dict repr would appear
    # verbatim as "{'num': '68%', 'label': '满意度'}".
    assert "68% 满意度" in joined, "agenda dict element must render as its num label pair, got %r" % texts
    assert "{'num'" not in joined, "agenda must not render a dict repr: %r" % texts
    assert "前言" in joined, "agenda string elements must render verbatim"


# ── 14. null array elements: schema-illegal but the JSON-string salvage path
#        bypasses schema validation, so a null element must neither leak
#        str(None) into a slide nor crash into the per-page error guard. ──
@case("14. null array elements render as fallbacks, never None or error pages")
def test_null_array_elements_are_safe():
    deck = _deck(
        {"layout": "bullets", "title": "空要点", "bullets": ["第一", None]},
        {"layout": "kpi", "title": "空指标",
         "items": [None, {"num": "1", "label": "存活"}]},
        {"layout": "agenda", "title": None, "items": [None, {"label": "只标签"}, "文字"]},
        {"layout": "table", "title": "空表格", "headers": [None, "表头"], "rows": [None, ["格子"]]},
        {"layout": "chart", "title": "空图表", "chartType": "bar",
         "categories": ["Q1"], "series": [None, {"name": "系列", "data": [1]}]},
    )
    r = server.make_pptx(theme="business-blue", slides=deck, filename="test_null_elements")
    assert r.get("ok") is True, "null elements must render ok=True, got %r" % r
    assert r["slides"] == 5, "slide count must be 5, got %r" % r["slides"]
    prs = Presentation(r["path"])
    texts = [shape.text_frame.text
             for slide in prs.slides for shape in slide.shapes if shape.has_text_frame]
    cells = [cell.text
             for slide in prs.slides for shape in slide.shapes
             if getattr(shape, "has_table", False) and shape.has_table
             for row in shape.table.rows for cell in row.cells]
    joined = "\n".join(texts + cells)
    assert "本页渲染失败" not in joined, "no page may render as a failure: %s" % joined[:300]
    assert "None" not in joined, "str(None) must never reach a slide, got %r" % (texts + cells)
    # The surviving sibling elements must still render (table content lives
    # in table cells; the chart's surviving series count is read off the
    # chart object — neither appears in plain text frames).
    for needle in ("第一", "存活", "只标签", "文字", "格子", "表头"):
        assert needle in joined, "element %r must survive its null siblings, got %r" % (needle, joined[:300])
    charts = [shape.chart for slide in prs.slides for shape in slide.shapes
              if getattr(shape, "has_chart", False) and shape.has_chart]
    assert len(charts) == 1, "exactly one chart must render, got %r" % len(charts)
    assert len(charts[0].plots[0].series) == 1, "the null series must be dropped, leaving 1, got %r" % len(charts[0].plots[0].series)


# ── 15. non-string salvage garbage: the JSON-string salvage path bypasses
#        schema validation, so any element type can arrive. Every element
#        lane drops what it cannot render (kept in lockstep with the
#        series/rows filters): string items where strings are expected,
#        (str, dict) items in the shared items anyOf — dropped before the
#        agenda ordinals are numbered, and string-only chart categories —
#        with every series dropping the same positions as the labels. ──
@case("15. non-string salvage elements are dropped, never rendered as reprs or phantom slots")
def test_non_string_salvage_elements_are_dropped():
    deck = _deck(
        {"layout": "bullets", "title": "标量要点", "bullets": ["文字", True, 3.5, ["嵌"], {"k": "v"}]},
        {"layout": "two_col", "title": "两栏", "leftTitle": "左",
         "left": {"a": 1}, "right": ["右", None, 2]},
        {"layout": "agenda", "title": None, "items": "not-a-list"},
        {"layout": "agenda", "title": None, "items": [None, "唯一", ["嵌"], 7]},
        {"layout": "kpi", "title": "指标", "items": [["嵌"], {"num": "1", "label": "存活"}]},
        {"layout": "chart", "title": "空图表", "chartType": "bar",
         "categories": ["Q1", None, "Q3"], "series": [{"name": "系列", "data": [10, 20, 30]}]},
    )
    r = server.make_pptx(theme="business-blue", slides=deck, filename="test_salvage_elements")
    assert r.get("ok") is True, "salvage garbage must render ok=True, got %r" % r
    assert r["slides"] == 6, "slide count must be 6, got %r" % r["slides"]
    prs = Presentation(r["path"])
    texts = [shape.text_frame.text
             for slide in prs.slides for shape in slide.shapes if shape.has_text_frame]
    joined = "\n".join(texts)
    assert "本页渲染失败" not in joined, "no page may render as a failure: %s" % joined[:300]
    for needle in ("True", "3.5", "['嵌']", "{'k': 'v'}", "{'a': 1}", "not-a-list", "None"):
        assert needle not in joined, "salvage repr %r must never reach a slide, got %r" % (needle, joined[:400])
    # Surviving real content must still render.
    for needle in ("文字", "右", "唯一", "存活"):
        assert needle in joined, "element %r must survive its garbage siblings, got %r" % (needle, joined[:400])
    # A string-valued items renders nothing (no per-character ordinals), and
    # garbage elements must not consume agenda ordinals: "唯一" is the only
    # item on its page, so exactly one ordinal ("01") may exist there.
    agenda_page = prs.slides[3]
    agenda_texts = [shape.text_frame.text for shape in agenda_page.shapes if shape.has_text_frame]
    ordinals = [t for t in agenda_texts if t in ("01", "02", "03", "04", "05", "06")]
    assert ordinals == ["01"], "dropped elements must not consume agenda ordinals, got %r" % agenda_texts
    # Null/garbage categories are dropped from the chart axis, and every
    # series drops the SAME positions its labels drop: Q3 must keep its own
    # value (30), not the shifted 20 — read labels and values back together
    # off the saved file (already-shortened data cannot catch the shift).
    charts = [shape.chart for slide in prs.slides for shape in slide.shapes
              if getattr(shape, "has_chart", False) and shape.has_chart]
    assert len(charts) == 1, "exactly one chart must render, got %r" % len(charts)
    plot = charts[0].plots[0]
    got_categories = list(plot.categories)
    assert got_categories == ["Q1", "Q3"], "null categories must be dropped, got %r" % got_categories
    got_values = [float(v) for v in plot.series[0].values]
    assert got_values == [10.0, 30.0], \
        "series values must drop the dropped label positions, got %r" % got_values


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
