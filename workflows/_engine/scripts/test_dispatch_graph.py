#!/usr/bin/env python3
"""差事图编译器单测。纯逻辑,无 I/O。跑: python3 test_dispatch_graph.py"""
from __future__ import annotations

from dispatch_graph import compile_dispatch_graph, DispatchGraphError, _derive_title


def _ok(name):
    print(f"  ✓ {name}")


def test_basic_single_per_bu():
    """每部一份差事、单 wave → 节点 = 部#1,全并行依赖尚书省,回奏等齐。"""
    d = {"assignments": [
        {"bu": "bingbu", "wave": 1, "task": "调研现状", "requirements": "带来源"},
        {"bu": "hubu", "wave": 1, "task": "数据统计", "requirements": ""},
    ]}
    g = compile_dispatch_graph(d)
    assert set(g["task_nodes"]) == {"bingbu~1", "hubu~1"}, g["task_nodes"].keys()
    assert g["depends_on"]["bingbu~1"] == ["shangshu"]
    assert g["depends_on"]["hubu~1"] == ["shangshu"]
    assert set(g["depends_on"]["huizou"]) == {"bingbu~1", "hubu~1"}
    assert g["task_nodes"]["bingbu~1"]["output_file"] == "deliverables/bingbu_1.md"
    assert g["skip_bus"] == ["libu", "xingbu", "gongbu", "libu_renshi"]
    _ok("basic: 每部一份单 wave → 并行 + 回奏全等")


def test_multi_appearance():
    """一个部多次出场(兵部 wave1 + wave3)→ 两个节点 bingbu~1 / bingbu~2。"""
    d = {"assignments": [
        {"bu": "bingbu", "wave": 1, "task": "初勘", "requirements": ""},
        {"bu": "hubu", "wave": 1, "task": "统计", "requirements": ""},
        {"bu": "xingbu", "wave": 2, "task": "质检上一批", "requirements": ""},
        {"bu": "bingbu", "wave": 3, "task": "复核结论", "requirements": ""},
    ]}
    g = compile_dispatch_graph(d)
    assert "bingbu~1" in g["task_nodes"] and "bingbu~2" in g["task_nodes"]
    assert g["task_nodes"]["bingbu~1"]["wave"] == 1
    assert g["task_nodes"]["bingbu~2"]["wave"] == 3
    assert g["task_nodes"]["bingbu~2"]["output_file"] == "deliverables/bingbu_2.md"
    _ok("multi: 兵部两次出场 → bingbu~1(w1)/bingbu~2(w3)")


def test_wave_barrier_deps():
    """wave N 依赖 wave N-1 全部(会办内并行,批间串行 barrier)。"""
    d = {"assignments": [
        {"bu": "bingbu", "wave": 1, "task": "a", "requirements": ""},
        {"bu": "hubu", "wave": 1, "task": "b", "requirements": ""},
        {"bu": "xingbu", "wave": 2, "task": "c", "requirements": ""},
        {"bu": "bingbu", "wave": 3, "task": "d", "requirements": ""},
    ]}
    g = compile_dispatch_graph(d)
    # wave1 → 上游
    assert g["depends_on"]["bingbu~1"] == ["shangshu"]
    assert g["depends_on"]["hubu~1"] == ["shangshu"]
    # wave2(xingbu~1)→ wave1 全部
    assert set(g["depends_on"]["xingbu~1"]) == {"bingbu~1", "hubu~1"}
    # wave3(bingbu~2)→ wave2 全部
    assert g["depends_on"]["bingbu~2"] == ["xingbu~1"]
    _ok("barrier: waveN depends_on wave(N-1) 全部")


def test_same_wave_parallel():
    """同 wave 多部 = 会办,彼此无依赖,都只依赖上游(并行)。"""
    d = {"assignments": [
        {"bu": "bingbu", "wave": 1, "task": "a", "requirements": ""},
        {"bu": "hubu", "wave": 1, "task": "b", "requirements": ""},
        {"bu": "gongbu", "wave": 1, "task": "c", "requirements": ""},
    ]}
    g = compile_dispatch_graph(d)
    assert g["waves"]["1"] == ["bingbu~1", "hubu~1", "gongbu~1"]
    for nid in g["waves"]["1"]:
        assert g["depends_on"][nid] == ["shangshu"], nid
    _ok("parallel: 同 wave 会办,互不依赖")


def test_wave_compaction():
    """弱模型跳号 wave {1,3,7} → 压实成 {1,2,3},保序,不报错。"""
    d = {"assignments": [
        {"bu": "bingbu", "wave": 1, "task": "a", "requirements": ""},
        {"bu": "hubu", "wave": 3, "task": "b", "requirements": ""},
        {"bu": "xingbu", "wave": 7, "task": "c", "requirements": ""},
    ]}
    g = compile_dispatch_graph(d)
    assert g["task_nodes"]["bingbu~1"]["wave"] == 1
    assert g["task_nodes"]["hubu~1"]["wave"] == 2
    assert g["task_nodes"]["xingbu~1"]["wave"] == 3
    assert set(g["waves"]) == {"1", "2", "3"}
    _ok("compact: wave 跳号 {1,3,7} → {1,2,3}")


def test_skip_bus_derived():
    """skip_bus 由代码派生(没领差事的部),不信尚书省自己算集合差。"""
    d = {"assignments": [{"bu": "hubu", "wave": 1, "task": "x", "requirements": ""}]}
    g = compile_dispatch_graph(d)
    assert "hubu" not in g["skip_bus"]
    assert set(g["skip_bus"]) == {"libu", "bingbu", "xingbu", "gongbu", "libu_renshi"}
    _ok("skip: skip_bus 派生 = 六部 - 已派")


def test_title_explicit_and_derived():
    d = {"assignments": [
        {"bu": "hubu", "wave": 1, "task": "做一份很长很长的财务统计报表给上面看", "requirements": "", "title": "财算"},
        {"bu": "bingbu", "wave": 1, "task": "调研AI Agent现状：覆盖框架与平台", "requirements": ""},
    ]}
    g = compile_dispatch_graph(d)
    assert g["task_nodes"]["hubu~1"]["title"] == "财算"           # 显式优先
    assert g["task_nodes"]["bingbu~1"]["title"] == "调研AI Agent现状"  # 派生:首句截断
    _ok("title: 显式优先,缺则从 task 派生短描")


def test_validation_errors():
    # 空 assignments
    try:
        compile_dispatch_graph({"assignments": []})
        assert False, "空 assignments 应报错"
    except DispatchGraphError as e:
        assert "为空" in e.errors[0]
    # 非法 bu
    try:
        compile_dispatch_graph({"assignments": [{"bu": "haha", "wave": 1, "task": "x", "requirements": ""}]})
        assert False, "非法 bu 应报错"
    except DispatchGraphError as e:
        assert any("不是合法的部" in x for x in e.errors)
    # wave 非正整数
    try:
        compile_dispatch_graph({"assignments": [{"bu": "hubu", "wave": 0, "task": "x", "requirements": ""}]})
        assert False, "wave=0 应报错"
    except DispatchGraphError as e:
        assert any("wave" in x for x in e.errors)
    # task 空
    try:
        compile_dispatch_graph({"assignments": [{"bu": "hubu", "wave": 1, "task": "  ", "requirements": ""}]})
        assert False, "task 空应报错"
    except DispatchGraphError as e:
        assert any("task 为空" in x for x in e.errors)
    _ok("validation: 空/非法bu/wave非正/task空 全部打回")


def test_active_roles_order_stable():
    """active_roles 顺序 = 先 wave 后原始顺序,稳定可复现。"""
    d = {"assignments": [
        {"bu": "bingbu", "wave": 2, "task": "a", "requirements": ""},
        {"bu": "hubu", "wave": 1, "task": "b", "requirements": ""},
        {"bu": "gongbu", "wave": 1, "task": "c", "requirements": ""},
    ]}
    g = compile_dispatch_graph(d)
    assert g["active_roles"] == ["hubu~1", "gongbu~1", "bingbu~1"], g["active_roles"]
    _ok("order: active_roles 按 (wave, 原序) 稳定排列")


if __name__ == "__main__":
    print("差事图编译器单测:")
    fns = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    for fn in fns:
        fn()
    print(f"\n全部 {len(fns)} 项通过 ✅")
