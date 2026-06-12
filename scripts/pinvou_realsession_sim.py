#!/usr/bin/env python3
"""Pinvou v4 · 真实 session 压力测试 · 全喂(A) vs 投影(B).

团建场景(1238 token)还没逼出"全喂被淹没"。这里直接拿真实完成态 session
(巴黎旅行, 30 条 messages, ~1.8万 token, 94% 是 tool 噪音)做极端压力测试:
  - A 全喂: 整个 transcript 塞进去, 看解析稳不稳、延迟多少、会不会被海量
    web_search dump 带偏而乱挑刺(§1 少说在极端噪音下保不保持)。
  - B 投影: 按 §4.2 确定性规则抽骨架, 看投影 token 量 + 质量。
真实 session 是正常完成的、没埋坑, 所以重点不是"抓坑"而是"极端长度下还撑不撑得住"。

复现: python3 scripts/pinvou_realsession_sim.py [session.json]
"""

from __future__ import annotations

import json
import os
import re
import sys
import time
import urllib.request
from pathlib import Path
from typing import Any

DEFAULT_BASE_URL = "http://10.214.74.113:8000/v1"
DEFAULT_MODEL = "qwen36_35b_256k"
DEFAULT_SESSION = "/home/hexin/.pinvou3/sessions/g4y7jw7pz4jd0.json"

from importlib import util as _util
_spec = _util.spec_from_file_location("mt", str(Path(__file__).with_name("pinvou_multiturn_sim.py")))
_mt = _util.module_from_spec(_spec); sys.modules["mt"] = _mt; _spec.loader.exec_module(_mt)
PINVOU_PROMPT = _mt.PINVOU_PROMPT
call_vllm = _mt.call_vllm
extract_json = _mt.extract_json


def tool_name_map(messages):
    m = {}
    for msg in messages:
        c = msg.get("content")
        if isinstance(c, list):
            for b in c:
                if isinstance(b, dict) and b.get("type") == "tool_use":
                    m[b.get("id")] = b.get("name")
    return m


def full_transcript(messages):
    names = tool_name_map(messages)
    lines = []
    for msg in messages:
        c = msg.get("content")
        if not isinstance(c, list):
            if isinstance(c, str):
                lines.append(f"[{ 'Boss' if msg['role']=='user' else 'AI'}] {c}")
            continue
        for b in c:
            if not isinstance(b, dict):
                continue
            t = b.get("type")
            if t == "text":
                lines.append(f"[{'Boss' if msg['role']=='user' else 'AI'}] {b.get('text','')}")
            elif t == "tool_use":
                lines.append(f"[AI 调用 {b.get('name')}] {json.dumps(b.get('input',{}),ensure_ascii=False)[:400]}")
            elif t == "tool_result":
                nm = names.get(b.get("tool_use_id"), "?")
                cont = b.get("content", "")
                if not isinstance(cont, str):
                    cont = json.dumps(cont, ensure_ascii=False)
                lines.append(f"[工具结果·{nm}] {cont}")
    return "\n".join(lines)


def project(messages):
    """§4.2 确定性投影: Boss原话全留 / request_user_input决策 / 最新产物 / web_search事实截断 / 丢噪音。"""
    names = tool_name_map(messages)
    boss_says, decisions, facts = [], [], []
    latest_artifact = None
    for msg in messages:
        c = msg.get("content")
        if not isinstance(c, list):
            continue
        for b in c:
            if not isinstance(b, dict):
                continue
            t = b.get("type")
            if t == "text" and msg["role"] == "user":
                boss_says.append(b.get("text", ""))
            elif t == "tool_use" and b.get("name") in ("write_file", "present_artifact"):
                latest_artifact = b.get("input", {})  # 最新产物(覆盖)
            elif t == "tool_result":
                nm = names.get(b.get("tool_use_id"), "?")
                cont = b.get("content", "")
                if not isinstance(cont, str):
                    cont = json.dumps(cont, ensure_ascii=False)
                if nm == "request_user_input":
                    # Boss 的选择 = 强需求信号
                    try:
                        ans = json.loads(cont).get("answers", [])
                        decisions += [f"{a.get('id')}={a.get('label')}" for a in ans]
                    except Exception:
                        pass
                elif nm == "web_search":
                    facts.append(cont[:200])  # 粗暴截断
                # checklist / 中间 write_file result / tool_use 细节 → 丢
    out = ["【Boss 需求】"]
    out += [f"- {s}" for s in boss_says if s.strip()]
    if decisions:
        out.append("【Boss 已确认的选择】")
        out += [f"- {d}" for d in decisions]
    out.append("\n【AI 应对·最新产物】")
    if isinstance(latest_artifact, dict):
        out.append(json.dumps(latest_artifact, ensure_ascii=False)[:3000])
    else:
        out.append(str(latest_artifact)[:3000])
    out.append("\n【相关事实(搜索摘录)】")
    out += [f"- {f}" for f in facts[:4]]
    return "\n".join(out)


def run(label, base, model, key, content, timeout):
    try:
        raw, dt, usage = call_vllm(base, model, key, content, timeout)
    except Exception as e:
        print(f"\n  ── {label} ── ERROR after timeout: {e}")
        return
    rv = extract_json(raw)
    print(f"\n  ── {label} ── prompt_tokens={usage.get('prompt_tokens','?')}, "
          f"completion={usage.get('completion_tokens','?')}, 延迟 {dt:.1f}s, 解析={'OK' if rv else '失败'}")
    if not rv:
        print(f"     裸输出尾部: ...{raw[-180:]}")
        return
    print(f"     人格: {' / '.join(p.get('label','') for p in rv.get('personas',[]))} | risk={rv.get('risk')}")
    print(f"     trace: {rv.get('trace')}")
    for it in rv.get("issues", []):
        print(f"       [{it.get('severity')}/{it.get('kind')}] {it.get('text')}")


def main():
    base = os.environ.get("DEEPSEEK_BASE_URL", DEFAULT_BASE_URL)
    model = os.environ.get("DEEPSEEK_MODEL", DEFAULT_MODEL)
    key = os.environ.get("DEEPSEEK_API_KEY", "local-no-auth")
    path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_SESSION
    messages = json.load(open(path))["messages"]
    full = full_transcript(messages)
    proj = project(messages)
    out = Path("target/pinvou-realsession-sim") / str(int(time.time()))
    out.mkdir(parents=True, exist_ok=True)
    (out / "A_full.txt").write_text(full, encoding="utf-8")
    (out / "B_proj.txt").write_text(proj, encoding="utf-8")

    print("=" * 70)
    print(f"真实 session: {path}")
    print(f"{len(messages)} 条 messages")
    print(f"A 全喂: {len(full):,} 字 ≈ {int(len(full)/1.7):,} token")
    print(f"B 投影: {len(proj):,} 字 ≈ {int(len(proj)/1.7):,} token  (压缩 {len(full)//max(len(proj),1)}×)")
    print("(此 session 正常完成、未埋坑 → 看极端长度下解析稳不稳、会不会被噪音带偏乱挑刺)")

    run("A 全喂原始 1.8万token", base, model, key,
        "下面是我和主 AI 的完整对话记录，帮我检阅：\n\n" + full, 180)
    run("B 投影骨架", base, model, key, proj, 180)
    print(f"\n原文已存: {out}")


if __name__ == "__main__":
    sys.exit(main() or 0)
