#!/usr/bin/env python3
"""覆盖镜头(coverage)全场景验证:验证一条【领域无关】的元 prompt 能否让 Qwen3.6 按场景
临场生成贴合行业惯例的"完整性维度框架",再对照产物抓准覆盖缺口。命根子=框架质量+跨场景稳。
故意挑差异极大的三场景(旅行/技术架构/商业BP),各埋好"该有却没写"的短板,看模型抓不抓得到。
跑:python3 scripts/pinvou_coverage_sim.py
"""
import json
import sys
import urllib.request

URL = "http://10.214.74.113:8000/v1/chat/completions"
MODEL = "qwen36_35b_256k"

# 领域无关的元指令:不写任何领域的维度清单,只让模型以自判专家身份临场列框架+比对覆盖度。
COVERAGE_PROMPT = """你是 Pinvou，Boss 身边的独立检阅顾问。这次专做【覆盖度检查】——不挑已有内容的对错，只看产物"全不全"。

给你 Boss 的需求 + 主 AI 的产物。两步走：
1. 以你**自判的领域专家身份**，先想清楚：**这一类产物**要算完整、合格、能交付，行业惯例本该覆盖哪些维度？列出这个领域的完整性维度框架（贴合行业惯例，别硬套别的领域）。
2. 对照产物，逐个维度看覆盖度。只把**薄弱/缺失**的维度列进 gaps，说明缺什么、建议补什么；已经齐的不用列。

硬规则：
- 维度框架必须贴合这类产物自己的行业惯例。
- 核心维度（该有必须有的）缺失 → severity="high"；加分维度（锦上添花）缺失 → severity="low"。
- 克制：最多列 5-7 个最重要的缺口，按 severity 排序。
- 只指"缺哪些维度"，不挑"已写内容对不对"（那是另一套镜头）。

输出只能是 JSON：
{
  "persona": {"id":"领域英文短id","label":"领域顾问中文名"},
  "framework": ["这类产物完整该覆盖的维度，逐个列"],
  "gaps": [{"dimension":"维度名","coverage":"weak|missing","severity":"high|medium|low","text":"缺/薄弱在哪","suggestion":"建议补什么"}]
}"""


def call(prompt, ctx):
    body = json.dumps({
        "model": MODEL,
        "messages": [{"role": "system", "content": prompt}, {"role": "user", "content": ctx}],
        "temperature": 0, "max_tokens": 1800,
        "chat_template_kwargs": {"enable_thinking": False},
    }).encode()
    req = urllib.request.Request(URL, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=70) as r:
        txt = json.loads(r.read())["choices"][0]["message"]["content"]
    if "```" in txt:
        txt = next((p for p in txt.split("```") if p.strip().lstrip("json").strip().startswith("{")), txt)
        txt = txt.strip().lstrip("json").strip()
    return json.loads(txt)


SCENES = [
    {
        "name": "旅行计划",
        "ctx": """【Boss 需求】带 6 岁孩子去欧洲玩 9 天，舒适型预算，第一次出国。
【AI 产物】《意大利 9 天亲子攻略》：D1-D9 每日行程（罗马斗兽场/梵蒂冈、佛罗伦萨、威尼斯的景点+亲子餐厅），含城际交通衔接（高铁车次）、每晚住宿推荐。
（埋的短板：没有签证办理流程/时间线、没有保险与应急医疗信息、预算只写"约4.8万"没有分项、没有雨天/孩子生病的备选方案、没有证件清单）""",
        "expect_framework": ["签证", "保险", "应急", "预算"],
        "expect_gaps": ["签证", "应急", "预算", "备选"],
    },
    {
        "name": "技术架构",
        "ctx": """【Boss 需求】设计一个电商订单服务的后端架构，给开发团队照着实现。
【AI 产物】《订单服务架构设计》：技术栈选型（Go + PostgreSQL + Redis）、对外 API 接口列表、核心数据库表结构、用 Kubernetes 部署。
（埋的短板：没有容错/降级/限流、没有鉴权与数据安全、没有监控告警/可观测性、没有容量与性能估算、没有数据一致性/事务方案）""",
        "expect_framework": ["容错", "安全", "监控", "性能"],
        "expect_gaps": ["容错", "安全", "监控", "一致"],
    },
    {
        "name": "商业BP",
        "ctx": """【Boss 需求】写一份社区团购 App 的商业计划书，要拿去见投资人。
【AI 产物】《社区团购 BP》：产品介绍、目标用户画像、核心功能模块、团队成员背景、市场规模（万亿赛道）。
（埋的短板：没有竞品分析、没有财务模型/盈利测算、没有获客成本与单位经济模型、没有风险与应对、没有融资需求与资金用途）""",
        "expect_framework": ["竞品", "财务", "风险", "获客"],
        "expect_gaps": ["竞品", "财务", "单位经济", "风险"],
    },
]


def hit(keys, text):
    return [k for k in keys if k in text]


def run():
    for s in SCENES:
        print(f"\n{'='*64}\n场景：{s['name']}\n{'='*64}")
        try:
            rv = call(COVERAGE_PROMPT, s["ctx"])
        except Exception as e:
            print(f"  ✗ 调用/解析失败：{e}")
            continue
        per = rv.get("persona", {})
        fw = rv.get("framework", [])
        gaps = rv.get("gaps", [])
        print(f"自判人格：{per.get('label','?')} ({per.get('id','?')})")
        print(f"临场框架({len(fw)}维)：{ '、'.join(fw) }")
        print("抓到的缺口：")
        for g in gaps:
            print(f"  · [{g.get('severity')}/{g.get('coverage')}] {g.get('dimension','')}：{g.get('text','')[:46]}")
        fw_text = " ".join(fw) + " " + " ".join(g.get("dimension", "") for g in gaps)
        gap_text = " ".join((g.get("dimension", "") + g.get("text", "")) for g in gaps)
        fw_hit = hit(s["expect_framework"], fw_text)
        gap_hit = hit(s["expect_gaps"], gap_text)
        print("── 自动判定 ──")
        print(f"  框架覆盖该领域核心维度：{len(fw_hit)}/{len(s['expect_framework'])} {fw_hit}")
        print(f"  抓到预埋短板：{len(gap_hit)}/{len(s['expect_gaps'])} {gap_hit}")
        core_high = [g for g in gaps if g.get("severity") == "high"]
        print(f"  核心缺口标 high：{len(core_high)} 个")
        ok = len(fw_hit) >= len(s["expect_framework"]) - 1 and len(gap_hit) >= len(s["expect_gaps"]) - 1
        print(f"  => {'✓ 框架贴合+缺口抓准' if ok else '✗ 需看输出细节'}")


if __name__ == "__main__":
    run()
