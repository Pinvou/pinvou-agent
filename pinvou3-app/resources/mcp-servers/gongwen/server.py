#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""pinvou3 公文套版 MCP server。

两个工具:
  make_gongwen(gongwen, filename) —— 结构化公文 JSON → GB/T 9704 .docx(落产物目录)。
  validate_gongwen(gongwen)       —— 立账核账:按党政机关公文规范逐项查,返回问题清单。

写作(文种/话术/序号)由 skill 指导模型产出进 JSON;此 server 只做确定性套版与校验,
一个字不改写内容。渲染细节见同目录 gbt9704_styles.py。
"""
import sys, os, io, json, re

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gbt9704_styles  # noqa: E402

# Windows 默认 stdout/stdin 是 GBK;MCP 协议要求 UTF-8 且 stdout 只能有 JSON-RPC。
if hasattr(sys.stdout, "buffer"):
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")


# ── 工具实现 ────────────────────────────────────────────────────────────────
def _coerce_list(v):
    """正文/附件 可能被模型序列化成 JSON 字符串,解析回 list。"""
    if isinstance(v, str):
        try:
            v = json.loads(v)
        except Exception:
            return []
    return v if isinstance(v, list) else []


# 对外 schema 用英文 key(见 _GONGWEN_SCHEMA),进来后映射回中文内部 key。
# 工具表里若出现唯一中文 key/全角符号的 schema,会污染 Qwen3.6 mtp 的 tool_call
# 格式参照、把别的工具(write_file)采歪成裸文本——这条漂因 warmup 堵不住(预热的是
# KV cache 冷热,改不了工具表内容)。所以 schema 全英文 key,翻译留在这层。
_EN2CN = {
    "doc_type": "文种", "issuer": "发文机关", "doc_number": "发文字号", "title": "标题",
    "recipient": "主送机关", "main_recipient": "主送机关", "body": "正文",
    "signer": "落款机关", "date": "成文日期", "disclosure": "公开方式",
    "print_note": "印发说明", "attachments": "附件",
    "level": "级别", "text": "文字",
}


def _remap_keys(obj):
    """英文 key → 中文内部 key(递归);中文 key 原样保留(belt:模型若仍产中文也认)。幂等。"""
    if isinstance(obj, dict):
        return {_EN2CN.get(k, k): _remap_keys(v) for k, v in obj.items()}
    if isinstance(obj, list):
        return [_remap_keys(x) for x in obj]
    return obj


def _auto_split_attachment(doc):
    """印发/转发型主件(办法/规定/规划等)规整,两种错位都纠:
       ① 主件塞进 正文 而非 附件 → 拆进 附件,壳话术(承启句)留 正文;
       ② 主件同时塞进 正文 和 附件(模型重复)→ 剥掉 正文里的主件,只留壳话术,
          防双份渲染(真机实测过的坑)。
       锚点 = 正文里第一个『主件起始』block(文字以 一、/第N章 起,或 == 主件名);
       非印发型、正文无主件(纯壳话术)、主件在最前(无壳话术)时不动。
       注:广州印发件标题不加书名号,故主件名按『印发…的通知』提取。"""
    title = (doc.get("标题") or "").strip()
    if "印发" not in title and "转发" not in title:
        return doc
    body = doc.get("正文") or []
    m = re.search(r"(?:印发|转发)《?(.+?)》?的(?:通知|函|意见|批复|报告|命令|公告)", title)
    main_name = m.group(1).strip().strip("《》") if m else None

    def _is_main_start(b):
        # 纯看文字前缀(不信模型标的 级别,它常打错):一、/第N章 是法规体例起始;或独立主件名行
        t = (b.get("文字") or "").strip().strip("《》")
        return (bool(re.match(r"^[一二三四五六七八九十]+、", t))
                or bool(re.match(r"^第[一二三四五六七八九十百]+[章节]", t))
                or (main_name and t == main_name))

    cut = next((i for i, b in enumerate(body) if _is_main_start(b)), None)
    if cut:  # cut>0:前面是壳话术,cut 起是混入/待拆的主件(cut=0/None 不动)
        shell, main_blocks = body[:cut], body[cut:]
        doc["正文"] = shell
        if not doc.get("附件"):
            # 主件只在正文 → 拆进附件(首块若就是主件标题,提为附件标题不重复)
            first = (main_blocks[0].get("文字") or "").strip().strip("《》") if main_blocks else ""
            if main_name and first == main_name:
                doc["附件"] = [{"标题": main_name, "正文": main_blocks[1:]}]
            else:
                doc["附件"] = [{"标题": main_name or first, "正文": main_blocks}]
        # else: 附件已有主件 → 正文剥离的主件直接丢弃(去重)

    # 附件内再去一层重:附件正文首块若与附件标题同名(模型常多写一行标题),删掉
    for a in (doc.get("附件") or []):
        ab = a.get("正文") or []
        at = (a.get("标题") or "").strip().strip("《》")
        if ab and (ab[0].get("文字") or "").strip().strip("《》") == at:
            a["正文"] = ab[1:]
    return doc


def _normalize_doc(filename, fields):
    """统一取出公文 dict(中文内部 key)。兼容:
       ① 英文 key 平铺(匹配 inputSchema,模型默认这么调:doc_type=…、title=…、body=[…]);
       ② 中文 key 平铺 / 整体裹进 gongwen=（dict 或 JSON 字符串）——均 belt 兼容。
       并把 body / attachments[].body 里被序列化成字符串的数组解析回 list。"""
    if list(fields.keys()) == ["gongwen"]:
        doc = fields["gongwen"]
        if isinstance(doc, str):
            doc = json.loads(doc)
    else:
        doc = dict(fields)
    if not isinstance(doc, dict):
        raise ValueError("公文参数必须是对象")
    doc = _remap_keys(doc)  # 英文 key → 中文(顶层 + 已是 list 的 block)
    doc["正文"] = [_remap_keys(b) for b in _coerce_list(doc.get("正文"))]
    atts = []
    for a in _coerce_list(doc.get("附件")):
        a = _remap_keys(a) if isinstance(a, dict) else a
        if isinstance(a, dict):
            a["正文"] = [_remap_keys(b) for b in _coerce_list(a.get("正文"))]
        atts.append(a)
    doc["附件"] = atts
    return _auto_split_attachment(doc)


def _sanitize_filename(name):
    name = (name or "公文").strip() or "公文"
    return re.sub(r'[\\/:*?"<>|\r\n]+', "_", name)[:80]


def _unique_path(d, base, ext=".docx"):
    """文件名碰撞时自动加 (2)(3)…,避免跨会话同标题相互覆盖丢件。"""
    p = os.path.join(d, base + ext)
    n = 2
    while os.path.exists(p):
        p = os.path.join(d, "%s (%d)%s" % (base, n, ext))
        n += 1
    return p


def _artifacts_dir():
    # 落盘:优先 app 注入的会话产物目录;否则 ~/.pinvou3 默认。绝不用 cwd(release 下是程序目录,不可写)。
    default = os.path.join(os.path.expanduser("~"), ".pinvou3", "sessions", "default", "artifacts")
    art = os.environ.get("PINVOU3_SESSION_ARTIFACTS") or default
    try:
        os.makedirs(art, exist_ok=True)
    except Exception:
        art = os.path.join(os.path.expanduser("~"), ".pinvou3")
        os.makedirs(art, exist_ok=True)
    return art


def _check(d):
    """立账核账(对账不评论):逐条查规范。返回 {ok, issues:[{level,msg}]}。"""
    issues = []
    err = lambda m: issues.append({"level": "error", "msg": m})
    warn = lambda m: issues.append({"level": "warn", "msg": m})

    wenzhong = (d.get("文种") or "").strip()
    title = (d.get("标题") or "").strip()
    fwzh = (d.get("发文字号") or "").strip()
    zhusong = (d.get("主送机关") or "").strip()
    body = d.get("正文") or []
    riqi = (d.get("成文日期") or "").strip()
    fwjg = (d.get("发文机关") or "").strip()
    luokuan = (d.get("落款机关") or fwjg or "").strip()

    if not wenzhong:
        err("缺『文种』(通知/意见/请示/报告/函/批复…)")
    if not title:
        err("缺『标题』")
    else:
        # 标题应 = 发文机关 + 关于 + 事由 + 文种,且文种与标题结尾一致
        if wenzhong and not title.endswith(wenzhong):
            warn("标题结尾『%s』与文种『%s』不一致" % (title[-3:], wenzhong))
        if "关于" not in title:
            warn("标题缺『关于…的』结构")
        if title.count("，") or title.count("。") or title.count("、"):
            warn("标题含逗号/句号/顿号等标点(公文标题除书名号外不用标点)")
    # 发文字号后缀性质。序号留 X/N 占位是起草纪律的合法状态(序号由发文机关登记后赋予),
    # 放行不报;只对真正的格式错误(缺〔〕、序号非数字非占位)报 warn。
    if not fwzh:
        warn("缺『发文字号』")
    elif not re.search(r"〔\d{4}〕(\d+|[XxNn×])号$", fwzh):
        warn("发文字号格式异常,应形如『穗府办规〔2026〕12号』(序号待编可留 X)")
    if not zhusong:
        err("缺『主送机关』")
    if not body:
        err("『正文』为空")
    # 层级序号校验:出现的级别是否按 一、→（一）→1. 顺序、不跳级
    seen = [b.get("级别") for b in body if b.get("级别")]
    order = {"一级": 1, "二级": 2, "三级": 3, "四级": 4, "正文": 0}
    last = 0
    for lv in seen:
        n = order.get(lv, 0)
        if n and n > last + 1 and last != 0:
            warn("层级序号疑似跳级:%s 之前未见上一级" % lv)
        if n:
            last = n
    # 成文日期:阿拉伯数字 X年X月X日,无占位
    if not riqi:
        err("缺『成文日期』")
    elif not re.match(r"^\d{4}年\d{1,2}月\d{1,2}日$", riqi):
        warn("成文日期应为阿拉伯数字『2026年5月28日』式,且不留占位(当前:%s)" % riqi)
    if not luokuan:
        warn("缺『落款机关』(且无发文机关可兜底)")
    elif fwjg and luokuan != fwjg and title and not title.startswith(luokuan):
        warn("落款机关与标题/发文机关不一致")
    # 承启句:仅印发/转发型(标题含『印发』『转发』)才需『经…同意，现印发给你们』。
    # 自行制发的意见/通知本不需承启句,过去对所有意见/通知误报,反诱导模型乱塞。
    bodytext = "".join(b.get("文字", "") for b in body)
    if ("印发" in title or "转发" in title) and "同意" not in bodytext:
        warn("印发/转发型正文未见承启句『经…同意，现印发给你们…』(置于正文首段)")

    ok = not any(i["level"] == "error" for i in issues)
    return {"ok": ok, "issues": issues, "checked": True}


def validate_gongwen(filename=None, **fields):
    """立账核账 MCP 入口。字段平铺(同 make_gongwen),也兼容 gongwen= 整传。"""
    try:
        d = _normalize_doc(filename, fields)
    except Exception as e:
        return {"ok": False, "issues": [{"level": "error", "msg": "公文参数无法解析:%s" % e}]}
    return _check(d)


def make_gongwen(filename=None, **fields):
    """结构化公文字段 → GB/T 9704 .docx。字段平铺(见 inputSchema)。返回 {ok, path, validate}。"""
    try:
        d = _normalize_doc(filename, fields)
    except Exception as e:
        return {"ok": False, "error": "公文参数无法解析:%s。字段见 inputSchema。" % e}

    # 立账核账:有 error 级硬伤(正文为空/缺文种·标题·主送·成文日期)→ 拒绝渲染,
    # 把 issues 带回逼模型补好 JSON 再出件,杜绝空壳废件被 present_artifact 当成品。
    # warn 级(字号格式/标题标点/承启句等)不阻断,照常渲染并随结果带回提示。
    v = _check(d)
    if not v["ok"]:
        return {"ok": False, "blocked_by_validate": True, "issues": v["issues"],
                "hint": "存在 error 级硬伤,未渲染。请按 issues 补全字段后重调 make_gongwen。"}

    fname = _sanitize_filename(filename or d.get("标题") or "公文")
    out = _unique_path(_artifacts_dir(), fname)
    try:
        gbt9704_styles.render_gongwen(d, out)
    except Exception as e:
        return {"ok": False, "error": "套版渲染失败:%s" % e}
    return {"ok": True, "path": out, "文种": d.get("文种"), "validate": v,
            "hint": ".docx 是二进制成品;改内容请改 JSON 重调 make_gongwen,勿用 read_file/edit_file。"
                    "拿 path 调 present_artifact 上卡即可。"}


# ── 工具定义 ────────────────────────────────────────────────────────────────
_BODY_DESC = (
    "body 数组,每项 {level, text}。level 取 一级|二级|三级|四级|正文,渲染器据此套字体字号"
    "(一级=黑体三号;二级=楷体三号;三级=仿宋三号加粗;正文=仿宋三号首行缩进2字)。"
    "text 含序号前缀本身(一级写 一、,二级写 （一）,三级写 1.),渲染器只套字体不补序号。"
)
_GONGWEN_SCHEMA = {
    "type": "object",
    "properties": {
        "doc_type": {"type": "string", "description": "文种:通知/意见/请示/报告/函/批复"},
        "issuer": {"type": "string", "description": "发文机关,如 广州市人民政府办公厅(作红头与落款兜底)"},
        "doc_number": {"type": "string", "description": "发文字号,完整如 穗府办规〔2026〕12号;规范性文件用 …规,函式用 …函,普通用 …"},
        "title": {"type": "string", "description": "标题=发文机关+关于+事由+文种;印发型务必含书名号《主件名》;除书名号外不用标点"},
        "recipient": {"type": "string", "description": "主送机关,如 各区人民政府，市政府各部门、各直属机构(渲染器自动补末尾冒号)"},
        "body": {"type": "array", "items": {"type": "object"}, "description": _BODY_DESC},
        "signer": {"type": "string", "description": "落款机关,缺省=发文机关"},
        "date": {"type": "string", "description": "成文日期,阿拉伯数字如 2026年5月28日;不留 XX 占位"},
        "disclosure": {"type": "string", "description": "公开方式,缺省 主动公开"},
        "print_note": {"type": "string", "description": "印发说明,如 广州市人民政府办公厅秘书处 2026年5月29日印发(可空)"},
        "attachments": {
            "type": "array",
            "description": "印发型通知的主件(办法/措施/规划等):每项 {title, body:[blocks]},渲染时换页+居中标题。",
            "items": {"type": "object"},
        },
    },
    "required": ["doc_type", "title", "recipient", "body", "date"],
}

TOOL_DEFS = [
    {
        "name": "make_gongwen",
        "description": ("结构化公文 JSON → 合规 .docx(GB/T 9704 党政机关公文格式:方正小标宋标题、仿宋_GB2312 正文、"
                        "国标页边距/行距、红头与红色分隔线)。内容你来写进 JSON,套版由它做。生成后拿 path 再调 present_artifact 上卡。"),
        "inputSchema": _GONGWEN_SCHEMA,
    },
    {
        "name": "validate_gongwen",
        "description": "立账核账:对公文字段逐项查党政机关公文规范(标题/字号/主送/层级序号/落款日期/承启句),返回 issues 清单。字段平铺,同 make_gongwen。出件前自检用。",
        "inputSchema": {"type": "object", "properties": _GONGWEN_SCHEMA["properties"]},
    },
]
DISPATCH = {"make_gongwen": make_gongwen, "validate_gongwen": validate_gongwen}


# ── MCP stdio JSON-RPC(套 pptx server 范式)────────────────────────────────
def _send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _result(req_id, result):
    _send({"jsonrpc": "2.0", "id": req_id, "result": result})


def _error(req_id, code, message):
    _send({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}})


def _handle(msg):
    method = msg.get("method")
    req_id = msg.get("id")
    if req_id is None:
        return
    if method == "initialize":
        _result(req_id, {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pinvou3-gongwen", "version": "1.0.0"},
        })
    elif method == "tools/list":
        _result(req_id, {"tools": TOOL_DEFS})
    elif method == "tools/call":
        params = msg.get("params") or {}
        name = params.get("name")
        fn = DISPATCH.get(name)
        if fn is None:
            _error(req_id, -32601, "unknown tool: %s" % name)
            return
        args = params.get("arguments") or {}
        result = fn(**args)
        _result(req_id, {"content": [{"type": "text", "text": json.dumps(result, ensure_ascii=False)}]})
    else:
        _error(req_id, -32601, "method not found: %s" % method)


def main():
    for line in sys.stdin:
        line = line.lstrip("﻿").strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        try:
            _handle(msg)
        except Exception as e:
            rid = msg.get("id") if isinstance(msg, dict) else None
            if rid is not None:
                _error(rid, -32603, "internal error: %s" % e)


if __name__ == "__main__":
    main()
