#!/usr/bin/env python3
"""
pinvou3 天气 MCP server — 包装高德地图天气 API（零第三方依赖，纯 stdlib）。

用法：由 DeepSeek-TUI MCP client 通过 stdio 启动。
配置：~/.deepseek/mcp.json 中注册，AMAP_KEY 通过 env 传入。

协议：newline-delimited JSON-RPC 2.0 over stdio。
LLM 可见工具名：mcp_weather_get_weather
"""
import io
import json
import os
import sys
import urllib.error
import urllib.request

# Windows 默认 stdout/stdin 编码为 GBK，MCP 协议要求 UTF-8
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")

AMAP_KEY = os.environ.get("AMAP_KEY", "")
AMAP_BASE = "https://restapi.amap.com/v3"
MAX_FORECAST_DAYS = 4
WEEK_LABELS = {"1": "周一", "2": "周二", "3": "周三", "4": "周四", "5": "周五", "6": "周六", "7": "周日"}
ICON_MAP = [
    ("晴", "sunny"),
    ("多云", "partly_cloudy"),
    ("阴", "cloudy"),
    ("雷", "thunderstorm"),
    ("暴雨", "heavy_rain"),
    ("大雨", "heavy_rain"),
    ("雨", "rainy"),
    ("雪", "snowy"),
    ("雾", "foggy"),
    ("霾", "foggy"),
]

TOOL_DEF = {
    "name": "get_weather",
    "description": (
        "查询城市天气。city 为空则默认北京；days 是 JSON 数组表示距今天的偏移"
        "（0=今天 1=明天），最多4天。"
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "city": {
                "type": "string",
                "description": "城市名称，默认北京",
            },
            "days": {
                "type": "string",
                "description": "JSON 数组，距今天的偏移天数，如 [0] 今天，[0,1,2] 今明后三天",
                "default": "[0]",
            },
        },
        "required": [],
    },
}


def _icon(weather):
    for keyword, code in ICON_MAP:
        if keyword in weather:
            return code
    return "partly_cloudy"


def _week(val):
    return WEEK_LABELS.get(val.strip(), val)


def _parse_days(raw):
    if not raw:
        return [0]
    try:
        arr = json.loads(raw) if isinstance(raw, str) else raw
        if isinstance(arr, int):
            arr = [arr]
        return sorted(set(d for d in arr if isinstance(d, int) and 0 <= d < MAX_FORECAST_DAYS)) or [0]
    except Exception:
        return [0]


def _query_mode(days):
    if len(days) > 1:
        return "multi"
    if days[0] == 0:
        return "today"
    return "single_day"


def _http_get(url, params):
    query = urllib.request.urlencode(params)
    full_url = f"{url}?{query}"
    req = urllib.request.Request(full_url)
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read().decode("utf-8"))


def get_weather(city="", days="[0]"):
    if not AMAP_KEY:
        return {"error": "AMAP_KEY 未配置"}

    city = city.strip() or "北京"
    day_list = _parse_days(days)
    mode = _query_mode(day_list)

    try:
        base_data = _http_get(f"{AMAP_BASE}/weather/weatherInfo",
                              {"key": AMAP_KEY, "city": city, "extensions": "base"})
        all_data = _http_get(f"{AMAP_BASE}/weather/weatherInfo",
                             {"key": AMAP_KEY, "city": city, "extensions": "all"})
    except Exception as e:
        return {"error": "高德 API 请求失败: %s" % e}

    lives = base_data.get("lives") or []
    if not lives:
        return {"error": "未找到城市 '%s' 的天气数据" % city}

    live = lives[0]
    temperature = live.get("temperature", "")
    weather = live.get("weather", "")
    humidity = live.get("humidity", "")

    if not temperature or not weather:
        return {"error": "天气数据不完整"}

    forecasts = all_data.get("forecasts") or []
    casts = forecasts[0].get("casts", []) if forecasts else []

    high_temp = temperature
    low_temp = temperature
    if casts:
        high_temp = casts[0].get("daytemp", temperature)
        low_temp = casts[0].get("nighttemp", temperature)

    daily = []
    selected = day_list if mode != "today" or len(day_list) > 1 else list(range(len(casts)))
    for idx in selected:
        if idx >= len(casts):
            continue
        c = casts[idx]
        day_weather = c.get("dayweather", "")
        daily.append({
            "date": c.get("date", ""),
            "week": _week(c.get("week", "")),
            "dayWeather": day_weather,
            "nightWeather": c.get("nightweather", ""),
            "dayTemp": c.get("daytemp", ""),
            "nightTemp": c.get("nighttemp", ""),
            "iconCode": _icon(day_weather),
        })

    return {
        "type": "weather",
        "city": live.get("city", city),
        "province": live.get("province", ""),
        "weather": weather,
        "temperature": temperature,
        "humidity": humidity,
        "highTemp": high_temp,
        "lowTemp": low_temp,
        "windDirection": live.get("winddirection", ""),
        "windPower": live.get("windpower", ""),
        "queryMode": mode,
        "daily": daily,
    }


# ── JSON-RPC 2.0 协议处理 ────────────────────────────────────────────────

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
            "serverInfo": {"name": "pinvou3-weather", "version": "1.0.0"},
        })
    elif method == "tools/list":
        _result(req_id, {"tools": [TOOL_DEF]})
    elif method == "tools/call":
        params = msg.get("params") or {}
        if params.get("name") == "get_weather":
            args = params.get("arguments") or {}
            result = get_weather(**args)
            content_text = json.dumps(result, ensure_ascii=False)
            _result(req_id, {
                "content": [{"type": "text", "text": content_text}],
            })
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
            continue
        try:
            _handle(msg)
        except Exception as e:
            rid = msg.get("id") if isinstance(msg, dict) else None
            if rid is not None:
                _error(rid, -32603, "internal error: %s" % e)


if __name__ == "__main__":
    main()
