#!/usr/bin/env python3
"""验证 2026-06-11 三处修复的后端行为(真实模型,PROMPT 直接从 Rust 提取保证同源):
  #12 首轮 PROMPT 分工:决策点/缺信息进 recommendations、产物缺陷进 issues、不重叠;
       且 recommendations 涉及外部事实不硬推未核实项。
  #13 RECONCILE_PROMPT 核账 pass 时 trace 逐条交代各账目核对结论。
跑:python3 scripts/pinvou_v4_fixes_sim.py
"""
import json
import pathlib
import re
import sys
import urllib.request

RS = pathlib.Path(__file__).resolve().parents[1] / "pinvou3-app/src-tauri/src/features/review/mod.rs"
URL = "http://127.0.0.1:8000/v1/chat/completions"
MODEL = "qwen36_35b_256k"


def extract(name: str) -> str:
    m = re.search(rf'const {name}: &str = r#"(.*?)"#;', RS.read_text(), re.DOTALL)
    if not m:
        sys.exit(f"提取不到 {name}")
    return m.group(1)


def call(prompt: str, ctx: str) -> dict:
    body = json.dumps({
        "model": MODEL,
        "messages": [{"role": "system", "content": prompt}, {"role": "user", "content": ctx}],
        "temperature": 0, "max_tokens": 1600,
        "chat_template_kwargs": {"enable_thinking": False},
    }).encode()
    req = urllib.request.Request(URL, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        txt = json.loads(r.read())["choices"][0]["message"]["content"]
    # 容错:可能裹 ```json
    if "```" in txt:
        txt = next((p for p in txt.split("```") if p.strip().lstrip("json").strip().startswith("{")), txt)
        txt = txt.strip().lstrip("json").strip()
    return json.loads(txt)


PROMPT = extract("PROMPT")
RECONCILE = extract("RECONCILE_PROMPT")

# ── 场景1:首轮,AI 方案含缺陷(交通错误)+ 决策点(日期/同行人没定)+ 外部事实(直飞) ──
FIRST_CTX = """【Boss 需求】
Boss：我要去欧洲9天8夜，帮我规划。
（AI 问过出发地/偏好/预算，Boss 答：中国出发、经典城市打卡、舒适型）

【AI 应对】(产物 europe.md)
# 欧洲9天8夜 · 法国瑞士线
Day1-4 巴黎。
Day5 巴黎 → 琉森：乘火车经米卢斯中转，约2小时直达琉森。
Day6-8 少女峰区域。
预算：人均2-3万。
签证：办法国申根签证即可。
巴黎-威尼斯段如需可直飞，约2小时。
（AI 自始至终没问 Boss 具体哪个月/哪天出发，也没确认同行人是情侣还是带老人小孩）
"""

# ── 场景2:核账,上轮账目已改好 ──
RECONCILE_CTX = """【上轮账目】
0. [high] Day5 巴黎→米卢斯→琉森交通不可行：需多次换乘且米卢斯非枢纽
1. [medium] 出发日期未锁定，无法查实时票价

【当前产物 `europe.md`】
# 欧洲9天8夜 · 法国瑞士线
> 建议出发日期：5月15日-25日 或 9月15日-25日
Day5 巴黎 → 苏黎世：方案A 巴黎CDG直飞苏黎世约1.5h；方案B 巴黎东站TGV直达苏黎世约4h。
Day6-8 少女峰区域。
签证：办法国申根签证（主停留国原则）。
"""


def show(title, rv):
    print(f"\n{'='*60}\n{title}\n{'='*60}")
    recs = rv.get("recommendations", [])
    iss = rv.get("issues", [])
    print(f"trace: {rv.get('trace','')}")
    print(f"verdict: {rv.get('verdict')}")
    print(f"\nrecommendations({len(recs)}):")
    for r in recs:
        print(f"  · {r.get('topic','')} → {r.get('pick','')}  | why: {r.get('why','')[:60]}")
    print(f"issues({len(iss)}):")
    for i in iss:
        print(f"  · [{i.get('severity')}/{i.get('kind')}] {i.get('text','')[:70]}")
    return recs, iss


print("场景1:首轮——分工 + 需核实")
recs, iss = show("首轮检阅", call(PROMPT, FIRST_CTX))
rec_topics = " ".join((r.get("topic", "") + r.get("pick", "")) for r in recs)
iss_text = " ".join(i.get("text", "") for i in iss)
print("\n── 自动判定 ──")
print(f"  决策点(日期/同行人)进 recommendations? {'✓' if ('日期' in rec_topics or '同行' in rec_topics or '出发' in rec_topics) else '✗'}")
print(f"  缺陷(交通)进 issues? {'✓' if ('米卢斯' in iss_text or '交通' in iss_text or '琉森' in iss_text) else '✗'}")
dup = ("日期" in iss_text or "同行" in iss_text)
print(f"  决策点没在 issues 重复? {'✓' if not dup else '✗ 重复了:'+iss_text[:50]}")
# 真正该拦的:把外部事实"断言不存在/取消"当硬伤(误杀对的方案),而非笼统匹配关键词。
hard_flight = [i for i in iss if i.get("kind") in ("quality", "risk", "irreversible")
               and any(w in i.get("text", "") for w in ["无直飞", "不存在直飞", "直飞已取消", "没有直飞", "无此航班"])]
print(f"  外部事实(直飞)没被当硬伤误杀? {'✓' if not hard_flight else '✗ '+str([i['text'][:40] for i in hard_flight])}")
push_flight = [r for r in recs if "优先" in r.get("pick", "") and ("直飞" in r.get("pick", ""))]
print(f"  recommendations 没硬推未核实直飞? {'✓' if not push_flight else '✗ '+str(push_flight)}")

print("\n\n场景2:核账——pass 逐条留痕")
rv2 = call(RECONCILE, RECONCILE_CTX)
show("核账", rv2)
tr = rv2.get("trace", "")
print("\n── 自动判定 ──")
print(f"  verdict=pass? {'✓' if rv2.get('verdict')=='pass' else '✗ '+str(rv2.get('verdict'))}")
itemized = sum(1 for kw in ["交通", "苏黎世", "日期", "5月", "直飞"] if kw in tr) >= 2
print(f"  trace 逐条交代核对结论(非空泛'通过')? {'✓' if itemized else '✗ trace='+tr}")
