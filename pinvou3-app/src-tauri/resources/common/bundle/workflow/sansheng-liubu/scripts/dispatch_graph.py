#!/usr/bin/env python3
"""dispatch_graph.py — 差事图编译器（三省六部·尚书省派单 → 动态路由数据）

【单一职责】把尚书省的 dispatch.json（派单书）编译成一张**动态路由子图**，
供路由器(WorkflowState/scheduler)像消费静态 route_table 一样消费。引擎不感知
静态 vs 动态——它只多读一份 `_state/dynamic_routes.json` 并 union 进来。

设计见 SDAN 架构：本模块是「数据平面 → 控制平面」唯一的注入点，把尚书省的
语义判断（哪个部、第几批、干什么）翻译成路由器能吃的结构（节点/依赖/join），
**结构构造 + 校验全在这里，弱模型只管吐语义**。

核心模型（B2 差事驱动）：
  - 调度单位 = **差事(task-instance)**，不是固定的「部」。
  - 一个部可领多份差事（不同 wave）→ 同一个部在多个环节出现。
  - 同 wave 的差事 = 会办（并行）；wave 之间 = 接力（串行 barrier）。
  - 节点 id = `<bu>~<seq>`（seq 为该部第几份差事，按 assignments 顺序，1 基）。
  - 产物文件代码生成 `deliverables/<bu>_<seq>.md`（防撞名，尚书省不用编文件名）。

纯函数 + 确定性 + 自带单测（test_dispatch_graph.py）。不依赖 scheduler/engine。

CLI（materialize 步骤，harness 在尚书省 gate 通过后调一次）：
    python3 dispatch_graph.py <project_dir>
        读 <project>/dispatch.json → 写 <project>/_state/dynamic_routes.json
        校验不过 → 非零退出 + stderr 打印错误（映射成尚书省 gate FAIL）。
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SCHEMA_VERSION = 1

# 六部 id（能力真相源 = agent_registry.json 各部条目；本表只列 id 用于校验/默认动态层）。
SIX_BU = ["hubu", "libu", "bingbu", "xingbu", "gongbu", "libu_renshi"]
UPSTREAM = "shangshu"   # 差事扇出起点（尚书省派单）
JOIN_NODE = "huizou"    # 回奏：扇入等齐全部差事


class DispatchGraphError(Exception):
    """校验失败聚合错误。errors 列表逐条人话，映射成尚书省 gate FAIL 的 findings。"""

    def __init__(self, errors: list[str]):
        self.errors = errors
        super().__init__("; ".join(errors))


def compile_dispatch_graph(
    dispatch: dict,
    *,
    bu_enum: list[str] | None = None,
    upstream: str = UPSTREAM,
    join_node: str = JOIN_NODE,
) -> dict:
    """尚书省 dispatch.json → 动态路由子图（纯函数）。

    入参 dispatch: {assignments: [{bu, wave, task, requirements, title?}], skip_bus?: [...]}
    返回 dynamic_routes 字典（见模块 docstring 的 schema）。校验失败抛 DispatchGraphError。
    """
    bu_enum = bu_enum or SIX_BU
    errors: list[str] = []

    assignments = dispatch.get("assignments")
    if not isinstance(assignments, list) or not assignments:
        raise DispatchGraphError(["assignments 为空：尚书省必须至少给一个部派一份差事"])

    # ── 1. 逐条校验 + 规整 ──
    cleaned: list[dict] = []
    for i, a in enumerate(assignments):
        where = f"assignments[{i}]"
        if not isinstance(a, dict):
            errors.append(f"{where} 不是对象")
            continue
        bu = a.get("bu")
        if bu not in bu_enum:
            errors.append(f"{where}.bu={bu!r} 不是合法的部（须 ∈ {bu_enum}）")
            continue
        wave = a.get("wave", 1)
        if isinstance(wave, bool) or not isinstance(wave, int) or wave < 1:
            errors.append(f"{where}.wave={wave!r} 必须是 ≥1 的整数")
            continue
        task = (a.get("task") or "").strip()
        if not task:
            errors.append(f"{where}.task 为空（{bu} 这份差事没说要干什么）")
            continue
        cleaned.append({
            "bu": bu,
            "wave": wave,
            "task": task,
            "requirements": (a.get("requirements") or "").strip(),
            "title": (a.get("title") or "").strip(),
            "_order": i,
        })

    if errors:
        raise DispatchGraphError(errors)

    # ── 2. wave 压实（容忍弱模型跳号：{1,3,7} → {1,2,3}，保序，不报错）──
    used_waves = sorted({a["wave"] for a in cleaned})
    wave_remap = {w: idx + 1 for idx, w in enumerate(used_waves)}
    for a in cleaned:
        a["wave"] = wave_remap[a["wave"]]

    # ── 3. 建差事节点（id=<bu>~<seq>，seq 为该部第几份差事，按原始顺序）──
    bu_seq: dict[str, int] = {}
    task_nodes: dict[str, dict] = {}
    waves: dict[int, list[str]] = {}
    # active_roles 顺序：先按 wave，再按原始 assignment 顺序（稳定可复现）
    for a in sorted(cleaned, key=lambda x: (x["wave"], x["_order"])):
        bu = a["bu"]
        seq = bu_seq.get(bu, 0) + 1
        bu_seq[bu] = seq
        node_id = f"{bu}~{seq}"
        title = a["title"] or _derive_title(a["task"])
        task_nodes[node_id] = {
            "bu": bu,
            "wave": a["wave"],
            "seq": seq,
            "title": title,
            "task": a["task"],
            "requirements": a["requirements"],
            "output_file": f"deliverables/{bu}_{seq}.md",
        }
        waves.setdefault(a["wave"], []).append(node_id)

    active_roles = list(task_nodes.keys())

    # ── 4. 连边：差事 depends_on（wave1→上游；waveN→上一 wave 全部）+ 回奏 join 全差事 ──
    depends_on: dict[str, list[str]] = {}
    for node_id, spec in task_nodes.items():
        w = spec["wave"]
        if w == 1:
            depends_on[node_id] = [upstream]
        else:
            depends_on[node_id] = list(waves[w - 1])  # 上一批全部（barrier）
    depends_on[join_node] = active_roles[:]   # 回奏等齐全部差事（动态 join）

    # ── 5. skip_bus 派生（不信弱模型算集合差：没领差事的部 = skip）──
    busy = {spec["bu"] for spec in task_nodes.values()}
    skip_bus = [b for b in bu_enum if b not in busy]

    return {
        "schema_version": SCHEMA_VERSION,
        "source": f"{upstream}/dispatch.json",
        "dynamic_layer": list(bu_enum),   # 这些静态节点被差事节点取代
        "upstream": upstream,
        "join_node": join_node,
        "task_nodes": task_nodes,
        "active_roles": active_roles,
        "depends_on": depends_on,
        "waves": {str(w): ids for w, ids in sorted(waves.items())},
        "skip_bus": skip_bus,
    }


def _derive_title(task: str) -> str:
    """没给 title 时从 task 取个短描（首句/首段截断，≤12 字），仅 UI 副标题用。"""
    head = task.strip().splitlines()[0]
    for sep in ("：", ":", "。", "，", ",", " "):
        if sep in head:
            head = head.split(sep)[0]
            break
    return head[:12]


# ── CLI / materialize ──

def materialize(project_dir: str | Path) -> Path:
    """读 <project>/dispatch.json，编译，写 <project>/_state/dynamic_routes.json。返回输出路径。"""
    project = Path(project_dir).resolve()
    dispatch_path = project / "dispatch.json"
    if not dispatch_path.exists():
        raise DispatchGraphError([f"dispatch.json 不存在: {dispatch_path}"])
    with open(dispatch_path, encoding="utf-8") as f:
        dispatch = json.load(f)
    graph = compile_dispatch_graph(dispatch)
    out = project / "_state" / "dynamic_routes.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        json.dump(graph, f, ensure_ascii=False, indent=2)
    return out


def main():
    if len(sys.argv) < 2:
        print("用法: python3 dispatch_graph.py <project_dir>", file=sys.stderr)
        sys.exit(2)
    try:
        out = materialize(sys.argv[1])
    except DispatchGraphError as e:
        print("差事图编译失败:", file=sys.stderr)
        for err in e.errors:
            print(f"  - {err}", file=sys.stderr)
        sys.exit(1)
    print(json.dumps({"ok": True, "dynamic_routes": str(out)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
