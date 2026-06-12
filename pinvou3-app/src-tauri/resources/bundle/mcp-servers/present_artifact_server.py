#!/usr/bin/env python3
"""present_artifact — pinvou3 内置 MCP server(零第三方依赖,只用 stdlib)。

把一个"阶段性成品"展示给用户:Pinvou 调 present_artifact 工具,pinvou3 前端
在聊天区弹一张可点击的成品卡,客户点一下直接打开预览。

协议:newline-delimited JSON-RPC 2.0 over stdio(对齐底座 mcp.rs 的 stdio
transport:每条消息一行 JSON + '\\n',read_line 读)。protocolVersion 2024-11-05。

本 server 只做"验证文件存在 + 回显元数据",不做快照、不需要 session 上下文。
展示所需的全部信息(title / path / description)都在工具入参里,前端从 tool_use
的 args 直接渲染。相对路径相对本进程 cwd 解析 —— 引导 agent 传绝对路径更稳。
"""
import io
import json
import sys
import os

# Windows 默认 stdout/stdin 编码为 GBK，MCP 协议要求 UTF-8
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")

PROTOCOL_VERSION = "2024-11-05"

_KIND_BY_EXT = {
    ".html": "html", ".htm": "html",
    ".md": "markdown", ".markdown": "markdown",
    ".png": "image", ".jpg": "image", ".jpeg": "image",
    ".gif": "image", ".webp": "image", ".svg": "image", ".bmp": "image",
}

TOOL_DEF = {
    "name": "present_artifact",
    "description": (
        "把一个阶段性作品展示给用户 —— 聊天区会弹一张可点击的成品卡,客户点一下"
        "直接打开预览。什么时候调:产出 html / markdown / 图片等单文件作品且打算"
        "给客户看时,立刻调。什么时候别调:写中间文件、配置、脚本、做内部处理时不要调。"
        "标题要让客户一眼看懂这是什么作品(例:'SpaceX 产业趋势单页总结')。"
        "非阻塞,调完立即返回,继续对话。"
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "作品文件路径。强烈建议传绝对路径(相对路径相对工具进程 cwd,可能解析不到)。",
            },
            "title": {
                "type": "string",
                "description": "客户一眼看懂的中文标题,不要用文件名,要有叙述感。",
            },
            "description": {
                "type": "string",
                "description": "(可选)一两句话描述这是什么成品、客户为什么想看。",
            },
        },
        "required": ["path", "title"],
    },
}


def _kind(path):
    _, ext = os.path.splitext(path)
    return _KIND_BY_EXT.get(ext.lower(), "other")


def _send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _result(req_id, result):
    _send({"jsonrpc": "2.0", "id": req_id, "result": result})


def _error(req_id, code, message):
    _send({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}})


def _text_content(payload, is_error=False):
    """MCP tools/call 结果:content 数组。文本即 JSON 串,前端主要走 args,这里供兜底。"""
    return {
        "content": [{"type": "text", "text": json.dumps(payload, ensure_ascii=False)}],
        "isError": is_error,
    }


def _handle_call(req_id, params):
    args = (params or {}).get("arguments") or {}
    raw_path = (args.get("path") or "").strip()
    title = (args.get("title") or "").strip()
    description = (args.get("description") or "").strip()

    if not raw_path or not title:
        _result(req_id, _text_content(
            {"ok": False, "error": "present_artifact 需要 path 和 title"}, is_error=True))
        return

    abs_path = raw_path if os.path.isabs(raw_path) else os.path.abspath(raw_path)

    if not os.path.exists(abs_path):
        _result(req_id, _text_content(
            {"ok": False, "error": "文件不存在:%s(请传绝对路径)" % abs_path}, is_error=True))
        return
    if not os.path.isfile(abs_path):
        _result(req_id, _text_content(
            {"ok": False, "error": "路径不是文件:%s" % abs_path}, is_error=True))
        return

    _result(req_id, _text_content({
        "ok": True,
        "kind": _kind(abs_path),
        "abs_path": abs_path,
        "basename": os.path.basename(abs_path),
        "bytes": os.path.getsize(abs_path),
        "title": title,
        "description": description,
    }))


def _handle(msg):
    method = msg.get("method")
    req_id = msg.get("id")

    # 通知(无 id):initialized 等,不回复
    if req_id is None:
        return

    if method == "initialize":
        _result(req_id, {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pinvou3-present-artifact", "version": "1.0.0"},
        })
    elif method == "tools/list":
        _result(req_id, {"tools": [TOOL_DEF]})
    elif method == "tools/call":
        params = msg.get("params") or {}
        if params.get("name") == "present_artifact":
            _handle_call(req_id, params)
        else:
            _error(req_id, -32601, "unknown tool: %s" % params.get("name"))
    else:
        _error(req_id, -32601, "method not found: %s" % method)


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue  # 跳过坏行,不崩
        try:
            _handle(msg)
        except Exception as e:
            rid = msg.get("id") if isinstance(msg, dict) else None
            if rid is not None:
                _error(rid, -32603, "internal error: %s" % e)


if __name__ == "__main__":
    main()
