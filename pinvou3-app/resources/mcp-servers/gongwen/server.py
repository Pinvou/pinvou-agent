#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""pinvou3 公文写作 MCP server。

工具 make_gongwen —— 结构化公文字段(扁平 string)→ GB/T 9704 .docx(落产物目录);
出件前自带合规校验(_check,有 error 级硬伤则拒绝渲染并带回 issues)。

写作(文种/话术/序号)由 skill 指导模型产出进字段;此 server 只做确定性套版与校验,
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
    "attachment_title": "附件标题", "attachment_body": "附件正文",
    "level": "级别", "text": "文字",
}


def _remap_keys(obj):
    """英文 key → 中文内部 key(递归);中文 key 原样保留(belt:模型若仍产中文也认)。幂等。"""
    if isinstance(obj, dict):
        return {_EN2CN.get(k, k): _remap_keys(v) for k, v in obj.items()}
    if isinstance(obj, list):
        return [_remap_keys(x) for x in obj]
    return obj


def _text_to_blocks(text):
    """正文/主件纯文本(每段一行)→ [{级别,文字}]。级别按行首序号前缀判(与渲染器
       _infer_level 同源,模型不必标),供 _check 层级校验与渲染共用。"""
    blocks = []
    for line in (text or "").replace("\r\n", "\n").replace("\r", "\n").split("\n"):
        line = line.strip()
        if line:
            blocks.append({"级别": gbt9704_styles._infer_level(line, None), "文字": line})
    return blocks


def _body_to_blocks(v):
    """body / 附件正文 统一成 blocks。新契约 = 多行纯文本字符串(扁平字段,无 JSON 转义);
       belt 兼容 list、JSON 字符串数组(模型偶发仍序列化成 array)。"""
    if isinstance(v, list):
        return [_remap_keys(b) for b in v if isinstance(b, dict)]
    if isinstance(v, str):
        if v.lstrip()[:1] == "[":  # 像 JSON 数组
            try:
                arr = json.loads(v)
                if isinstance(arr, list):
                    return [_remap_keys(b) for b in arr if isinstance(b, dict)]
            except Exception:
                pass
        return _text_to_blocks(v)
    return []


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
       ① 扁平 string 字段(匹配 inputSchema,模型默认这么调:doc_type=…、body='多行纯文本'、
          attachment_title/attachment_body=印发主件);
       ② 整体裹进 gongwen=（dict 或 JSON 字符串）——belt 兼容。
       body / 附件正文 = 多行纯文本(新契约),也兼容 list / JSON 字符串数组(belt)。"""
    if list(fields.keys()) == ["gongwen"]:
        doc = fields["gongwen"]
        if isinstance(doc, str):
            doc = json.loads(doc)
    else:
        doc = dict(fields)
    if not isinstance(doc, dict):
        raise ValueError("公文参数必须是对象")
    doc = _remap_keys(doc)  # 英文 key → 中文
    doc["正文"] = _body_to_blocks(doc.get("正文"))
    # 附件:优先 belt 的数组;否则用扁平 附件标题/附件正文 组装单主件
    atts = []
    for a in _coerce_list(doc.get("附件")):
        a = _remap_keys(a) if isinstance(a, dict) else a
        if isinstance(a, dict):
            a["正文"] = _body_to_blocks(a.get("正文"))
            atts.append(a)
    if not atts:
        at = doc.get("附件标题")
        at = at.strip() if isinstance(at, str) else ""
        blocks = _body_to_blocks(doc.get("附件正文"))
        if at or blocks:
            atts = [{"标题": at, "正文": blocks}]
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
    # 印发主件型(标题印发《X办法/规定…》)只传了壳话术、主件章条既没进 body 也没进附件 → 空壳。
    # P0(正文为空)拦不住(承启句让 body 非空);判据=正文无任何章节序号结构(第N章/第N条/一、/（一）)。
    elif re.search(r"印发《.+?(办法|规定|规划|方案|细则|条例|意见)》", title) and not d.get("附件"):
        _struct = re.compile(r"^\s*(第[一二三四五六七八九十百]+[章条]|[一二三四五六七八九十]+、|（[一二三四五六七八九十]+）)")
        if not any(_struct.match(b.get("文字") or "") for b in body):
            err("印发《…》型缺主件正文:办法/规定主件未传入(疑似只写进了对话、未进 attachment_body);"
                "把主件章条写进 attachment_title/attachment_body 重新出件")
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
# 工具表 schema = 一组扁平 string 字段。两条真机教训叠出来的形状:
#   ① 嵌套 array-of-object(body/attachments) 会污染 Qwen3.6 mtp 的 tool_call 格式参照,把别的
#      工具(write_file/web_search)采歪成裸文本 → 故不用嵌套(对比不漂的 pptx 全扁平 string)。
#   ② 把整份公文 JSON 当单个 string 参数传,模型要做"双层 JSON 转义"(外层 tool_call + 内层公文
#      JSON),长文档(5000+字符)必在某处转义错 → 故不塞 JSON 字符串。
# 解:顶层全扁平 string 字段,function-calling 单独编码每个、保证合法;body/附件正文是"多行纯文本"
# (模型当普通写作输出,级别由渲染器按行首序号前缀自动判,无需标级别、无需任何 JSON 转义)。
_BODY_DESC = (
    "正文,多行纯文本、一段一行(非 JSON)。按公文序号逐段写,渲染器按行首前缀自动套版:"
    "一、=一级、(一)=二级、1.=三级、无前缀段=正文。换行分段。"
)
_ATT_DESC = (
    "印发型主件(办法/规定)正文,多行纯文本、法规体例,无则留空。"
    "渲染器按前缀套版:第一章=居中章标题、第一条=正文条文。"
)
_GONGWEN_SCHEMA = {
    "type": "object",
    "properties": {
        "doc_type": {"type": "string", "description": "文种:通知/意见/请示/报告/函/批复"},
        "issuer": {"type": "string", "description": "发文机关,如 广州市人民政府办公厅(红头与落款兜底)"},
        "doc_number": {"type": "string", "description": "发文字号,如 穗府办规〔2026〕X号(序号待编可留 X)"},
        "title": {"type": "string", "description": "标题=发文机关+关于+事由+文种;印发型含《主件名》;除书名号外不用标点"},
        "recipient": {"type": "string", "description": "主送机关,如 各区人民政府、市政府各部门、各直属机构"},
        "body": {"type": "string", "description": _BODY_DESC},
        "signer": {"type": "string", "description": "落款机关,缺省=发文机关"},
        "date": {"type": "string", "description": "成文日期,阿拉伯数字如 2026年5月28日;不留占位"},
        "disclosure": {"type": "string", "description": "公开方式,缺省 主动公开"},
        "print_note": {"type": "string", "description": "印发说明,可空"},
        "attachment_title": {"type": "string", "description": "印发型主件标题(办法/规定…),不带书名号;无主件留空"},
        "attachment_body": {"type": "string", "description": _ATT_DESC},
    },
    "required": ["doc_type", "title", "recipient", "body", "date"],
}

# 只暴露 make_gongwen 一个工具:validate 冗余(make 出件前自带同一个 _check、blocked_by_validate
# 拦 error),单列只会把整份 12 字段 schema 在工具表里塞第二遍、加重 mtp 残留毒,故去掉。
TOOL_DEFS = [
    {
        "name": "make_gongwen",
        "description": "把公文字段渲染成 GB/T 9704 合规 .docx,返回含 path(出件前自带合规校验);随后直接用该 path 调 present_artifact 上卡,勿自己 ls/find。",
        "inputSchema": _GONGWEN_SCHEMA,
    },
]
DISPATCH = {"make_gongwen": make_gongwen}


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
