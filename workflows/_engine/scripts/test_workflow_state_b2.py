#!/usr/bin/env python3
"""B2 集成测试:WorkflowState 消费 dynamic_routes.json,差事节点波次推进。
跑真 route_table.json + agent_registry.json(sansheng_liubu 场景)。
用法: python3 test_workflow_state_b2.py"""
from __future__ import annotations

import json
import shutil
import tempfile
from pathlib import Path

import workflow_state as _W
from workflow_state import WorkflowState
from dispatch_graph import compile_dispatch_graph

# [引擎/数据分离] 引擎源(_engine/)上一级没有 route_table——装配后才在工作流目录里。
# 在源位置直接跑测试时,把数据指向 sansheng-liubu(本仓唯一真数据)。
if not Path(_W.ROUTE_TABLE_JSON).exists():
    _DATA = Path(__file__).resolve().parent.parent.parent / "sansheng-liubu"
    _W.ROUTE_TABLE_JSON = _DATA / "route_table.json"
    _W.REGISTRY_JSON = _DATA / "agent_registry.json"

MAIN_CHAIN = ["taizi", "zhongshu", "menxia", "shangshu"]  # 太子 + 三省主链


def _setup(tmp: str, dispatch: dict):
    """造一个「尚书省刚派完单」的项目:主链全 completed + dynamic_routes.json 落盘。"""
    state_dir = Path(tmp) / "_state"
    state_dir.mkdir(parents=True, exist_ok=True)
    # 先 init 静态 state,把主链标 completed(模拟跑到尚书省之后)
    ws = WorkflowState(tmp, scenario="sansheng_liubu")
    for r in MAIN_CHAIN:
        ws._state["roles"][r]["status"] = "completed"
    ws.save()
    # 编译派单 → dynamic_routes.json
    graph = compile_dispatch_graph(dispatch)
    with open(state_dir / "dynamic_routes.json", "w", encoding="utf-8") as f:
        json.dump(graph, f, ensure_ascii=False)


def test_wave_progression():
    tmp = tempfile.mkdtemp()
    try:
        dispatch = {"assignments": [
            {"bu": "bingbu", "wave": 1, "task": "初勘调研", "requirements": ""},
            {"bu": "hubu", "wave": 1, "task": "数据统计", "requirements": ""},
            {"bu": "xingbu", "wave": 2, "task": "质检上一批", "requirements": ""},
            {"bu": "bingbu", "wave": 3, "task": "复核结论", "requirements": ""},
        ]}
        _setup(tmp, dispatch)
        ws = WorkflowState(tmp, scenario="sansheng_liubu")

        # 差事节点进了 active,静态六部被取代
        assert "bingbu~1" in ws._active_roles and "bingbu~2" in ws._active_roles
        assert "hubu" not in ws._active_roles and "bingbu" not in ws._active_roles
        # 回奏仍在,在最后
        assert ws._active_roles[-1] == "huizou"

        # 尚书省 completed → 第1批会办(并行)可办
        assert set(ws.next_actionable()) == {"bingbu~1", "hubu~1"}
        # 只办完一个 → 回奏还不能动(要等齐全部差事)
        ws.complete_role("bingbu~1")
        assert "huizou" not in ws.next_actionable()
        # 办完第1批 → 第2批
        ws.complete_role("hubu~1")
        assert set(ws.next_actionable()) == {"xingbu~1"}
        # 第2批 → 第3批(兵部再出场)
        ws.complete_role("xingbu~1")
        assert set(ws.next_actionable()) == {"bingbu~2"}
        # 全差事办完 → 回奏(动态 join 等齐)
        ws.complete_role("bingbu~2")
        assert set(ws.next_actionable()) == {"huizou"}
        # 回奏办完 → 全工作流完成
        ws.complete_role("huizou")
        assert ws.all_completed()
        print("  ✓ 波次推进: wave1会办 → wave2 → wave3兵部再战 → 回奏等齐 → 完成")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def test_role_view_inherits_bu():
    """差事节点能力按部继承,name=部·差事标题,outputs=专属文件。"""
    tmp = tempfile.mkdtemp()
    try:
        _setup(tmp, {"assignments": [
            {"bu": "bingbu", "wave": 1, "task": "调研AI现状", "requirements": "", "title": "初勘"},
        ]})
        ws = WorkflowState(tmp, scenario="sansheng_liubu")
        view = ws._roles_by_id["bingbu~1"]
        assert view["name"] == "兵部·初勘", view["name"]
        assert view["outputs"] == ["deliverables/bingbu_1.md"]
        assert view["gate"] == ws._reg["bingbu"].get("gate", "auto")  # 继承兵部 gate
        # 差事的输入应含上游尚书省产物 dispatch.json
        assert "dispatch.json" in view["inputs"]
        print("  ✓ 差事节点: 能力继承部 + name=部·标题 + 专属产物 + 继承上游输入")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def test_static_sansheng_unchanged():
    """没有 dynamic_routes.json → 静态六部照旧,差事层不激活(向后兼容)。"""
    tmp = tempfile.mkdtemp()
    try:
        ws = WorkflowState(tmp, scenario="sansheng_liubu")
        assert "hubu" in ws._active_roles and "libu_renshi" in ws._active_roles
        assert ws._task_nodes == {}
        assert not any("#" in r for r in ws._active_roles)
        print("  ✓ 无 dynamic_routes: sansheng 静态六部行为不变")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)



# (test_other_scenarios_unchanged 已删:solution_deck 场景随 h3c-ppt 2026-06-11 存档)

if __name__ == "__main__":
    print("B2 WorkflowState 集成测试:")
    fns = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    for fn in fns:
        fn()
    print(f"\n全部 {len(fns)} 项通过 ✅")
