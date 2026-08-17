#!/usr/bin/env python3
"""
pinvou3 企微群机器人 MCP server — 包装企业微信官方群机器人 webhook
（消息推送 API，https://developer.work.weixin.qq.com/document/path/91770），
零第三方依赖，纯 stdlib。

用法：由 CodeWhale MCP client 通过 stdio 启动。
配置：~/.pinvou3/bundle/mcp.json 中注册，WECOM_BOT_KEY 通过 env 传入
（密钥本体存本机系统凭据库，mcp.json 里只有 ${...} 占位符）。

协议：newline-delimited JSON-RPC 2.0 over stdio。
LLM 可见工具名：mcp_wecom-bot_send_text / send_markdown / send_news /
send_image / send_file
"""
import base64
import hashlib
import io
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid

# Windows 默认 stdout/stdin 编码为 GBK，MCP 协议要求 UTF-8
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")

SEND_URL = "https://qyapi.weixin.qq.com/cgi-bin/webhook/send"
UPLOAD_URL = "https://qyapi.weixin.qq.com/cgi-bin/webhook/upload_media"

MAX_TEXT_BYTES = 2048        # 官方限制：文本最长 2048 字节(utf-8)
MAX_MARKDOWN_BYTES = 4096    # 官方限制：markdown 最长 4096 字节
MAX_NEWS_ARTICLES = 8        # 官方限制：图文 1-8 条
MAX_IMAGE_BYTES = 2 * 1024 * 1024   # 官方限制：图片 base64 前不超过 2M，仅 JPG/PNG
MAX_FILE_BYTES = 20 * 1024 * 1024   # 官方限制：文件不超过 20M
MIN_FILE_BYTES = 5                  # 官方限制：文件须大于 5 字节


def _bot_key():
    """取 webhook key：用户可粘贴完整 webhook URL 或纯 key，这里统一解析。"""
    raw = os.environ.get("WECOM_BOT_KEY", "").strip()
    if not raw:
        return ""
    if "://" in raw:
        query = urllib.parse.urlparse(raw).query
        key = urllib.parse.parse_qs(query).get("key", [""])[0].strip()
        return key
    return raw


BOT_KEY = _bot_key()


TEXT_DEF = {
    "name": "send_text",
    "description": (
        "向企业微信群机器人绑定的群发送文本消息，可在群里 @成员。"
        "content 最长约 2048 字节；用 <@userid> 语法提及成员"
        "（userid 需从成员资料获取，手机号提及用 mentioned_mobile_list）。"
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "content": {"type": "string", "description": "消息文本，最长 2048 字节"},
            "mentioned_list": {
                "type": "array",
                "description": "要 @ 的成员 userid 列表，['@all'] 表示 @所有人",
                "items": {"type": "string"},
            },
            "mentioned_mobile_list": {
                "type": "array",
                "description": "要 @ 的成员手机号列表，['@all'] 表示 @所有人",
                "items": {"type": "string"},
            },
        },
        "required": ["content"],
    },
}

MARKDOWN_DEF = {
    "name": "send_markdown",
    "description": (
        "向企业微信群机器人绑定的群发送 Markdown 消息（标题、加粗、链接、"
        "行内代码、引用、字体颜色等），content 最长约 4096 字节。"
        "无 mentioned_list 字段；@ 成员可在 content 中用 <@userid> 语法"
        "（官方 text/markdown 均支持），按手机号 @ 请改用 send_text。"
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "content": {"type": "string", "description": "Markdown 文本，最长 4096 字节"}
        },
        "required": ["content"],
    },
}

NEWS_DEF = {
    "name": "send_news",
    "description": (
        "向企业微信群机器人绑定的群发送图文消息（一次 1-8 条，手机端按卡片流展示）。"
        "每条 article 含 title(必填，128 字节内)、description(512 字节内)、"
        "url(点击跳转)、picurl(封面图 URL)。"
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "articles": {
                "type": "array",
                "description": "图文条目数组，1-8 条",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "标题，必填"},
                        "description": {"type": "string", "description": "摘要，可选"},
                        "url": {"type": "string", "description": "点击跳转链接，可选"},
                        "picurl": {"type": "string", "description": "封面图 URL，可选"},
                    },
                    "required": ["title"],
                },
            }
        },
        "required": ["articles"],
    },
}

IMAGE_DEF = {
    "name": "send_image",
    "description": (
        "向企业微信群机器人绑定的群发送本地图片（JPG/PNG，base64 编码前不超过 2M）。"
        "image_path 为本机图片文件绝对路径。"
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "image_path": {"type": "string", "description": "本机图片文件路径（JPG/PNG）"}
        },
        "required": ["image_path"],
    },
}

FILE_DEF = {
    "name": "send_file",
    "description": (
        "向企业微信群机器人绑定的群发送本地文件（不超过 20M）。"
        "file_path 为本机文件绝对路径；工具会先上传临时素材再发送，"
        "media_id 3 天内有效。"
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "file_path": {"type": "string", "description": "本机文件路径"}
        },
        "required": ["file_path"],
    },
}

TOOL_DEFS = [TEXT_DEF, MARKDOWN_DEF, NEWS_DEF, IMAGE_DEF, FILE_DEF]


# 企业微信 API 是国内域名：强制直连、绕过代理（同 weather，避免代理掐断 TLS）。
# 与 weather 不同：本 server 发的是消息推送 POST，有副作用——超时后自动重试会
# 重复发消息，因此 send 路径单次请求不重试，失败如实返回交给模型决定。
_DIRECT_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def _http_post_json(url, payload):
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(
        url, data=body, headers={"Content-Type": "application/json"}
    )
    try:
        with _DIRECT_OPENER.open(req, timeout=15) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        # HTTP 层错误读 body 里的 errcode/errmsg
        detail = e.read().decode("utf-8", "replace")
        try:
            return json.loads(detail)
        except Exception:
            return {"errcode": e.code, "errmsg": "HTTP %s: %s" % (e.code, detail[:200])}
    except Exception as e:
        return {"errcode": -1, "errmsg": "网络请求失败: %s" % e}


def _check_reply(reply):
    """官方返回 errcode!=0 即失败；0 或缺省视为成功。"""
    if int(reply.get("errcode", 0) or 0) != 0:
        return {"error": "企业微信 API 错误 %s: %s"
                % (reply.get("errcode"), reply.get("errmsg", "")),
                "errcode": reply.get("errcode"), "errmsg": reply.get("errmsg", "")}
    return {"ok": True}


def _send_message(msg):
    if not BOT_KEY:
        return {"error": "WECOM_BOT_KEY 未配置，请在工具详情里填写群机器人 webhook key"}
    url = "%s?key=%s" % (SEND_URL, urllib.parse.quote(BOT_KEY))
    return _check_reply(_http_post_json(url, msg))


def _as_list(raw):
    """宽容解析数组参数：模型可能传 JSON 字符串而非原生数组；dict 元素原样保留。"""
    if raw is None or raw == "":
        return None
    if isinstance(raw, str):
        try:
            raw = json.loads(raw)
        except Exception:
            return [raw]
    if isinstance(raw, list):
        return [x if isinstance(x, dict) else str(x) for x in raw]
    return [str(raw)]


def send_text(content, mentioned_list=None, mentioned_mobile_list=None):
    content = (content or "").strip()
    if not content:
        return {"error": "content 不能为空"}
    if len(content.encode("utf-8")) > MAX_TEXT_BYTES:
        return {"error": "文本超过 2048 字节限制（当前 %d 字节），请精简或拆分"
                % len(content.encode("utf-8"))}
    msg = {"msgtype": "text", "text": {"content": content}}
    mentioned = _as_list(mentioned_list)
    mobiles = _as_list(mentioned_mobile_list)
    if mentioned:
        msg["text"]["mentioned_list"] = mentioned
    if mobiles:
        msg["text"]["mentioned_mobile_list"] = mobiles
    return _send_message(msg)


def send_markdown(content):
    content = (content or "").strip()
    if not content:
        return {"error": "content 不能为空"}
    if len(content.encode("utf-8")) > MAX_MARKDOWN_BYTES:
        return {"error": "Markdown 超过 4096 字节限制（当前 %d 字节），请精简或拆分"
                % len(content.encode("utf-8"))}
    return _send_message({"msgtype": "markdown", "markdown": {"content": content}})


def _maybe_json_obj(item):
    """模型可能把每条 article 传成 JSON 字符串，宽容解析回 dict。"""
    if isinstance(item, str):
        try:
            parsed = json.loads(item)
            if isinstance(parsed, dict):
                return parsed
        except Exception:
            pass
    return item


def send_news(articles):
    articles = _as_list(articles)
    if isinstance(articles, list):
        articles = [_maybe_json_obj(a) for a in articles]
    if not isinstance(articles, list) or not articles:
        return {"error": "articles 不能为空，需要 1-8 条图文"}
    if len(articles) > MAX_NEWS_ARTICLES:
        return {"error": "图文最多 8 条（当前 %d 条），请删减" % len(articles)}
    cleaned = []
    for i, art in enumerate(articles):
        if not isinstance(art, dict) or not (art.get("title") or "").strip():
            return {"error": "第 %d 条图文缺少 title" % (i + 1)}
        item = {"title": art["title"].strip()}
        for src, dst in (("description", "description"), ("url", "url"), ("picurl", "picurl")):
            val = art.get(src)
            if val and str(val).strip():
                item[dst] = str(val).strip()
        cleaned.append(item)
    return _send_message({"msgtype": "news", "news": {"articles": cleaned}})


def send_image(image_path):
    path = (image_path or "").strip()
    if not path:
        return {"error": "image_path 不能为空"}
    path = os.path.expanduser(path)
    if not os.path.isfile(path):
        return {"error": "图片不存在: %s" % path}
    size = os.path.getsize(path)
    if size > MAX_IMAGE_BYTES:
        return {"error": "图片超过 2M 限制（当前 %.1fM）" % (size / 1024 / 1024)}
    with open(path, "rb") as f:
        data = f.read()
    return _send_message({
        "msgtype": "image",
        "image": {
            "base64": base64.b64encode(data).decode("ascii"),
            "md5": hashlib.md5(data).hexdigest(),
        },
    })


def _sanitize_filename(name):
    """清洗 multipart filename：引号/反斜杠/换行会破坏 Content-Disposition（同 gongwen）。"""
    name = re.sub(r'[\\/:*?"<>|\r\n]+', "_", (name or "").strip()) or "file"
    return name[:80]


def _upload_media(path):
    """官方 upload_media：multipart/form-data 上传，返回 (media_id, error)。"""
    filename = _sanitize_filename(os.path.basename(path))
    boundary = "pinvou3-%s" % uuid.uuid4().hex
    with open(path, "rb") as f:
        file_body = f.read()
    parts = []
    parts.append(
        ("--%s\r\nContent-Disposition: form-data; name=\"media\"; "
         "filename=\"%s\"\r\nContent-Type: application/octet-stream\r\n\r\n"
         % (boundary, filename)).encode("utf-8")
    )
    parts.append(file_body)
    parts.append(("\r\n--%s--\r\n" % boundary).encode("utf-8"))
    body = b"".join(parts)
    url = "%s?key=%s&type=file" % (UPLOAD_URL, urllib.parse.quote(BOT_KEY))
    req = urllib.request.Request(url, data=body, headers={
        "Content-Type": "multipart/form-data; boundary=%s" % boundary,
    })
    last_err = None
    # 上传临时素材无用户可见副作用（重复上传只是多占一份素材），可对瞬时
    # 网络抖动重试；HTTP 层错误读 body，不重试（同 _http_post_json）。
    for _ in range(3):
        try:
            with _DIRECT_OPENER.open(req, timeout=60) as resp:
                reply = json.loads(resp.read().decode("utf-8"))
                if int(reply.get("errcode", 0) or 0) != 0:
                    return None, "上传临时素材失败 %s: %s" % (
                        reply.get("errcode"), reply.get("errmsg", ""))
                media_id = reply.get("media_id", "")
                if not media_id:
                    return None, "上传成功但未返回 media_id"
                return media_id, None
        except urllib.error.HTTPError as e:
            detail = e.read().decode("utf-8", "replace")
            try:
                reply = json.loads(detail)
            except Exception:
                reply = {"errcode": e.code, "errmsg": "HTTP %s: %s" % (e.code, detail[:200])}
            return None, "上传临时素材失败 %s: %s" % (
                reply.get("errcode"), reply.get("errmsg", ""))
        except Exception as e:
            last_err = e
    return None, "上传临时素材网络失败: %s" % last_err


def send_file(file_path):
    if not BOT_KEY:
        return {"error": "WECOM_BOT_KEY 未配置，请在工具详情里填写群机器人 webhook key"}
    path = (file_path or "").strip()
    if not path:
        return {"error": "file_path 不能为空"}
    path = os.path.expanduser(path)
    if not os.path.isfile(path):
        return {"error": "文件不存在: %s" % path}
    size = os.path.getsize(path)
    if size < MIN_FILE_BYTES:
        return {"error": "文件须大于 5 字节"}
    if size > MAX_FILE_BYTES:
        return {"error": "文件超过 20M 限制（当前 %.1fM）" % (size / 1024 / 1024)}
    media_id, err = _upload_media(path)
    if err:
        return {"error": err}
    return _send_message({"msgtype": "file", "file": {"media_id": media_id}})


# ── JSON-RPC 2.0 协议处理 ────────────────────────────────────────────────

def _send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _result(req_id, result):
    _send({"jsonrpc": "2.0", "id": req_id, "result": result})


def _error(req_id, code, message):
    _send({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}})


_HANDLERS = {
    "send_text": send_text,
    "send_markdown": send_markdown,
    "send_news": send_news,
    "send_image": send_image,
    "send_file": send_file,
}


def _handle(msg):
    method = msg.get("method")
    req_id = msg.get("id")

    if req_id is None:
        return

    if method == "initialize":
        _result(req_id, {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pinvou3-wecom-bot", "version": "1.0.0"},
        })
    elif method == "tools/list":
        _result(req_id, {"tools": TOOL_DEFS})
    elif method == "tools/call":
        params = msg.get("params") or {}
        handler = _HANDLERS.get(params.get("name"))
        if handler is None:
            _error(req_id, -32601, "unknown tool: %s" % params.get("name"))
            return
        args = params.get("arguments") or {}
        result = handler(**args)
        _result(req_id, {
            "content": [{"type": "text", "text": json.dumps(result, ensure_ascii=False)}],
        })
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
            continue
        try:
            _handle(msg)
        except Exception as e:
            rid = msg.get("id") if isinstance(msg, dict) else None
            if rid is not None:
                _error(rid, -32603, "internal error: %s" % e)


if __name__ == "__main__":
    main()
