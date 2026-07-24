#!/usr/bin/env python3
"""
pinvou3 Obsidian 知识库 MCP server —— 检索 + 管理本机 Obsidian vault（零第三方依赖，纯 stdlib）。

用法：由 CodeWhale MCP client 通过 stdio 启动。
配置：~/.pinvou3/.../mcp.json 中注册。OBSIDIAN_VAULT_PATH 可选——
      没配则自动读 Obsidian 的 obsidian.json 发现当前库（跨 Win/mac/Linux）。

协议：newline-delimited JSON-RPC 2.0 over stdio（骨架对齐 weather/server.py）。
LLM 可见工具：
  读：mcp_obsidian_search / mcp_obsidian_read_note / mcp_obsidian_list
  写：mcp_obsidian_create_note / mcp_obsidian_edit_note /
      mcp_obsidian_rename_note / mcp_obsidian_delete_note

特性：
- 读：搜索 / 读取 / 列目录。
- 写：新建 / 编辑追加 / 改名搬移（自动修全库 [[wikilinks]]）/ 删除。
- 自动发现 vault：未配 OBSIDIAN_VAULT_PATH 时读 obsidian.json 取 open:true 的库。
- 只索引 .md，跳过 .obsidian 配置目录与附件二进制。
- 一切路径做越界（..）+ symlink 防护，强制限定在 vault 根目录内。
- 删除 / 改名默认人在环中：先返回 confirm_required 预览，带 confirm=true 才真正执行。
"""
import io
import json
import os
import re
import sys

# Windows 默认 stdout/stdin 编码为 GBK，MCP 协议要求 UTF-8
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")


# ── vault 定位（手配优先，否则自动发现）──────────────────────────────────────

def _obsidian_config_path():
    """Obsidian 记录"有哪些库 / 哪个开着"的 obsidian.json，跨平台路径。"""
    if sys.platform == "win32":
        return os.path.join(os.environ.get("APPDATA", ""), "obsidian", "obsidian.json")
    if sys.platform == "darwin":
        return os.path.expanduser("~/Library/Application Support/obsidian/obsidian.json")
    return os.path.expanduser("~/.config/obsidian/obsidian.json")  # Linux


def _autodiscover_vault():
    """读 obsidian.json：优先 open:true 的库，否则取最近打开（ts 最大）的。"""
    try:
        with open(_obsidian_config_path(), "r", encoding="utf-8-sig") as f:
            vaults = json.load(f).get("vaults", {})
    except (OSError, ValueError):
        return ""
    if not vaults:
        return ""
    opened = [v for v in vaults.values() if v.get("open")]
    pick = opened[0] if opened else max(vaults.values(), key=lambda v: v.get("ts", 0))
    return os.path.normpath(os.path.expanduser(pick.get("path", "")))


def _vault_path():
    """vault 根目录：手配的 OBSIDIAN_VAULT_PATH 优先，没配就自动发现。"""
    raw = os.environ.get("OBSIDIAN_VAULT_PATH", "")
    raw = raw.strip().strip('"').strip("'").strip()
    if raw:
        return os.path.normpath(os.path.expanduser(raw))
    return _autodiscover_vault()


SKIP_DIRS = {".obsidian", ".trash", ".git", "node_modules"}
MAX_NOTE_CHARS = 40000          # read_note 单篇返回上限，防爆上下文
SNIPPET_RADIUS = 120            # 命中片段前后各取多少字符
DEFAULT_SEARCH_LIMIT = 10
DEFAULT_LIST_LIMIT = 200


# ── vault 遍历 ────────────────────────────────────────────────────────────

def _iter_md(vault, sub=""):
    """遍历 vault（或其子目录 sub）下所有 .md 的绝对路径，跳过配置/附件目录。"""
    root = os.path.join(vault, sub) if sub else vault
    for dirpath, dirnames, filenames in os.walk(root):
        # 原地裁剪：跳过隐藏目录与配置目录
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and not d.startswith(".")]
        for name in filenames:
            if name.lower().endswith(".md"):
                yield os.path.join(dirpath, name)


def _rel(vault, full):
    """返回相对 vault 的正斜杠路径，跨平台统一。"""
    return os.path.relpath(full, vault).replace(os.sep, "/")


def _read_text(full):
    try:
        # utf-8-sig 自动剥离某些编辑器写入的 BOM，避免污染片段/正文
        with open(full, "r", encoding="utf-8-sig", errors="ignore") as f:
            return f.read()
    except OSError:
        return ""


def _write_text(full, text):
    """写文件（UTF-8），自动建父目录。"""
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w", encoding="utf-8") as f:
        f.write(text)


def _safe_join(vault, rel):
    """把用户给的相对路径限制在 vault 内，realpath 解析后防 .. 与 symlink 越界。返回绝对路径或 None。"""
    if not rel:
        return None
    rel = rel.strip().strip('"').strip("'").replace("\\", "/")
    vault_real = os.path.realpath(vault)
    # realpath 解析路径中的 symlink，防 vault 内 symlink 指向外部目录/文件越界
    candidate = os.path.realpath(os.path.join(vault_real, rel))
    # 必须仍在 vault 真实根目录之内
    if candidate != vault_real and not candidate.startswith(vault_real + os.sep):
        return None
    return candidate


def _norm_md(path):
    """补 .md 后缀。"""
    p = (path or "").strip().strip('"').strip("'")
    if p and not p.lower().endswith(".md"):
        p += ".md"
    return p


# ── 读：三个工具实现 ──────────────────────────────────────────────────────

def search(query="", limit=DEFAULT_SEARCH_LIMIT):
    vault = _vault_path()
    if not vault:
        return {"error": "未找到 vault：请配置 OBSIDIAN_VAULT_PATH 或先在 Obsidian 打开一个库"}
    if not os.path.isdir(vault):
        return {"error": "vault 路径不存在或不是文件夹: %s" % vault}

    query = (query or "").strip()
    if not query:
        return {"error": "query 不能为空"}

    try:
        limit = int(limit)
    except (TypeError, ValueError):
        limit = DEFAULT_SEARCH_LIMIT
    limit = max(1, min(limit, 50))

    # 拆词：所有词都命中（AND）才算匹配，提升召回精度
    terms = [t.lower() for t in re.split(r"\s+", query) if t]
    hits = []
    for full in _iter_md(vault):
        rel = _rel(vault, full)
        text = _read_text(full)
        haystack = (rel + "\n" + text).lower()
        if not all(t in haystack for t in terms):
            continue
        # 评分：词频之和 + 文件名命中加权
        score = sum(haystack.count(t) for t in terms)
        if any(t in os.path.basename(rel).lower() for t in terms):
            score += 5
        hits.append((score, rel, text, terms))

    hits.sort(key=lambda h: h[0], reverse=True)
    results = []
    for score, rel, text, terms in hits[:limit]:
        results.append({
            "path": rel,
            "title": os.path.splitext(os.path.basename(rel))[0],
            "snippet": _snippet(text, terms),
            "score": score,
        })
    return {"type": "obsidian_search", "query": query, "count": len(results), "results": results}


def _snippet(text, terms):
    low = text.lower()
    pos = -1
    for t in terms:
        p = low.find(t)
        if p != -1 and (pos == -1 or p < pos):
            pos = p
    if pos == -1:
        return text[:SNIPPET_RADIUS * 2].strip()
    start = max(0, pos - SNIPPET_RADIUS)
    end = min(len(text), pos + SNIPPET_RADIUS)
    snip = text[start:end].replace("\n", " ").strip()
    return ("…" if start > 0 else "") + snip + ("…" if end < len(text) else "")


def read_note(path=""):
    vault = _vault_path()
    if not vault:
        return {"error": "未找到 vault：请配置 OBSIDIAN_VAULT_PATH 或先在 Obsidian 打开一个库"}
    if not os.path.isdir(vault):
        return {"error": "vault 路径不存在: %s" % vault}

    full = _safe_join(vault, _norm_md(path))
    if full is None:
        return {"error": "非法路径（越界或为空）: %s" % path}
    if not os.path.isfile(full):
        return {"error": "笔记不存在: %s" % path}

    text = _read_text(full)
    truncated = len(text) > MAX_NOTE_CHARS
    return {
        "type": "obsidian_note",
        "path": _rel(vault, full),
        "title": os.path.splitext(os.path.basename(full))[0],
        "truncated": truncated,
        "content": text[:MAX_NOTE_CHARS],
    }


def list_notes(folder="", tag="", limit=DEFAULT_LIST_LIMIT):
    vault = _vault_path()
    if not vault:
        return {"error": "未找到 vault：请配置 OBSIDIAN_VAULT_PATH 或先在 Obsidian 打开一个库"}
    if not os.path.isdir(vault):
        return {"error": "vault 路径不存在: %s" % vault}

    sub = ""
    if folder:
        safe = _safe_join(vault, folder)
        if safe is None or not os.path.isdir(safe):
            return {"error": "子目录不存在: %s" % folder}
        sub = os.path.relpath(safe, vault)

    try:
        limit = int(limit)
    except (TypeError, ValueError):
        limit = DEFAULT_LIST_LIMIT
    limit = max(1, min(limit, 1000))

    tag = (tag or "").strip().lstrip("#").lower()
    tag_re = re.compile(r"(?:^|\s)#" + re.escape(tag) + r"\b", re.IGNORECASE) if tag else None

    items = []
    for full in _iter_md(vault, sub):
        rel = _rel(vault, full)
        if tag_re is not None:
            text = _read_text(full)
            if not (tag_re.search(text) or _frontmatter_has_tag(text, tag)):
                continue
        items.append({"path": rel, "title": os.path.splitext(os.path.basename(rel))[0]})
        if len(items) >= limit:
            break

    items.sort(key=lambda x: x["path"].lower())
    return {"type": "obsidian_list", "folder": folder or "/", "tag": tag, "count": len(items), "notes": items}


def _frontmatter_has_tag(text, tag):
    """简单识别 YAML frontmatter 里的 tags: [a, b] / tags:\n  - a。"""
    if not text.startswith("---"):
        return False
    end = text.find("\n---", 3)
    if end == -1:
        return False
    fm = text[3:end].lower()
    m = re.search(r"tags\s*:\s*(.+)", fm)
    if m and tag in re.split(r"[\s,\[\]\"']+", m.group(1)):
        return True
    # 多行列表形式
    return bool(re.search(r"^\s*-\s*" + re.escape(tag) + r"\s*$", fm, re.MULTILINE))


# ── 写：新建 / 编辑 / 改名修双链 / 删除 ────────────────────────────────────

def create_note(path="", content=""):
    """新建笔记。已存在则报错（不覆盖，改用 edit_note）。"""
    vault = _vault_path()
    if not vault or not os.path.isdir(vault):
        return {"error": "未找到 vault"}
    full = _safe_join(vault, _norm_md(path))
    if full is None:
        return {"error": "非法路径（越界或为空）: %s" % path}
    if os.path.exists(full):
        return {"error": "笔记已存在，改用 edit_note: %s" % path}
    _write_text(full, content or "")
    return {"type": "obsidian_created", "path": _rel(vault, full)}


def edit_note(path="", content="", mode="append"):
    """编辑笔记。mode=append 追加 / replace 整篇替换。"""
    vault = _vault_path()
    if not vault or not os.path.isdir(vault):
        return {"error": "未找到 vault"}
    full = _safe_join(vault, _norm_md(path))
    if full is None or not os.path.isfile(full):
        return {"error": "笔记不存在: %s" % path}
    if mode not in ("append", "replace"):
        return {"error": "mode 只能是 append 或 replace"}
    if mode == "append":
        old = _read_text(full)
        new = old + ("\n" if old and not old.endswith("\n") else "") + (content or "")
    else:
        new = content or ""
    _write_text(full, new)
    return {"type": "obsidian_edited", "path": _rel(vault, full), "mode": mode}


def _wikilink_rewrites(vault, old_name, new_name):
    """扫全库，算出把指向 old_name 的引用改成 new_name 后需要改写的文件。
    覆盖 [[名]] / [[名|别名]] / [[名#标题]] / [[名#^块]] / ![[名]] 以及 ](名.md)。
    返回 [(file, new_text), ...]。按笔记名（basename）匹配——Obsidian 默认引用方式。"""
    # 名后必跟 # | 或 ]（避免误伤前缀相同的别的笔记名）
    wiki = re.compile(r"(!?\[\[)" + re.escape(old_name) + r"(?=[#|\]])")
    # markdown 链接形式 ](可能的路径/old.md)。old 前必须紧跟 `(` 或以 `/` 结尾的
    # 路径前缀,否则 `](my_old.md)` 里的 `old.md` 会被误匹配 → 改坏别的笔记的链接。
    mdlink = re.compile(r"(\]\((?:[^)]*/)?)" + re.escape(old_name) + r"\.md(\))")
    out = []
    for f in _iter_md(vault):
        text = _read_text(f)
        new_text = wiki.sub(lambda m: m.group(1) + new_name, text)
        new_text = mdlink.sub(lambda m: m.group(1) + new_name + ".md" + m.group(2), new_text)
        if new_text != text:
            out.append((f, new_text))
    return out


def rename_note(old="", new="", confirm=False):
    """改名/搬移笔记，并自动重写全库指向它的 [[wikilinks]] 与 md 链接。
    confirm=False 时只返回预览（将影响几处引用），带 confirm=true 才真正执行。"""
    vault = _vault_path()
    if not vault or not os.path.isdir(vault):
        return {"error": "未找到 vault"}
    src = _safe_join(vault, _norm_md(old))
    dst = _safe_join(vault, _norm_md(new))
    if src is None or not os.path.isfile(src):
        return {"error": "源笔记不存在: %s" % old}
    if dst is None:
        return {"error": "目标路径非法（越界）: %s" % new}
    if os.path.exists(dst):
        return {"error": "目标已存在: %s" % new}

    old_name = os.path.splitext(os.path.basename(src))[0]
    new_name = os.path.splitext(os.path.basename(dst))[0]
    rewrites = _wikilink_rewrites(vault, old_name, new_name)

    if not confirm:
        return {
            "type": "confirm_required", "action": "rename",
            "from": _rel(vault, src), "to": _rel(vault, dst),
            "files_to_fix": len(rewrites),
            "files_affected": [_rel(vault, f) for f, _ in rewrites],
            "hint": "确认无误请再次调用并带 confirm=true；引用按笔记名匹配，"
                    "若库内有同名笔记可能误改，建议核对 files_affected。",
        }

    os.makedirs(os.path.dirname(dst), exist_ok=True)
    os.rename(src, dst)
    for f, new_text in rewrites:
        _write_text(f, new_text)
    return {
        "type": "obsidian_renamed",
        "from": _rel(vault, src), "to": _rel(vault, dst),
        "files_fixed": len(rewrites),
    }


def delete_note(path="", confirm=False):
    """删除笔记。confirm=False 只返回预览，带 confirm=true 才真删。"""
    vault = _vault_path()
    if not vault or not os.path.isdir(vault):
        return {"error": "未找到 vault"}
    full = _safe_join(vault, _norm_md(path))
    if full is None or not os.path.isfile(full):
        return {"error": "笔记不存在: %s" % path}
    rel = _rel(vault, full)
    if not confirm:
        return {
            "type": "confirm_required", "action": "delete", "path": rel,
            "hint": "确认删除请再次调用并带 confirm=true（不可恢复）",
        }
    os.remove(full)
    return {"type": "obsidian_deleted", "path": rel}


# ── 工具定义 ──────────────────────────────────────────────────────────────

TOOL_DEFS = [
    {
        "name": "search",
        "description": "全文检索 Obsidian 笔记库，返回命中笔记的路径、标题与片段。多个关键词按 AND 匹配。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "检索关键词，可多个空格分隔"},
                "limit": {"type": "integer", "description": "最多返回几条，默认 10，上限 50"},
            },
            "required": ["query"],
        },
    },
    {
        "name": "read_note",
        "description": "读取指定笔记的完整内容。path 为相对 vault 的路径（如 '项目/方案.md'）。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对 vault 的笔记路径，.md 后缀可省略"},
            },
            "required": ["path"],
        },
    },
    {
        "name": "list",
        "description": "列出笔记，可按子目录 folder 或标签 tag 过滤。用于浏览知识库结构。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "folder": {"type": "string", "description": "限定子目录，留空为全库"},
                "tag": {"type": "string", "description": "按 #标签 过滤，留空不过滤"},
                "limit": {"type": "integer", "description": "最多返回几条，默认 200"},
            },
            "required": [],
        },
    },
    {
        "name": "create_note",
        "description": "新建笔记。path 为相对 vault 的路径，content 为正文。笔记已存在会报错（改用 edit_note）。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对 vault 的笔记路径，.md 后缀可省略"},
                "content": {"type": "string", "description": "笔记正文（Markdown）"},
            },
            "required": ["path"],
        },
    },
    {
        "name": "edit_note",
        "description": "编辑已存在的笔记。mode=append 在末尾追加，mode=replace 整篇替换。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对 vault 的笔记路径"},
                "content": {"type": "string", "description": "要写入的内容"},
                "mode": {"type": "string", "description": "append（默认）或 replace"},
            },
            "required": ["path", "content"],
        },
    },
    {
        "name": "rename_note",
        "description": "改名或搬移笔记，并自动修好全库指向它的 [[双链]] 引用。先不带 confirm 调用看影响范围，再带 confirm=true 执行。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "old": {"type": "string", "description": "原路径（相对 vault）"},
                "new": {"type": "string", "description": "新路径（相对 vault，可换目录=搬移）"},
                "confirm": {"type": "boolean", "description": "true 才真正执行；false/省略只返回预览"},
            },
            "required": ["old", "new"],
        },
    },
    {
        "name": "delete_note",
        "description": "删除笔记（不可恢复）。先不带 confirm 调用确认目标，再带 confirm=true 执行。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对 vault 的笔记路径"},
                "confirm": {"type": "boolean", "description": "true 才真正删除；false/省略只返回预览"},
            },
            "required": ["path"],
        },
    },
]

DISPATCH = {
    "search": search,
    "read_note": read_note,
    "list": list_notes,
    "create_note": create_note,
    "edit_note": edit_note,
    "rename_note": rename_note,
    "delete_note": delete_note,
}


# ── JSON-RPC 2.0 协议处理（对齐 weather/server.py）────────────────────────

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
            "serverInfo": {"name": "pinvou3-obsidian", "version": "1.1.0"},
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
        # 去掉行首可能的 BOM（str.strip 不会清 ﻿），再去空白
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
