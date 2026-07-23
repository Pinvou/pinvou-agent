#!/usr/bin/env python3
"""scheduler.py — SDAN 工作流调度器(通用引擎,工作流无关)

工作流调度器核心逻辑（Harness Loop 的工具层）：
1. 读状态机，找到可执行角色
2. 选最高优先级角色
3. 加载该角色的 prompt 模板
4. 组装完整 prompt（角色指令 + 输入文件摘要 + 上下文）
5. 返回 prompt 供 Harness Loop 派给 SubAgent
6. 执行完毕后跑 gate，更新状态

这个脚本不直接跟 LLM 对话——它是 Harness Loop 调用的工具层。
Harness Loop 拿到 scheduler 输出的 prompt 后，spawn SubAgent 执行。
所有工作流知识来自数据(agent_registry.json / route_table.json / roles/),
本文件不许出现具体工作流的角色名/场景名/产物路径。

用法:
    # 获取下一步该做什么
    python3 scheduler.py /path/to/project --scenario sansheng_liubu --next

    # 获取某个角色的完整 prompt
    python3 scheduler.py /path/to/project --scenario sansheng_liubu --prompt taizi

    # 标记角色完成 / 可重试失败 / 不可重试失败
    python3 scheduler.py /path/to/project --scenario sansheng_liubu --complete taizi
    python3 scheduler.py /path/to/project --scenario sansheng_liubu --fail taizi --reason "..."
    python3 scheduler.py /path/to/project --scenario sansheng_liubu --fail-fatal taizi --reason "HTTP 401"

    # 应用回滚规则(violation_type 来自该工作流 route_table 的 rollback_rules)
    python3 scheduler.py /path/to/project --scenario sansheng_liubu --rollback fengbo

    # 查看完整状态摘要
    python3 scheduler.py /path/to/project --scenario sansheng_liubu --status
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# Windows 安装包使用 CPython embeddable runtime。其 `python*._pth` 会启用隔离
# 模式并忽略 PYTHONPATH，因此不能依赖父进程注入脚本目录。显式把当前脚本目录
# 放进 sys.path，确保直接执行 scheduler.py 时能导入同目录 workflow_state.py。
SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from workflow_state import WorkflowState

WORKFLOW_ROOT = SCRIPTS_DIR.parent
ROLES_DIR = WORKFLOW_ROOT / "roles"


def load_role_prompt(role_id: str, prompt_file: str | None = None) -> str:
    """加载角色 prompt 模板。优先用 registry 声明的 prompt_file（能力真相源），
    缺省再回退约定路径 roles/<role_id>.md。"""
    f = (WORKFLOW_ROOT / prompt_file) if prompt_file else (ROLES_DIR / f"{role_id}.md")
    if not f.exists():
        return f"[角色模板缺失: {role_id}]"
    return f.read_text(encoding="utf-8")


def load_agent_overrides(project_dir) -> dict:
    """读 per-project 的 agent overrides（前端通过 save_agent_overrides 命令写入）。

    位置：`{project}/_state/agent_overrides.json`
    结构：`{ "<role_id>": { "max_steps": 20, "timeout_secs": 600, ... }, ... }`

    不污染 `agent_registry.json` ground truth；仅在 build_full_prompt 时合并。
    """
    from pathlib import Path as _P
    path = _P(project_dir) / "_state" / "agent_overrides.json"
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return {}


def apply_overrides(role_def: dict, role_id: str, overrides: dict) -> dict:
    """合并 overrides 到 role_def（返回新 dict，不修改原对象）。

    role_def 来自 _build_role_view（registry 能力 + route_table.nodes 裁决），
    overrides 来自 _state/agent_overrides.json（Runtime Config 层，08-resources）。
    顶层字段被 override 字段覆盖（如 max_steps / max_retries / timeout / model / tools_enabled）。
    """
    role_override = overrides.get(role_id, {})
    if not role_override:
        return role_def
    merged = dict(role_def)
    for k, v in role_override.items():
        merged[k] = v
    return merged


def build_context_summary(ws: WorkflowState, role_id: str) -> str:
    """构建角色执行时的上下文摘要：项目状态 + 已完成角色的产出概要。"""
    lines = []
    lines.append(f"## 项目状态概要")
    lines.append(f"- 场景: {ws.scenario}")
    lines.append(f"- 项目目录: {ws.project_dir}")

    summary = ws.summary()
    completed = [rid for rid, info in summary["roles"].items()
                 if info["status"] == "completed"]
    if completed:
        lines.append(f"- 已完成角色: {', '.join(completed)}")

    running = [rid for rid, info in summary["roles"].items()
               if info["status"] == "running"]
    if running:
        lines.append(f"- 正在执行: {', '.join(running)}")
    # [2026-06-06 F7] 角色清单仅供定位。PM 曾据"researcher已完成"+输入里无其产物，
    # 在交付物里断言"researcher未输出可用产物"（实情:不是它的输入,本就看不到）误导下游。
    lines.append(
        "- ⚠️ 以上角色清单仅供定位进度；你只许使用与评论下方「你的输入文件」"
        "清单里的内容，**不得在交付物里断言其他角色产出的存在性或质量**"
    )

    # 列出该角色的输入文件（视图 inputs = 有效上游 registry.outputs 并集）
    role_def = ws._roles_by_id.get(role_id, {})
    inputs = role_def.get("inputs", [])

    lines.append(f"\n## 你的输入文件")
    if isinstance(inputs, list):
        for inp in inputs:
            # glob 模式(registry.outputs 常写 `_research/*.md` 这类)必须展开后逐个列出
            # 具体可读路径;早期 bug 是拿字面 `*.md` 去 .exists() → 永远 ❌,
            # 害得下游被骗"上游不存在"、产空壳。§0 寻址要给 SubAgent 具体地址。
            if any(c in inp for c in "*?["):
                matches = sorted(p for p in ws.project_dir.glob(inp) if p.is_file())
                if matches:
                    for m in matches:
                        rel = m.relative_to(ws.project_dir)
                        lines.append(f"- ✅ `{rel}` ({m.stat().st_size} bytes)")
                else:
                    lines.append(f"- ❌ `{inp}` (无匹配文件)")
                continue
            fp = ws.project_dir / inp
            if fp.exists():
                if fp.is_file():
                    size = fp.stat().st_size
                    lines.append(f"- ✅ `{inp}` ({size} bytes)")
                elif fp.is_dir():
                    count = len(list(fp.rglob("*")))
                    lines.append(f"- ✅ `{inp}/` ({count} 个文件)")
            else:
                lines.append(f"- ❌ `{inp}` (不存在)")

    # [2026-06-04 素材链落地] inventory.usable_by 含本角色的用户素材直接列具体路径——
    # 此前 usable_by 绑定只登记无人消费,下游全靠 web_search 而用户给的资料躺着没人读。
    try:
        inv_fp = ws.project_dir / "_state" / "materials_inventory.json"
        if inv_fp.is_file():
            inv = json.loads(inv_fp.read_text(encoding="utf-8"))
            mine = [i for i in inv if isinstance(i, dict)
                    and role_id in (i.get("usable_by") or [])]
            if mine:
                lines.append("\n## 用户提供的素材（优先取材，数据/说法以此为准，引用时标素材文件名）")
                for i in mine:
                    lines.append(f"- ✅ `{i.get('file_path','')}` — {i.get('notes','')[:120]}")
    except (OSError, json.JSONDecodeError):
        pass

    # [B2 素材直通] 三省六部等无 materials_auditor 的流程:直接扫 配套材料/,把用户上传的
    # 文件列给每个角色(仅当没有 materials_inventory 时走,避免与上面 PPT 素材链重复)。
    try:
        cailiao_dir = ws.project_dir / "配套材料"
        inv_exists = (ws.project_dir / "_state" / "materials_inventory.json").is_file()
        if not inv_exists and cailiao_dir.is_dir():
            files = sorted(p for p in cailiao_dir.rglob("*") if p.is_file())
            if files:
                lines.append("\n## 用户上传的素材（在 配套材料/ 下，按需 read_file 取用；数据/说法以此为准，引用时标文件名）")
                for f in files:
                    rel = f.relative_to(ws.project_dir)
                    lines.append(f"- ✅ `{rel}` ({f.stat().st_size} bytes)")
    except OSError:
        pass

    # [2026-06-06 F2] gaps 按 needed_by 注入——此前缺料表只有 auditor 自己的 L1 闸门读，
    # needed_by/workaround 无人消费；下游缺料时各凭 role prompt 自由发挥纯属撞巧。
    try:
        gaps_fp = ws.project_dir / "_state" / "materials_gaps.json"
        if gaps_fp.is_file():
            gaps = json.loads(gaps_fp.read_text(encoding="utf-8"))
            mine = [g for g in gaps if isinstance(g, dict)
                    and role_id in (g.get("needed_by") or [])]
            if mine:
                lines.append("\n## 已知缺料（素材收集员登记；按 workaround 行动，缺料导致的降级在产出里如实标注）")
                for g in mine:
                    lines.append(f"- [{g.get('severity','')}] {g.get('what','')[:80]}"
                                 f" → {g.get('workaround','')[:100]}")
    except (OSError, json.JSONDecodeError):
        pass

    return "\n".join(lines)


def build_full_prompt(ws: WorkflowState, role_id: str) -> str:
    """组装完整的角色执行 prompt。"""
    # [B2] 差事节点(<bu>#<seq>):persona/能力按所属部加载,具体差事下方注入
    bu = ws._bu_of(role_id)
    task_spec = ws._task_nodes.get(role_id)
    prompt_file = ws._reg.get(bu, {}).get("prompt_file")
    role_prompt = load_role_prompt(bu, prompt_file)
    context = build_context_summary(ws, role_id)
    overrides = load_agent_overrides(ws.project_dir)
    role_def = apply_overrides(ws._roles_by_id.get(role_id, {}), role_id, overrides)

    sections = []
    sections.append(f"# 当前任务：{role_def.get('name', role_id)}")
    sections.append("")
    sections.append(context)
    sections.append("")
    sections.append("---")
    sections.append("")
    sections.append(role_prompt)
    # [B2] 差事节点:把这次的具体差事直接注入(以此为准,不要去 dispatch.json 找别的)
    if task_spec:
        sections.append("")
        sections.append("---")
        sections.append("")
        sections.append("## 📋 你这次的差事(以此为准)")
        sections.append(f"**任务**：{task_spec.get('task', '')}")
        if task_spec.get("requirements"):
            sections.append(f"**要求**：{task_spec['requirements']}")
        sections.append("\n> 直接按上面这份差事干活。**不要去 dispatch.json 里找任务令**——你这次的活就是上面这段。产物写到下方[产物地址]给的路径。")
    sections.append("")
    sections.append("---")
    sections.append("")
    sections.append(f"## 完成后")
    gate = role_def.get("gate", "auto")
    gate_desc = role_def.get("gate_description", "")
    if gate == "human":
        sections.append(f"完成所有输出文件后，**停下来等用户确认**。")
        sections.append(f"确认要点：{gate_desc}")
    elif gate == "auto":
        sections.append(f"完成所有输出文件后，自动通过。验证条件：{gate_desc}")

    max_retries = role_def.get("max_retries", 3)
    sections.append(f"\n最大重试次数: {max_retries}")

    return "\n".join(sections)


def _dispatch_spec(ws: WorkflowState, role: str) -> dict:
    """registry.<role>.dispatch（能力真相源里的派发模式声明）。缺省 single。"""
    return ws._reg.get(role, {}).get("dispatch", {}) or {}


def is_per_page(ws: WorkflowState, role: str) -> bool:
    return _dispatch_spec(ws, role).get("mode") == "per_page"


def _load_json_safe(path: Path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return None


def build_tasks_for(ws: WorkflowState, role: str) -> tuple[list, bool]:
    """[per_page] 按 registry dispatch.over 路由枚举器。
    当前没有注册任何枚举器——原 outline.slides / html_image_slots 枚举器是 h3c-ppt
    专属,随该工作流于 2026-06-11 存档(WorkSpace/_archive/)。声明 per_page 的角色
    会得到 ([], False) → 上层回退普通 dispatch。新工作流需要 per_page 时,在此按
    dispatch.over 注册自己的枚举器(返回 (tasks, inputs_ready))。"""
    _ = (ws, role)
    return [], False




def determine_next_role(ws: WorkflowState) -> dict:
    """决定下一步该派谁。返回调度决策。"""
    actionable = ws.next_actionable()

    if ws.all_completed():
        return {
            "action": "all_done",
            "message": "所有角色已完成，工作流结束",
        }

    if not actionable:
        summary = ws.summary()
        gate_waiting = [rid for rid, info in summary["roles"].items()
                        if info["status"] == "gate_waiting"]
        running = [rid for rid, info in summary["roles"].items()
                   if info["status"] == "running"]
        failed = [rid for rid, info in summary["roles"].items()
                  if info["status"] == "failed"]
        blocked = [rid for rid, info in summary["roles"].items()
                   if info["status"] == "blocked"]

        if gate_waiting:
            return {
                "action": "waiting_for_human",
                "message": f"等待用户审批: {', '.join(gate_waiting)}",
                "waiting_roles": gate_waiting,
            }
        if running:
            return {
                "action": "role_running",
                "message": f"角色执行中: {', '.join(running)}",
                "running_roles": running,
            }
        if blocked:
            # 回滚次数耗尽（apply_rollback 转 blocked）—— 不再回滚，须人工介入
            return {
                "action": "blocked",
                "message": f"回滚次数耗尽，已阻塞: {', '.join(blocked)}（需人工介入）",
                "blocked_roles": blocked,
            }
        if failed:
            return {
                "action": "blocked_by_failure",
                "message": f"角色失败阻塞: {', '.join(failed)}",
                "failed_roles": failed,
            }
        return {
            "action": "blocked",
            "message": "所有可执行角色的上游依赖未满足",
        }

    next_role = actionable[0]
    role_name = ws._roles_by_id.get(next_role, {}).get("name", next_role)

    # [per_page] 纵向 fan-out：该节点按 dispatch.over 的列表拆成 N 个子任务并发派发。
    # 节点在 DAG 里仍是单一逻辑节点——这里只把"派发"从 1 个变 N 个；join/回滚/scenario 不受影响。
    if is_per_page(ws, next_role):
        tasks, inputs_ready = build_tasks_for(ws, next_role)
        if tasks:
            return {
                "action": "dispatch_batch",
                "role_id": next_role,
                "role_name": role_name,
                "tasks": tasks,
                "batch_total": len(tasks),
                "all_actionable": actionable,
            }
        if inputs_ready:
            # [空批次] 输入已就绪但 0 任务(如 0 图位 deck) → 原子完成本节点,继续推进下游
            ws.start_role(next_role, batch_total=0)
            ws.complete_role(next_role)
            ws.save()
            decision = determine_next_role(ws)
            decision.setdefault("_empty_batch_completed", []).append(next_role)
            return decision
        # 输入未就绪(slides/outline 还没生成) → 回退普通 dispatch（正常会因缺输入失败/重试）

    return {
        "action": "dispatch",
        "role_id": next_role,
        "role_name": role_name,
        "all_actionable": actionable,
    }


def main():
    parser = argparse.ArgumentParser(description="SDAN 工作流调度器(通用引擎)")
    parser.add_argument("project", help="项目目录")
    parser.add_argument("--scenario", required=True, help="场景 id(须在该工作流 route_table.scenarios 里)")

    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--next", action="store_true", help="获取下一步调度决策")
    group.add_argument("--prompt", metavar="ROLE_ID", help="获取角色完整 prompt")
    group.add_argument("--start", metavar="ROLE_ID", help="标记角色开始执行")
    group.add_argument("--complete", metavar="ROLE_ID", help="标记角色完成")
    group.add_argument("--fail", metavar="ROLE_ID", help="标记角色失败")
    group.add_argument("--fail-fatal", metavar="ROLE_ID", help="标记角色发生不可重试失败并直接进入 failed")
    group.add_argument("--gate-wait", metavar="ROLE_ID", help="标记角色等待审批")
    group.add_argument("--rollback", metavar="VIOLATION_TYPE", help="按 violation_type 应用 SDAN 声明式回滚（自动算闭包+max_rollback）")
    group.add_argument("--reset", metavar="ROLE_ID", help="重置角色为 pending（清重试计数），用于从失败节点续跑")
    group.add_argument("--status", action="store_true", help="查看完整状态")
    group.add_argument("--batch-tasks", metavar="ROLE_ID", help="[per_page] 返回该角色的 per-page 子任务列表（不改状态），供 harness 重派整批")
    group.add_argument("--page-done", nargs=2, metavar=("ROLE_ID", "PAGE"), help="[per_page] 记一页完成，返回 {done,total,complete}")

    parser.add_argument("--reason", default="", help="失败原因（配合 --fail）")
    parser.add_argument("--batch-total", type=int, default=None,
                        help="[per_page] 本批实际任务数（配合 --start；缺省回退枚举）")
    args = parser.parse_args()

    ws = WorkflowState(args.project, scenario=args.scenario)

    if args.next:
        decision = determine_next_role(ws)
        print(json.dumps(decision, ensure_ascii=False, indent=2))

    elif args.prompt:
        prompt = build_full_prompt(ws, args.prompt)
        print(prompt)

    elif args.batch_tasks:
        # 不改状态，只返回该 per_page 角色的子任务（供 harness 重派整批）。按 dispatch.over 路由枚举器。
        tasks, _ = build_tasks_for(ws, args.batch_tasks)
        print(json.dumps({"tasks": tasks}, ensure_ascii=False))

    elif args.page_done:
        role, page = args.page_done
        res = ws.record_page_done(role, int(page))
        ws.save()
        print(json.dumps(res, ensure_ascii=False))

    elif args.start:
        # [per_page] --batch-total 由 harness 传本次实际派发任务数（幂等跳过后的真实 N，
        # 不等于 outline 页数）；缺省由 start_role 回退枚举。
        ws.start_role(args.start, batch_total=args.batch_total)
        ws.save()
        print(json.dumps({"ok": True, "role": args.start, "status": "running"}, ensure_ascii=False))

    elif args.complete:
        ws.complete_role(args.complete)
        ws.save()
        stale = ws.check_stale()
        print(json.dumps({
            "ok": True, "role": args.complete, "status": "completed",
            "stale_detected": stale,
        }, ensure_ascii=False))

    elif args.fail:
        ws.fail_role(args.fail, error=args.reason)
        ws.save()
        print(json.dumps({
            "ok": True, "role": args.fail,
            "status": ws.get_status(args.fail),
            "reason": args.reason,
        }, ensure_ascii=False))

    elif args.fail_fatal:
        ws.fail_role(args.fail_fatal, error=args.reason, fatal=True)
        ws.save()
        print(json.dumps({
            "ok": True, "role": args.fail_fatal,
            "status": ws.get_status(args.fail_fatal),
            "reason": args.reason,
            "retryable": False,
        }, ensure_ascii=False))

    elif args.gate_wait:
        ws.gate_waiting(args.gate_wait)
        ws.save()
        print(json.dumps({"ok": True, "role": args.gate_wait, "status": "gate_waiting"}, ensure_ascii=False))

    elif args.rollback:
        # args.rollback = violation_type（dispatch key，如 density_violation）。
        # apply_rollback 返回 dict：成功含 rolled_back_to/cascade/count；耗尽含 blocked。
        result = ws.apply_rollback(args.rollback)
        ws.save()
        result["next_actionable"] = ws.next_actionable()
        print(json.dumps(result, ensure_ascii=False, indent=2))

    elif args.reset:
        # 从失败节点续跑：标回 pending + 清重试计数,让 --next 重新派它。
        # 上游已 completed 的节点不动(天然不重跑);旧产出靠重跑覆盖(不清)。
        ws.reset_role(args.reset)
        ws.save()
        print(json.dumps({
            "ok": True, "role": args.reset,
            "status": ws.get_status(args.reset),
            "next_actionable": ws.next_actionable(),
        }, ensure_ascii=False))

    elif args.status:
        summary = ws.summary()
        # Enrich：附 per-role effective_config（应用 _state/agent_overrides.json 后的配置）。
        # 前端配置面板读这个字段渲染"当前生效"提示，区分 override vs 默认值。
        overrides = load_agent_overrides(ws.project_dir)
        for role_id, role_info in summary.get("roles", {}).items():
            role_def = ws._roles_by_id.get(role_id, {})
            effective = apply_overrides(role_def, role_id, overrides)
            role_info["effective_config"] = {
                "max_retries": effective.get("max_retries"),
                "max_steps": effective.get("max_steps"),
                "timeout_secs": effective.get("timeout_secs"),
                "model": effective.get("model"),
                "gate": effective.get("gate"),
                "_overridden": list(overrides.get(role_id, {}).keys()),
            }
        print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
