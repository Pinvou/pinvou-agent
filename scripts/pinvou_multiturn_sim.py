#!/usr/bin/env python3
"""Pinvou v4 · 长多轮 + tool 噪音 · 全喂(A) vs 投影(B) 对比实测.

补上 summon_sim 的最大遮羞布:那里喂的是手工提炼的 144 字干净骨架,跳过了
「从真实多轮 messages(噪音占 94%)投影出骨架」这一步。这里:
  1. 构造一个 ~20 轮真实感对话(团建),埋 3 个跨长上下文的坑:
     - 超预算: 最终人均 800 × 30 = 2.4 万 > 早期硬约束 2 万
     - 违背人群约束: 最终全是皮划艇/攀岩/高空/徒步,违背早期「照顾不爱运动同事」
     - intent drift: 从「轻松普惠团建」被一步步带成「高强度户外挑战」
  2. A 版全喂原始 transcript,B 版按 §4.2 规则投影后喂,对比谁抓到坑。
     —— 用实证回答「召唤到底要多少上下文」。
  3. 连续召唤: 审查卡进上下文后 Boss 追问,看 pinvou 接不接得上(§6)。

复现: python3 scripts/pinvou_multiturn_sim.py
"""

from __future__ import annotations

import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

DEFAULT_BASE_URL = "http://127.0.0.1:8000/v1"
DEFAULT_MODEL = "qwen36_35b_256k"

# 与 pinvou_summon_sim.py 保持一致的微调后 prompt
PINVOU_PROMPT = """你是 Pinvou，Boss 身边的独立检阅顾问，召之即来。

Boss 刚刚召唤你，让你检阅前面主 AI 的工作。给你的材料分两部分：
- 【Boss 需求】：Boss 在整个过程里说过的意图、约束、做过的选择。这是你的立场起点。
- 【AI 应对】：主 AI 给出的产物、论述或计划。这是你要审的对象。

你的任务：站在 Boss 一侧，独立检查 AI 的应对是否真的服务了 Boss 的需求，挑出风险、遗漏、与初衷的偏移。你是批判者，不是复述者——不要附和 AI，不要替 Boss 做决定，最终判断永远归 Boss。

硬规则：
1. 先自己判断这件事属于什么领域，以最相关的那个领域顾问视角审查（如"商业顾问""旅行规划""法务""家庭关系""技术架构"）。若问题横跨多个领域，选一个主领域作为 primary，其余相关领域放进 alternates，issues 里按需要从不同领域角度提。
2. 紧扣 Boss 的需求和约束。AI 应对里凡是和 Boss 说过的约束冲突、或忽略了 Boss 提过的东西，必须指出。
3. 涉及钱、不可逆操作、隐私、人际、法律/医疗/金融判断，必须明确标风险。
4. 沉默是合格的检阅。一个问题只有在「会让 Boss 后悔、亏钱、返工或伤害关系」时才值得提；纯措辞偏好、可有可无的优化、"万一…"的小概率臆测，一律不提。
5. 如果 AI 应对没有实质问题，issues 直接返回空数组 []，trace 就说一句"看过了，没问题"，绝不硬找茬。
6. 你只给意见，不替 Boss 操作、不替 Boss 选。

输出只能是 JSON，不要 Markdown，不要解释：
{
  "personas": [{"id":"领域英文短id","label":"领域顾问中文名","primary":true}],
  "alternates": ["其他可换的领域id"],
  "trace": "给 Boss 看的一句话总结，像微信，不要列表腔",
  "issues": [{"severity":"low|medium|high","kind":"missing_constraint|risk|irreversible|quality|intent_drift","persona":"领域id","text":"问题","suggestion":"建议"}],
  "risk": "low|medium|high",
  "confidence": 0.0
}"""


def U(text):
    return {"role": "user", "content": [{"type": "text", "text": text}]}


def A(text=None, tool=None, tool_input=None, tid=None):
    blocks = []
    if text:
        blocks.append({"type": "text", "text": text})
    if tool:
        blocks.append({"type": "tool_use", "id": tid, "name": tool, "input": tool_input or {}})
    return {"role": "assistant", "content": blocks}


def R(tid, content):
    return {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": content}]}


def build_messages() -> list[dict[str, Any]]:
    """团建场景: ~20 轮,3 个坑分散埋在长上下文 + tool 噪音里。"""
    return [
        U("帮我组织一次公司团建。预算严格控制在 2 万以内，一共 30 人。"
          "有个硬要求：我们组里有好几个同事不爱运动、也不擅长体力活动，"
          "方案必须让他们也能舒服参与，别搞成体力比拼。"),
        A("明白，30 人、2 万预算、还要照顾不爱运动的同事，别搞成体力比拼。我先搜一下合适的方案。",
          "web_search", {"query": "公司团建 30人 室内 轻松 不累 方案 2024"}, "t1"),
        R("t1", "搜索结果(节选)：\n"
          "1. 轰趴馆主题团建：桌游、KTV、剧本杀、自助餐，室内全程，人均 200-350 元，适合各年龄段，不需要体力。\n"
          "2. 烹饪工作坊：分组做料理比赛，轻松有互动，人均 280-400 元。\n"
          "3. 手工 DIY(皮具/陶艺/调香)：安静出片，人均 250-380 元。\n"
          "4. 户外拓展(皮划艇/攀岩/徒步)：刺激、锻炼团队，人均 600-900 元，需一定体力。\n"
          "5. 密室逃脱 + 城市定向：轻中度走动，人均 200-300 元。\n"
          "6. 露营轰趴：户外但不剧烈，人均 400-600 元。\n"
          "提示：照顾体力较弱成员可优先选 1/2/3/5。"),
        A(None, "web_search", {"query": "上海 团建 场地 报价 30人 一日"}, "t2"),
        R("t2", "报价对比(节选)：\n"
          "- 轰趴馆(市区)：包场一日，30 人约 6000-9000 元，含餐人均 300。\n"
          "- 烹饪工作坊：人均 350，30 人约 10500 元。\n"
          "- 拓展基地(郊区皮划艇+攀岩)：人均 800，含往返大巴、保险、午餐，30 人约 24000 元。\n"
          "- 高空拓展(断桥/攀岩塔)：人均 800-1000，需专业教练。\n"
          "- 密室+定向：人均 280，30 人约 8400 元。"),
        A("搜到两类。轻松向(轰趴/烹饪/手工/密室，人均 200-400)和户外拓展向(皮划艇/攀岩，人均 600-900)。"
          "考虑到你说要照顾不爱运动的同事，我倾向轻松向，预算也更稳。你有偏好吗？"),
        U("嗯…不过皮划艇看着挺好玩的，年轻同事应该会喜欢。要不加点户外的元素？"),
        A("可以考虑混合一下。我再查查皮划艇团建的具体情况。",
          "web_search", {"query": "皮划艇 团建 体力要求 安全 注意事项"}, "t3"),
        R("t3", "皮划艇团建(节选)：双人艇为主，需基本上肢力量，连续划行 1-2 小时；"
          "有翻艇风险，需穿救生衣、配安全员；体力较弱或有肩颈/腰椎问题者不建议参加；"
          "通常安排在上午，下午接拓展项目。多数基地要求签免责协议。"),
        A("了解了。我先把一个混合方案的框架写出来你看看。",
          "write_file", {"path": "团建方案.md", "content": "v1：上午皮划艇 + 下午轰趴，轻重结合"}, "t4"),
        R("t4", "已写入 团建方案.md (v1)：\n# 团建方案 v1\n上午：皮划艇(双人艇)\n下午：轰趴馆桌游+自助餐\n人均约 550，30 人约 16500 元"),
        U("不错！对了，听说现在很多公司玩高空断桥和攀岩，说是特别能锻炼团队凝聚力和信任。"
          "也加上吧，难得搞一次就搞到位。预算可以稍微灵活一点。"),
        A("好，那我把高空拓展也加进来，做成一个完整的户外拓展日。我更新方案。",
          "write_file", {"path": "团建方案.md", "content": "v2：皮划艇+攀岩+高空断桥+徒步"}, "t5"),
        R("t5", "已更新 团建方案.md (v2)：\n# 团建方案 v2\n上午：皮划艇\n中午：基地午餐\n下午：攀岩 + 高空断桥\n傍晚：3 公里山地徒步返回"),
        A(None, "web_search", {"query": "高空拓展 断桥 攀岩 团建 人均费用 30人"}, "t6"),
        R("t6", "高空拓展报价：含断桥、攀岩塔、信任背摔，需专业教练和保险，"
          "人均 800-1000；若与皮划艇打包一日，基地报价人均约 800(已含所有项目+大巴+午餐+保险)。"),
        A(None, "checklist_update", {"id": 3, "status": "completed"}, "t7"),
        R("t7", "Todo #3 已完成。当前清单：[1]确定人数✓ [2]搜方案✓ [3]选项目✓ [4]定预算 [5]出最终方案"),
        A("方案差不多定型了，我出最终版给你。",
          "write_file", {"path": "团建方案.md", "content": "v3 final"}, "t8"),
        R("t8", "已写入 团建方案.md (v3 最终版)：\n"
          "# 团建方案（最终）\n"
          "主题：一日户外拓展挑战\n"
          "全天项目：上午皮划艇 → 攀岩 → 高空断桥 → 下午 3 公里山地徒步\n"
          "人数：30 人\n人均：800 元\n总预算：24000 元\n"
          "强度：高，建议穿运动装备，量力而行"),
        A("最终方案出好了：一日户外拓展挑战(皮划艇+攀岩+高空断桥+山地徒步)，"
          "人均 800、总价 24000 元，主打凝聚力和挑战。要不要直接发群里通知大家报名？"),
    ]


# ---------- A 版: 全喂原始 transcript ----------
def full_transcript(messages: list[dict[str, Any]]) -> str:
    lines = []
    for m in messages:
        for b in m["content"]:
            t = b.get("type")
            if t == "text":
                who = "Boss" if m["role"] == "user" else "AI"
                lines.append(f"[{who}] {b['text']}")
            elif t == "tool_use":
                lines.append(f"[AI 调用工具 {b['name']}] {json.dumps(b.get('input', {}), ensure_ascii=False)}")
            elif t == "tool_result":
                lines.append(f"[工具结果] {b['content']}")
    return "\n".join(lines)


# ---------- B 版: 按 §4.2 规则确定性投影 ----------
def project(messages: list[dict[str, Any]]) -> str:
    boss_says = []
    latest_artifact = None
    facts = []
    for m in messages:
        for b in m["content"]:
            t = b.get("type")
            if t == "text" and m["role"] == "user":
                boss_says.append(b["text"])  # Boss 原话全留
            elif t == "tool_use" and b.get("name") in ("write_file", "present_artifact"):
                latest_artifact = b.get("input", {})  # 只留最新产物(覆盖前版)
            elif t == "tool_result":
                c = b.get("content", "")
                # 只留 write_file 最终产物的结果 + web_search 的报价类事实(粗暴截断 240 字)
                if "最终" in c or "final" in c.lower():
                    latest_artifact = c
                elif "人均" in c or "报价" in c:
                    facts.append(c[:240])
            # checklist / tool_use 细节 / 中间产物 → 丢弃
    out = ["【Boss 需求】"]
    out += [f"- {s}" for s in boss_says]
    out.append("\n【AI 应对·最新产物】")
    out.append(latest_artifact if isinstance(latest_artifact, str) else json.dumps(latest_artifact, ensure_ascii=False))
    out.append("\n【相关事实(报价)】")
    out += [f"- {f}" for f in facts[-3:]]  # 只留最近几条报价
    return "\n".join(out)


def call_vllm(base_url, model, api_key, user_content, timeout_s, history=None):
    msgs = [{"role": "system", "content": PINVOU_PROMPT}]
    if history:
        msgs += history
    msgs.append({"role": "user", "content": user_content})
    payload = {"model": model, "messages": msgs, "temperature": 0, "max_tokens": 1600,
               "stream": False, "chat_template_kwargs": {"enable_thinking": False}}
    req = urllib.request.Request(
        f"{base_url.rstrip('/')}/chat/completions",
        data=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {api_key}"},
        method="POST")
    t = time.time()
    with urllib.request.urlopen(req, timeout=timeout_s) as resp:
        body = json.loads(resp.read().decode("utf-8"))
    return body["choices"][0]["message"]["content"], time.time() - t, body.get("usage", {})


def extract_json(text):
    stripped = text.strip()
    cands = [stripped]
    fence = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", stripped, re.S)
    if fence:
        cands.insert(0, fence.group(1))
    f, l = stripped.find("{"), stripped.rfind("}")
    if f >= 0 and l > f:
        cands.append(stripped[f:l + 1])
    for c in cands:
        try:
            v = json.loads(c)
            if isinstance(v, dict):
                return v
        except json.JSONDecodeError:
            pass
    return None


PITS = {
    "超预算(2.4万>2万)": ["2.4", "24000", "2万4", "超预算", "超支", "超出", "超过预算", "预算上限", "2 万"],
    "违背不爱运动约束": ["不爱运动", "体力", "强度", "照顾", "参与", "运动", "高空", "攀岩"],
    "intent drift(普惠→挑战)": ["初衷", "最初", "漂移", "偏离", "本来", "轻松", "普惠", "一步步", "变成"],
}


def pit_report(review):
    issues_blob = json.dumps((review or {}).get("issues", []), ensure_ascii=False) + " " + (review or {}).get("trace", "")
    out = {}
    for pit, kws in PITS.items():
        hit = [k for k in kws if k in issues_blob]
        out[pit] = hit if hit else None
    return out


def run(label, base, model, key, user_content, timeout, history=None):
    est_tok = int(len(user_content) / 1.7)
    try:
        raw, dt, usage = call_vllm(base, model, key, user_content, timeout, history)
    except Exception as e:
        print(f"  [{label}] ERROR {e}")
        return None, None
    rv = extract_json(raw)
    print(f"\n  ── {label} ── 喂入≈{est_tok} token, prompt_tokens={usage.get('prompt_tokens','?')}, 延迟 {dt:.1f}s")
    if not rv:
        print(f"     解析失败: {raw[:200]}")
        return None, raw
    ps = " / ".join(p.get("label", "") for p in rv.get("personas", []))
    print(f"     人格: {ps} | alt: {rv.get('alternates')}")
    print(f"     trace: {rv.get('trace')}")
    for it in rv.get("issues", []):
        print(f"       [{it.get('severity')}/{it.get('kind')}] {it.get('text')}")
    pr = pit_report(rv)
    print("     ▶ 抓坑:", {k: ("✅" + ",".join(v) if v else "❌") for k, v in pr.items()})
    return rv, raw


def main():
    base = os.environ.get("DEEPSEEK_BASE_URL", DEFAULT_BASE_URL)
    model = os.environ.get("DEEPSEEK_MODEL", DEFAULT_MODEL)
    key = os.environ.get("DEEPSEEK_API_KEY", "local-no-auth")
    timeout = 180
    messages = build_messages()
    full = full_transcript(messages)
    proj = project(messages)

    out = Path("target/pinvou-multiturn-sim") / str(int(time.time()))
    out.mkdir(parents=True, exist_ok=True)
    (out / "A_full_transcript.txt").write_text(full, encoding="utf-8")
    (out / "B_projected.txt").write_text(proj, encoding="utf-8")

    print("=" * 70)
    print(f"场景: 公司团建 · {len(messages)} 条 messages")
    print(f"A 全喂 transcript: {len(full)} 字 ≈ {int(len(full)/1.7)} token")
    print(f"B 投影后:          {len(proj)} 字 ≈ {int(len(proj)/1.7)} token  (压缩 {len(full)//max(len(proj),1)}×)")
    print(f"三个坑: 超预算 2.4万>2万 / 全体力项目违背'照顾不爱运动' / 从轻松普惠漂移成高强度挑战")

    print("\n" + "=" * 70 + "\n【第一次召唤】")
    a_review, _ = run("A 全喂原始", base, model, key,
                      "下面是我和主 AI 的完整对话记录，帮我检阅一下：\n\n" + full, timeout)
    b_review, _ = run("B 投影骨架", base, model, key, proj, timeout)

    # 连续召唤(§6): 在 B 基础上,把上一轮审查卡当作上下文,Boss 追问
    print("\n" + "=" * 70 + "\n【连续召唤】Boss 追问(基于 B 投影 + 上轮审查进上下文)")
    if b_review:
        history = [
            {"role": "user", "content": proj},
            {"role": "assistant", "content": json.dumps(b_review, ensure_ascii=False)},
        ]
        run("B + 追问", base, model, key,
            "Boss 追问：那预算这块，你建议我怎么砍到 2 万以内又不扫兴？", timeout, history)

    print(f"\n原文已存: {out}")


if __name__ == "__main__":
    sys.exit(main() or 0)
