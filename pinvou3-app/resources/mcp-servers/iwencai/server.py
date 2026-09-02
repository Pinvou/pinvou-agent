#!/usr/bin/env python3
"""
同花顺问财 — 通用 MCP Server（零依赖，纯 Python 标准库）

将以下 12 个技能封装为 MCP 工具，通过 JSON-RPC 2.0 over stdio 对外暴露：
  Pattern A (query2data — 10 个):
    hithink_market_query      行情数据
    hithink_finance_query     财务数据
    hithink_event_query       事件数据
    hithink_management_query  股东股本
    hithink_business_query    公司经营
    hithink_industry_query    行业数据
    hithink_zhishu_query      指数数据
    hithink_sector_selector   选板块
    hithink_astock_selector   选A股
    hithink_macro_query       宏观数据
  Pattern B (comprehensive/search — 2 个):
    news_search               财经资讯搜索
    announcement_search       公告搜索

数据来源：同花顺问财 (https://www.iwencai.com)
基于 lorek666/iwencai-mcp 适配。
"""

import io
import json
import os
import secrets
import sys
import urllib.error
import urllib.request
from typing import Any, Dict, List

# Windows 默认 stdout/stdin 编码为 GBK，MCP 协议要求 UTF-8
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")

SERVER_NAME = "iwencai-mcp"
SERVER_VERSION = "1.0.0"
API_BASE = "https://openapi.iwencai.com"


def _trace_id() -> str:
    return secrets.token_hex(32)


def _api_key() -> str:
    key = os.environ.get("IWENCAI_API_KEY", "")
    if not key:
        raise RuntimeError(
            "IWENCAI_API_KEY 环境变量未设置。"
            "请访问 https://www.iwencai.com/skillhub 获取 API Key 后设置。"
        )
    return key


def _claw_headers(skill_id: str, call_type: str = "normal") -> Dict[str, str]:
    return {
        "Authorization": f"Bearer {_api_key()}",
        "Content-Type": "application/json",
        "X-Claw-Call-Type": call_type,
        "X-Claw-Skill-Id": skill_id,
        "X-Claw-Skill-Version": "1.0.0",
        "X-Claw-Plugin-Id": "none",
        "X-Claw-Plugin-Version": "none",
        "X-Claw-Trace-Id": _trace_id(),
    }


def _post_json(url: str, payload: dict, headers: dict, timeout: int = 60) -> dict:
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8")
            if not body.strip():
                return {"text_response": ""}
            return json.loads(body)
    except urllib.error.HTTPError as e:
        err_body = e.read().decode("utf-8", errors="replace") if e.fp else ""
        return {"error": f"HTTP {e.code}: {e.reason}", "detail": err_body}
    except urllib.error.URLError as e:
        return {"error": f"网络错误: {e.reason}"}


def _query2data(skill_id: str, query: str, page: str = "1", limit: str = "10",
                call_type: str = "normal", timeout: int = 60) -> dict:
    url = f"{API_BASE}/v1/query2data"
    payload = {
        "query": query,
        "page": page,
        "limit": limit,
        "is_cache": "1",
        "expand_index": "true",
    }
    headers = _claw_headers(skill_id, call_type)
    return _post_json(url, payload, headers, timeout)


def _comprehensive_search(channel: str, query: str, call_type: str = "normal",
                          timeout: int = 60) -> dict:
    url = f"{API_BASE}/v1/comprehensive/search"
    payload = {
        "channels": [channel],
        "app_id": "AIME_SKILL",
        "query": query,
    }
    skill_id = "news-search" if channel == "news" else "announcement-search"
    headers = _claw_headers(skill_id, call_type)
    return _post_json(url, payload, headers, timeout)


# ── 商品/贵金属价格查询重定向(方案 B)─────────────────────────────────────
# 问财行情接口对商品/贵金属(金价/银价/油价)支持差:无论关键词怎么调,NLP 多匹配成
# "字段稀疏的外盘期货"(只有收盘价)→ 前端渲染成一堆 "--"。这类查询应走 news_search
# (返回实时报价 + 新闻,实测金价/沪金/AU9999 数据齐全)。market_query 专注 A股/ETF/指数个股。
# 含"股"则视为 A股个股(黄金股/个股/股价等),不拦。
_COMMODITY_PRICE_TERMS = (
    "金价", "银价", "油价", "铜价", "现货黄金", "现货白银", "纸黄金",
    "xauusd", "xau", "comex", "贵金属", "金店",
)


def _is_commodity_price_query(query):
    s = (query or "").lower()
    if "股" in s:  # 黄金股 / 个股 / 股价 → A股,放行
        return False
    return any(t in s for t in _COMMODITY_PRICE_TERMS)


# ── 12 个 Tool 函数 ──────────────────────────────────────────────────────

def hithink_market_query(query, page="1", limit="10", timeout=60):
    # 商品/贵金属价格 → 引导到 news_search,避免问财返回稀疏外盘数据(满屏 "--")
    if _is_commodity_price_query(query):
        return {
            "ok": False,
            "redirect": "news_search",
            "hint": "问财行情(market_query)专注 A股/ETF/指数个股,不支持商品/贵金属价格"
                    "(金价/银价/油价等)。请改用 news_search 查询(如 query='金价 黄金 最新行情'),"
                    "可拿到实时报价 + 新闻。",
        }
    return _query2data("hithink-market-query", query, page, limit, timeout=timeout)

def hithink_finance_query(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-finance-query", query, page, limit, timeout=timeout)

def hithink_event_query(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-event-query", query, page, limit, timeout=timeout)

def hithink_management_query(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-management-query", query, page, limit, timeout=timeout)

def hithink_business_query(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-business-query", query, page, limit, timeout=timeout)

def hithink_industry_query(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-industry-query", query, page, limit, timeout=timeout)

def hithink_zhishu_query(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-zhishu-query", query, page, limit, timeout=timeout)

def hithink_sector_selector(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-sector-selector", query, page, limit, timeout=timeout)

def hithink_astock_selector(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-astock-selector", query, page, limit, timeout=timeout)

def hithink_macro_query(query, page="1", limit="10", timeout=60):
    return _query2data("hithink-macro-query", query, page, limit, timeout=timeout)

def news_search(query, timeout=60):
    return _comprehensive_search("news", query, timeout=timeout)

def announcement_search(query, timeout=60):
    return _comprehensive_search("announcement", query, timeout=timeout)


TOOL_DISPATCH = {
    "hithink_market_query": hithink_market_query,
    "hithink_finance_query": hithink_finance_query,
    "hithink_event_query": hithink_event_query,
    "hithink_management_query": hithink_management_query,
    "hithink_business_query": hithink_business_query,
    "hithink_industry_query": hithink_industry_query,
    "hithink_zhishu_query": hithink_zhishu_query,
    "hithink_sector_selector": hithink_sector_selector,
    "hithink_astock_selector": hithink_astock_selector,
    "hithink_macro_query": hithink_macro_query,
    "news_search": news_search,
    "announcement_search": announcement_search,
}


def _query_tool_schema(name: str, desc: str) -> dict:
    return {
        "name": name,
        "description": desc,
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "自然语言查询语句",
                },
                "page": {
                    "type": "string",
                    "description": "分页页码，默认 '1'",
                    "default": "1",
                },
                "limit": {
                    "type": "string",
                    "description": "每页条数，默认 '10'",
                    "default": "10",
                },
                "timeout": {
                    "type": "integer",
                    "description": "请求超时秒数，默认 60",
                    "default": 60,
                },
            },
            "required": ["query"],
        },
    }


def _search_tool_schema(name: str, desc: str) -> dict:
    return {
        "name": name,
        "description": desc,
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词",
                },
                "timeout": {
                    "type": "integer",
                    "description": "请求超时秒数，默认 60",
                    "default": 60,
                },
            },
            "required": ["query"],
        },
    }


TOOL_SCHEMAS = [
    _query_tool_schema("hithink_market_query", "查询【A股/ETF/指数】个股实时行情：价格、涨跌幅、成交量、主力资金流向、技术指标。⚠️ 不支持商品/贵金属价格（金价/银价/油价/现货黄金等）——这类请用 news_search 查询（能拿到实时报价+新闻）。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_finance_query", "查询个股财务数据：营业收入、净利润、ROE、ROA、负债率、PE/PB/PS等。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_event_query", "查询个股事件数据：业绩预告、增发配股、股权质押、限售解禁等。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_management_query", "查询公司股东股本信息：前十大股东、实控人、高管团队等。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_business_query", "查询公司经营数据：主营业务构成、主要客户、供应商等。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_industry_query", "查询行业数据：行业估值、财务指标、板块排名等。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_zhishu_query", "查询指数行情：上证指数、沪深300、创业板指等。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_sector_selector", "智能筛选市场板块：按估值、资金流向、涨跌幅等条件。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_astock_selector", "智能筛选A股：按行情、技术形态、财务等多条件选股。数据来源：同花顺问财。"),
    _query_tool_schema("hithink_macro_query", "查询宏观经济数据：GDP、CPI、PPI、PMI、LPR等。数据来源：同花顺问财。"),
    _search_tool_schema("news_search", "搜索财经资讯：政策动态、行业新闻、企业进展，以及【商品/贵金属价格】（金价/银价/油价/现货黄金/AU9999 等的实时报价与走势）。数据来源：同花顺问财。"),
    _search_tool_schema("announcement_search", "搜索金融公告：财报、分红、回购、资产重组等。数据来源：同花顺问财。"),
]


# ── JSON-RPC 2.0 协议处理 ────────────────────────────────────────────────

def _send(response: dict) -> None:
    sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _error(id_val, code: int, message: str) -> None:
    _send({"jsonrpc": "2.0", "id": id_val, "error": {"code": code, "message": message}})


def handle_initialize(req: dict) -> None:
    _send({
        "jsonrpc": "2.0",
        "id": req["id"],
        "result": {
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
            "capabilities": {"tools": {}},
        },
    })


def handle_tools_list(req: dict) -> None:
    _send({
        "jsonrpc": "2.0",
        "id": req["id"],
        "result": {"tools": TOOL_SCHEMAS},
    })


def handle_tools_call(req: dict) -> None:
    params = req.get("params", {})
    tool_name = params.get("name", "")
    arguments = params.get("arguments", {})

    fn = TOOL_DISPATCH.get(tool_name)
    if fn is None:
        _error(req["id"], -32601, f"未知工具: {tool_name}")
        return

    try:
        result = fn(**arguments)
        content_text = json.dumps(result, ensure_ascii=False, indent=2)
        _send({
            "jsonrpc": "2.0",
            "id": req["id"],
            "result": {
                "content": [{"type": "text", "text": content_text}],
            },
        })
    except TypeError as e:
        _error(req["id"], -32602, f"参数错误: {e}")
    except RuntimeError as e:
        _error(req["id"], -32000, str(e))
    except Exception as e:
        _error(req["id"], -32603, f"工具执行异常: {e}")


METHODS = {
    "initialize": handle_initialize,
    "tools/list": handle_tools_list,
    "tools/call": handle_tools_call,
}


def main() -> None:
    print(f"[{SERVER_NAME}] v{SERVER_VERSION} starting on stdio", file=sys.stderr)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue
        if req.get("method", "").startswith("notifications/") or "id" not in req:
            continue
        method = req.get("method", "")
        handler = METHODS.get(method)
        if handler is None:
            _error(req.get("id"), -32601, f"未知方法: {method}")
            continue
        try:
            handler(req)
        except Exception as e:
            _error(req.get("id"), -32603, f"内部错误: {e}")


if __name__ == "__main__":
    main()
