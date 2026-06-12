#!/usr/bin/env python3
"""验证"pass 后重新立账"改动不会重新引入 pass↔continue 震荡(产物不变时连续点品应稳定)。
用真实 session nvk85zyp36jd0 的产物 + 首轮账目,Python 复刻 ledger_from_entries 状态机,
真模型连跑 6 轮品,看 verdict 序列是否稳定。跑:python3 scripts/pinvou_relist_sim.py
"""
import json
import pathlib
import re
import urllib.request

RS = pathlib.Path(__file__).resolve().parents[1] / "pinvou3-app/src-tauri/src/pinvou_review.rs"
SESS = pathlib.Path("/home/hexin/.pinvou3/sessions/nvk85zyp36jd0")
URL = "http://10.214.74.113:8000/v1/chat/completions"
MODEL = "qwen36_35b_256k"


def extract(name):
    m = re.search(rf'const {name}: &str = r#"(.*?)"#;', RS.read_text(), re.DOTALL)
    return m.group(1)


PROMPT = extract("PROMPT")
RECONCILE = extract("RECONCILE_PROMPT")
product = (SESS / "workspace/欧洲9天亲子游规划.md").read_text()[:9000]
sidecar = json.load(open(SESS / "pinvou_reviews.json"))
first_round = sidecar[0]["review"]          # 首轮立账(3 笔)
PATH = first_round["artifact_path"]


def call(prompt, ctx):
    body = json.dumps({
        "model": MODEL,
        "messages": [{"role": "system", "content": prompt}, {"role": "user", "content": ctx}],
        "temperature": 0, "max_tokens": 1600, "chat_template_kwargs": {"enable_thinking": False},
    }).encode()
    req = urllib.request.Request(URL, data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        txt = json.loads(r.read())["choices"][0]["message"]["content"]
    if "```" in txt:
        txt = next((p for p in txt.split("```") if p.strip().lstrip("json").strip().startswith("{")), txt)
        txt = txt.strip().lstrip("json").strip()
    return json.loads(txt)


def pick_ledger(entries, current_pos):
    """Python 复刻 Rust ledger_from_entries:最近 pass+产物改过(current_pos>那轮 pos)→None 重新立账;
    否则读最近一轮立账。连点品(current_pos==pass 的 pos,产物没变)→核最近立账、稳定。"""
    latest = next((e for e in reversed(entries) if e["review"].get("artifact_path") == PATH), None)
    if latest is None:
        return None
    if latest["review"].get("verdict") == "pass" and current_pos > latest.get("pos", 0):
        return None  # 产物改过 → 重新立账审增量
    for e in reversed(entries):  # 读最近一轮立账(最近 issues 非空)
        r = e["review"]
        if r.get("artifact_path") != PATH or not r.get("issues"):
            continue
        return [i for i in r["issues"] if i.get("resolution") not in ("accept", "confirmed")]
    return None


# 初始:首轮立账(pos=15);AI 已按它改好产物(product=改后态)。连点品时 messages 数固定=POS。
POS = 31
entries = [{"pos": 15, "review": first_round}]
need = "【Boss 需求】带 2 大 1 小去欧洲玩 9 天亲子游,舒适型预算。"

print(f"首轮账目({len(first_round.get('issues',[]))}笔):", [i["text"][:18] for i in first_round.get("issues", [])])
print("\n连续点品 6 轮(产物全程不变,current_pos 固定),看 verdict 是否稳定:\n")
seq = []
for rnd in range(6):
    ledger = pick_ledger(entries, POS)
    if ledger is None:
        mode, prompt = "首轮重审", PROMPT
        ctx = f"{need}\n\n【AI 应对】(产物)\n{product}"
    else:
        mode, prompt = "核账", RECONCILE
        acc = "\n".join(f"{i}. [{x['severity']}] {x['text']}" for i, x in enumerate(ledger))
        ctx = f"【上轮账目】\n{acc}\n\n【当前产物 `{PATH}`】\n{product}"
    try:
        rv = call(prompt, ctx)
    except Exception as ex:
        print(f"  品{rnd+1} 调用失败:{ex}")
        break
    verdict = rv.get("verdict")
    issues = rv.get("issues", [])
    seq.append(verdict if verdict else f"立账{len(issues)}笔")
    print(f"  品{rnd+1} [{mode}] → verdict={verdict} issues={len(issues)}  {[i.get('text','')[:18] for i in issues[:3]]}")
    entries.append({"pos": POS, "review": {"artifact_path": PATH, "verdict": verdict, "issues": issues}})

print(f"\nverdict 序列: {seq}")
tail = seq[2:] if len(seq) >= 3 else seq
stable = len(set(tail)) <= 1
print(f"=> 后几轮稳定(不震荡)? {'✓ 收敛' if stable else '✗ 仍在 '+str(set(tail))+' 间横跳'}")
