#!/usr/bin/env python3
"""
pinvou3 Obsidian 知识库 MCP server —— 检索本机 Obsidian vault（零第三方依赖，纯 stdlib）。

用法：由 DeepSeek-TUI MCP client 通过 stdio 启动。
配置：~/.pinvou3/.../mcp.json 中注册，OBSIDIAN_VAULT_PATH 通过 env 传入。

协议：newline-delimited JSON-RPC 2.0 over stdio（骨架对齐 weather/server.py）。
LLM 可见工具名：mcp_obsidian_search / mcp_obsidian_read_note / mcp_obsidian_list

特性：
- 只读：搜索 / 读取 / 列目录，不改用户笔记。
- 只索引 .md，跳过 .obsidian 配置目录与附件二进制。
- read_note 做路径越界（..）防护，强制限定在 vault 根目录内。
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


def _vault_path():
    """读取并清洗 vault 路径：去首尾引号/空格、容忍尾部斜杠、展开 ~。"""
    raw = os.environ.get("OBSIDIAN_VAULT_PATH", "")
    raw = raw.strip().strip('"').strip("'").strip()
    if not raw:
        return ""
    return os.path.normpath(os.path.expanduser(raw))


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


def _safe_join(vault, rel):
    """把用户给的相对路径限制在 vault 内，防 .. 越界。返回绝对路径或 None。"""
    if not rel:
        return None
    rel = rel.strip().strip('"').strip("'").replace("\\", "/")
    candidate = os.path.normpath(os.path.join(vault, rel))
    vault_norm = os.path.normpath(vault)
    # 必须仍在 vault 根目录之内
    if candidate != vault_norm and not candidate.startswith(vault_norm + os.sep):
        return None
    return candidate


# ── 三个工具实现 ──────────────────────────────────────────────────────────

def search(query="", limit=DEFAULT_SEARCH_LIMIT):
    vault = _vault_path()
    if not vault:
        return {"error": "OBSIDIAN_VAULT_PATH 未配置"}
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
        return {"error": "OBSIDIAN_VAULT_PATH 未配置"}
    if not os.path.isdir(vault):
        return {"error": "vault 路径不存在: %s" % vault}

    full = _safe_join(vault, path)
    if full is None:
        return {"error": "非法路径（越界或为空）: %s" % path}
    if not full.lower().endswith(".md"):
        full += ".md"          # 容忍用户省略 .md 后缀
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
        return {"error": "OBSIDIAN_VAULT_PATH 未配置"}
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
]

DISPATCH = {
    "search": search,
    "read_note": read_note,
    "list": list_notes,
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
            "serverInfo": {"name": "pinvou3-obsidian", "version": "1.0.0"},
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
