# -*- coding: utf-8 -*-
"""market_query 商品价格识别/重定向(方案 B)单测。纯函数、无网络。pytest 或 `python test_rewrite.py` 均可。"""
import importlib.util
import os

_spec = importlib.util.spec_from_file_location(
    "iw_server", os.path.join(os.path.dirname(__file__), "server.py")
)
_iw = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_iw)
is_commodity = _iw._is_commodity_price_query

# (query, 是否应判为"商品价格→重定向 news_search")
CASES = [
    # —— 商品/贵金属价格:应重定向 ——
    ("金价", True),
    ("今天金价多少", True),
    ("国际现货黄金 最新价格 涨跌幅", True),     # 模型真实传的长 query
    ("现货黄金 AU9999 上海金 最新价格", True),
    ("现货黄金 XAUUSD 当前价格", True),
    ("银价走势", True),
    ("油价多少", True),
    ("comex 黄金", True),
    ("贵金属行情", True),
    # —— A股/个股:不拦,正常走问财行情 ——
    ("贵州茅台股价", False),
    ("山东黄金股价", False),            # 黄金股,含"股"→放行
    ("黄金概念股有哪些", False),         # 含"股"
    ("宁德时代行情", False),
    ("沪深300指数", False),
    ("半导体板块走势", False),
]


def test_commodity_guard():
    fails = []
    for q, exp in CASES:
        got = is_commodity(q)
        if got != exp:
            fails.append(f"  {q!r} -> {got}  (expect {exp})")
    assert not fails, "判定错误:\n" + "\n".join(fails)


if __name__ == "__main__":
    ok = True
    for q, exp in CASES:
        got = is_commodity(q)
        flag = "✓" if got == exp else "✗"
        if got != exp:
            ok = False
        tag = "→news_search" if got else "→问财行情"
        print(f"{flag} {q!r:28s} {tag:14s} (expect {'重定向' if exp else '放行'})")
    print("\n全部通过 ✅" if ok else "\n有失败 ❌")
