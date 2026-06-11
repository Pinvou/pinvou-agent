#!/usr/bin/env python3
"""Pinvou v4 · 立账核账收敛机制验证(设计文档 §3)。

用真实不收敛 session tejz7cxrd5jd0 验证:核账模式 prompt 能不能治好它的四个出血点
(幻觉权威/无证据翻案/永久挂账/无终态)。
- 首轮账目 = 该 session #1 召唤立的 3 条 issues(预算/交通/签证)。
- 当前产物 = V5(最终版)。
- 核账模式 prompt 注入账目 + 禁新增 + 终态规则,看 pinvou:
  ① 只核这 3 条(标 resolved/unresolved/withdrawn) ② 不新增(不提孩子年龄、不翻回程)
  ③ 外部事实标「需核实」不当硬伤 ④ 全闭合则输出 verdict=pass「通过,可交付」。

复现: python3 scripts/pinvou_reconcile_sim.py
"""
from __future__ import annotations
import json, urllib.request, time

BASE = "http://10.214.74.113:8000/v1"
MODEL = "qwen36_35b_256k"
SID = "tejz7cxrd5jd0"

# 核账模式 prompt —— 把设计 §3 的规则文案化
RECONCILE_PROMPT = """你是 Pinvou，Boss 身边的独立检阅顾问。这是对**同一产出物**的复审（核账模式），不是重新自由批评。

给你两样东西：
- 【上轮账目】：上一次检阅立下的问题清单（每条是当时认定的问题）。
- 【当前产物】：主 AI 按账目修订后的最新版本。

你的任务是**核账**，严格遵守：
1. 逐条核对上轮账目、对照当前产物，给每条标闭合状态：
   - resolved：已改好
   - unresolved：没改 / 没改对（说明还差什么）
   - withdrawn：这条当初就提错了，撤回（比如基于错误事实）
2. **禁止新增问题**。唯一例外：本次修订在它改动的段落内新引入的错误（必须指明）。不要把"还能更好"的优化、或上轮没提过的新角度算进来。
3. 已结的账不要翻，除非你有**新证据**。
4. 外部事实（交通班次/票价/签证政策）你没有外部知识，只能在 note 里标「需核实」，**不得当硬伤反复要求修改**。
5. **终态**：若所有账目都已闭合（resolved 或 withdrawn），必须输出 verdict="pass"、trace 写"通过，可交付"。

输出只能是 JSON：
{
  "verdict": "pass|continue",
  "trace": "给 Boss 的一句话，像微信",
  "ledger": [{"id": "对应上轮账目序号", "status": "resolved|unresolved|withdrawn", "note": "一句说明，外部事实标『需核实』"}],
  "new_issues": [{"severity": "low|medium|high", "text": "仅本次修订引入的新错误，通常为空", "suggestion": "建议"}],
  "risk": "low|medium|high"
}"""


def call(prompt, user):
    payload = {"model": MODEL, "messages": [{"role": "system", "content": prompt}, {"role": "user", "content": user}],
               "temperature": 0, "max_tokens": 1200, "stream": False, "chat_template_kwargs": {"enable_thinking": False}}
    req = urllib.request.Request(f"{BASE}/chat/completions", data=json.dumps(payload, ensure_ascii=False).encode(),
                                 headers={"Content-Type": "application/json", "Authorization": "Bearer x"}, method="POST")
    t = time.time()
    with urllib.request.urlopen(req, timeout=120) as r:
        body = json.loads(r.read())
    return body["choices"][0]["message"]["content"], time.time() - t


def extract(text):
    import re
    for c in [text.strip(), (re.search(r"\{.*\}", text, re.S) or [None])[0]]:
        if not c:
            continue
        try:
            return json.loads(c)
        except Exception:
            pass
    f, l = text.find("{"), text.rfind("}")
    return json.loads(text[f:l + 1]) if f >= 0 and l > f else None


def main():
    pr = json.load(open(f"/home/hexin/.pinvou3/sessions/{SID}/pinvou_reviews.json"))
    first = pr[1]["review"]  # #1 是首轮实质账目(#0 是空召唤)
    ledger = [{"id": i, "severity": it.get("severity"), "text": it.get("text")} for i, it in enumerate(first.get("issues", []))]
    v5 = open(f"/home/hexin/.pinvou3/sessions/{SID}/workspace/欧洲9天8夜旅行计划_V5.md").read()

    user = "【上轮账目】\n" + "\n".join(f"{x['id']}. [{x['severity']}] {x['text']}" for x in ledger)
    user += f"\n\n【当前产物 V5】\n{v5[:30000]}"
    print(f"首轮账目 {len(ledger)} 条(预算/交通/签证),当前产物 V5 {len(v5)}字\n喂入≈{int(len(user)/1.7)} token")

    raw, dt = call(RECONCILE_PROMPT, user)
    print(f"延迟 {dt:.1f}s\n" + "=" * 60)
    rv = extract(raw)
    if not rv:
        print("解析失败:", raw[:300]); return 1
    print("verdict:", rv.get("verdict"), "| risk:", rv.get("risk"))
    print("trace:", rv.get("trace"))
    print("\n核账结果(应只核首轮 3 条):")
    for x in rv.get("ledger", []):
        print(f"  账目#{x.get('id')} [{x.get('status')}] {x.get('note', '')[:80]}")
    ni = rv.get("new_issues", [])
    print(f"\n新增问题(应≈空,禁新增): {len(ni)} 条")
    for x in ni:
        print(f"  [{x.get('severity')}] {x.get('text', '')[:80]}")

    blob = json.dumps(rv, ensure_ascii=False)
    print("\n=== 收敛检查(对照四个出血点) ===")
    print(f"① 不暴增条例(核账≤3+new≤1)? {'✓' if len(rv.get('ledger',[]))<=4 and len(ni)<=1 else '✗'}")
    print(f"② 不再挂账孩子年龄(首轮没立)? {'✗ 又提了' if '年龄' in blob else '✓ 没提'}")
    print(f"③ 外部事实(交通)标需核实/不当硬伤? {'✓' if '核实' in blob else '⚠️ 看 note'}")
    print(f"④ 有明确终态 verdict? {'✓ '+rv.get('verdict','') if rv.get('verdict') in ('pass','continue') else '✗ 无终态'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
