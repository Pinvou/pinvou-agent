#!/usr/bin/env python3
"""workflow_logger.py — 工作流双层日志

两层日志分开记录：
- flow_log.jsonl  — agent 间的流转：谁交给谁、交付物、通过/打回
- agent_log.jsonl — agent 内的执行：调了什么、写了什么、重试几次、有没有原地转圈

harness 每次 step 后调 log_flow()，agent 执行过程中调 log_agent()。
"""

import json
import sys
from datetime import datetime, timezone, timedelta
from pathlib import Path

TZ_CN = timezone(timedelta(hours=8))


def _now() -> str:
    return datetime.now(TZ_CN).isoformat(timespec="seconds")


def _append(path: Path, record: dict):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps(record, ensure_ascii=False) + "\n")


def log_flow(project_dir: str, event: str, **kwargs):
    """记录 agent 间的流转事件。

    event 类型：
    - dispatch: 派发角色
    - complete: 角色完成
    - gate_pass: gate 检查通过
    - gate_fail: gate 检查失败
    - review_pass: 品悟评审通过
    - review_fail: 品悟评审未通过
    - rollback: 回滚
    - human_gate: 等待用户审批
    - human_approve: 用户审批通过
    - human_reject: 用户审批拒绝
    - workflow_complete: 工作流完成
    """
    record = {
        "timestamp": _now(),
        "layer": "flow",
        "event": event,
        **kwargs,
    }
    _append(Path(project_dir) / "_state" / "flow_log.jsonl", record)


def log_agent(project_dir: str, role_id: str, event: str, **kwargs):
    """记录 agent 内部的执行事件。

    event 类型：
    - start: 开始执行
    - tool_call: 调了一个工具
    - file_write: 写了一个文件
    - retry: 重试
    - loop_warning: 检测到可能的原地转圈
    - finish: 执行结束
    """
    record = {
        "timestamp": _now(),
        "layer": "agent",
        "role_id": role_id,
        "event": event,
        **kwargs,
    }
    _append(Path(project_dir) / "_state" / "agent_log.jsonl", record)


def detect_loop(project_dir: str, role_id: str, window: int = 5) -> dict:
    """检测 agent 是否在原地转圈。

    读最近 window 条同角色的 agent_log，检查：
    1. 连续 retry 次数
    2. 是否在重复相同的 tool_call
    3. 是否没有新文件产出
    """
    log_path = Path(project_dir) / "_state" / "agent_log.jsonl"
    if not log_path.exists():
        return {"looping": False}

    recent = []
    with open(log_path, encoding="utf-8") as f:
        for line in f:
            try:
                r = json.loads(line)
                if r.get("role_id") == role_id:
                    recent.append(r)
            except json.JSONDecodeError:
                continue

    recent = recent[-window:]
    if not recent:
        return {"looping": False}

    # 检查 1：连续 retry
    retries = sum(1 for r in recent if r.get("event") == "retry")
    if retries >= 3:
        return {
            "looping": True,
            "reason": f"连续 {retries} 次重试",
            "suggestion": "可能遇到无法自动修复的问题，建议人工介入",
        }

    # 检查 2：重复 tool_call
    tool_calls = [r.get("tool") for r in recent if r.get("event") == "tool_call"]
    if len(tool_calls) >= 3 and len(set(tool_calls)) == 1:
        return {
            "looping": True,
            "reason": f"反复调用同一个工具: {tool_calls[0]}",
            "suggestion": "工具可能返回错误但 agent 没有换策略",
        }

    # 检查 3：没有文件产出
    writes = [r for r in recent if r.get("event") == "file_write"]
    if len(recent) >= window and not writes:
        return {
            "looping": True,
            "reason": f"最近 {window} 条记录没有文件产出",
            "suggestion": "agent 可能在思考但没有实际行动",
        }

    return {"looping": False}


def summary(project_dir: str) -> dict:
    """生成双层日志摘要。"""
    project = Path(project_dir)
    flow_log = project / "_state" / "flow_log.jsonl"
    agent_log = project / "_state" / "agent_log.jsonl"

    flow_events = []
    if flow_log.exists():
        with open(flow_log, encoding="utf-8") as f:
            for line in f:
                try:
                    flow_events.append(json.loads(line))
                except json.JSONDecodeError:
                    continue

    agent_events = []
    if agent_log.exists():
        with open(agent_log, encoding="utf-8") as f:
            for line in f:
                try:
                    agent_events.append(json.loads(line))
                except json.JSONDecodeError:
                    continue

    # 按角色聚合
    role_stats = {}
    for e in agent_events:
        rid = e.get("role_id", "unknown")
        if rid not in role_stats:
            role_stats[rid] = {"tool_calls": 0, "file_writes": 0, "retries": 0}
        if e.get("event") == "tool_call":
            role_stats[rid]["tool_calls"] += 1
        elif e.get("event") == "file_write":
            role_stats[rid]["file_writes"] += 1
        elif e.get("event") == "retry":
            role_stats[rid]["retries"] += 1

    return {
        "flow_events": len(flow_events),
        "agent_events": len(agent_events),
        "dispatches": sum(1 for e in flow_events if e.get("event") == "dispatch"),
        "completes": sum(1 for e in flow_events if e.get("event") == "complete"),
        "rollbacks": sum(1 for e in flow_events if e.get("event") == "rollback"),
        "gate_fails": sum(1 for e in flow_events if e.get("event") == "gate_fail"),
        "role_stats": role_stats,
    }


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(f"用法: {sys.argv[0]} <project_dir> [--summary]", file=sys.stderr)
        sys.exit(1)
    if "--summary" in sys.argv:
        print(json.dumps(summary(sys.argv[1]), ensure_ascii=False, indent=2))
    else:
        print(json.dumps(summary(sys.argv[1]), ensure_ascii=False, indent=2))
