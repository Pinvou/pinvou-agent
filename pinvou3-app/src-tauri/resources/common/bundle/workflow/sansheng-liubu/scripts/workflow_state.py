#!/usr/bin/env python3
"""workflow_state.py — SDAN 工作流状态机(通用引擎,工作流无关)

管理工作流各角色的执行状态、依赖追踪、文件 hash 变更传播(角色集来自数据)。
调度器(scheduler.py)读写这个状态来决定下一步该派谁。

用法:
    # 作为库导入
    from workflow_state import WorkflowState
    ws = WorkflowState("/path/to/project", scenario="sansheng_liubu")
    ws.next_actionable()    # 返回可执行的角色列表
    ws.start_role("taizi")
    ws.complete_role("taizi")
    ws.save()

    # CLI 查看状态
    python3 workflow_state.py /path/to/project --scenario sansheng_liubu
    python3 workflow_state.py /path/to/project --status

状态文件: <project>/_state/workflow_progress.json
"""

from __future__ import annotations

import hashlib
import json
import sys
from datetime import datetime, timezone, timedelta
from enum import Enum
from pathlib import Path


ROUTE_TABLE_JSON = Path(__file__).resolve().parent.parent / "route_table.json"
REGISTRY_JSON = Path(__file__).resolve().parent.parent / "agent_registry.json"

TZ_CN = timezone(timedelta(hours=8))


class RoleStatus(str, Enum):
    PENDING = "pending"
    BLOCKED = "blocked"
    READY = "ready"
    RUNNING = "running"
    GATE_WAITING = "gate_waiting"
    COMPLETED = "completed"
    FAILED = "failed"
    STALE = "stale"
    SKIPPED = "skipped"


class WorkflowState:
    def __init__(self, project_dir: str | Path, scenario: str):
        self.project_dir = Path(project_dir).resolve()
        self.scenario = scenario
        self._rt = self._load_route_table()
        self._reg = self._load_registry()
        self._scenario_cfg = self._rt["scenarios"].get(scenario, {})
        self._active_roles = self._scenario_cfg.get("roles", [])
        self._dep_overrides = self._scenario_cfg.get("dependency_overrides", {})
        self._edges = self._rt.get("edges", [])
        self._nodes = self._rt.get("nodes", {})
        # [B2 差事驱动] 动态路由层:尚书省派单后由 dispatch_graph.py 编译落盘
        # _state/dynamic_routes.json。此处把静态六部层换成差事节点 union 进来。
        # 文件不存在(非 sansheng / 尚书省尚未派单)→ 全程 no-op,静态行为不变。
        self._task_nodes: dict = {}      # 差事 id(<bu>#<seq>)→ spec
        self._dynamic_deps: dict = {}    # 差事/回奏 的依赖(覆盖静态 edges)
        self._apply_dynamic_routes()
        self._roles_by_id = self._build_role_view()
        self._state: dict = self._load_or_init_state()
        self._reconcile_state_roles()

    def _load_route_table(self) -> dict:
        with open(ROUTE_TABLE_JSON, encoding="utf-8") as f:
            return json.load(f)

    def _load_registry(self) -> dict:
        with open(REGISTRY_JSON, encoding="utf-8") as f:
            return json.load(f).get("agents", {})

    # ── [B2 差事驱动] 动态路由层 ──

    def _apply_dynamic_routes(self):
        """读 _state/dynamic_routes.json(尚书省派单 → dispatch_graph.py 编译),
        把静态六部层换成差事节点 union 进来。文件不存在/空 → no-op,静态行为不变。"""
        drf = self.project_dir / "_state" / "dynamic_routes.json"
        if not drf.exists():
            return
        try:
            with open(drf, encoding="utf-8") as f:
                dr = json.load(f)
        except (OSError, json.JSONDecodeError):
            return
        task_nodes = dr.get("task_nodes") or {}
        new_active = dr.get("active_roles") or []
        if not task_nodes or not new_active:
            return
        dynamic_layer = set(dr.get("dynamic_layer", []))
        # 静态 _active_roles 里的六部层(dynamic_layer)替换成差事节点,其余节点保序。
        spliced, inserted = [], False
        for rid in self._active_roles:
            if rid in dynamic_layer:
                if not inserted:
                    spliced.extend(new_active)
                    inserted = True
                # 跳过静态部节点本身（被差事节点取代）
            else:
                spliced.append(rid)
        if not inserted:  # 场景里没有六部层（不该发生）→ 保险：差事节点前置
            spliced = new_active + spliced
        self._active_roles = spliced
        self._task_nodes = task_nodes
        self._dynamic_deps = dr.get("depends_on") or {}

    def _bu_of(self, node_id: str) -> str:
        """差事节点 id(<bu>#<seq>)→ 所属部(查 registry 能力用);非差事原样返回。"""
        task = self._task_nodes.get(node_id)
        return task["bu"] if task else node_id

    def _reconcile_state_roles(self):
        """确保 _active_roles 每个角色(含动态差事节点)在 state.roles 有条目。
        差事节点是尚书省派单后才出现,init 时 state 没有,这里补 pending 并落盘。
        被取代的静态六部条目留在 state 当孤儿(不在 _active_roles,不参与调度)。"""
        roles = self._state.setdefault("roles", {})
        changed = False
        for rid in self._active_roles:
            if rid not in roles:
                roles[rid] = {
                    "status": RoleStatus.PENDING.value,
                    "started_at": None, "completed_at": None,
                    "retries": 0, "output_hashes": {}, "gate_result": None, "error": None,
                }
                changed = True
        if changed:
            self.save()

    def _build_role_view(self) -> dict:
        """合成每个激活角色的视图:能力字段 ← registry(唯一能力真像,06 第9行),
        拓扑/裁决字段 ← route_table.nodes。depends_on 不入视图——由 _get_depends_on
        直接从 edges + scenario overrides 算(统一"有效依赖图"来源)。"""
        view = {}
        for rid in self._active_roles:
            bu = self._bu_of(rid)
            reg = self._reg.get(bu, {})
            node = self._nodes.get(bu, {})
            soft = node.get("soft") if isinstance(node.get("soft"), dict) else {}
            task = self._task_nodes.get(rid)
            if task:  # [B2] 差事节点：能力按部查 registry，name/outputs 用差事自身
                title = task.get("title") or f"差事{task.get('seq', '')}"
                name = f"{reg.get('name', bu)}·{title}"
                outputs = [task["output_file"]]
            else:
                name = reg.get("name", rid)
                outputs = reg.get("outputs", [])
            view[rid] = {
                "id": rid,
                "name": name,
                "gate": reg.get("gate", "auto"),
                "outputs": outputs,
                "gate_description": (soft or {}).get("criteria", ""),
                "max_retries": node.get("max_retries", 3),
                "inputs": self._derive_inputs(rid),
            }
        return view

    def _derive_inputs(self, role_id: str) -> list:
        """角色输入文件 = 【传递闭包】所有上游祖先(active)的 registry.outputs 并集。
        §0 寻址按数据可达性给地址:沿有效依赖图上溯【全部祖先】(不只直接上游),
        否则跨级产物会被漏(实锤过:角色 route_table.accepts 需要隔级祖先的产物,只取直接上游就拿不到,SubAgent 只能瞎猜路径/脑补内容)。BFS 上溯,穿过非 active 节点(场景剪枝),
        只收 active 祖先的 outputs。首节点(无上游)返回空 list。"""
        files = []
        seen = set()
        queue = list(self._get_depends_on(role_id))
        while queue:
            dep = queue.pop(0)
            if dep in seen:
                continue
            seen.add(dep)
            if dep in self._active_roles:
                # [B2] 上游若是差事节点，输入文件取它的专属产物 deliverables/<bu>_<seq>.md
                dep_task = self._task_nodes.get(dep)
                outs = ([dep_task["output_file"]] if dep_task
                        else self._reg.get(self._bu_of(dep), {}).get("outputs", []))
                for out in outs:
                    if out not in files:
                        files.append(out)
            queue.extend(self._get_depends_on(dep))
        return files

    def _state_file(self) -> Path:
        return self.project_dir / "_state" / "workflow_progress.json"

    def _load_or_init_state(self) -> dict:
        sf = self._state_file()
        if not sf.exists():
            return self._init_state()
        with open(sf, encoding="utf-8") as f:
            existing = json.load(f)
        # 兼容 pinvou3-app harness::init_project 写入的最小骨架（仅 scenario/created_at），
        # 自动补全 roles + schema_version + project_dir 字段，保留外部字段。
        if "roles" not in existing or not isinstance(existing.get("roles"), dict):
            stub = self._init_state()
            for k, v in stub.items():
                existing.setdefault(k, v)
            existing["roles"] = stub["roles"]
            # 立即落盘补全后的 state，避免后续 --next / --status 重复补全
            sf.parent.mkdir(parents=True, exist_ok=True)
            with open(sf, "w", encoding="utf-8") as wf:
                json.dump(existing, wf, ensure_ascii=False, indent=2)
        # 兼容旧 state 文件（schema_version 1，无 rollback_counts）：补全后才能跑回滚计数。
        existing.setdefault("rollback_counts", {})
        return existing

    def _init_state(self) -> dict:
        roles_state = {}
        for rid in self._active_roles:
            roles_state[rid] = {
                "status": RoleStatus.PENDING.value,
                "started_at": None,
                "completed_at": None,
                "retries": 0,
                "output_hashes": {},
                "gate_result": None,
                "error": None,
            }
        return {
            "schema_version": 2,
            "scenario": self.scenario,
            "project_dir": str(self.project_dir),
            "created_at": _now_iso(),
            "updated_at": _now_iso(),
            "roles": roles_state,
            # per-(rollback_to:violation_type) 回滚计数，整轮累计、complete/reset 不清（反 livelock）
            "rollback_counts": {},
        }

    def save(self):
        self._state["updated_at"] = _now_iso()
        sf = self._state_file()
        sf.parent.mkdir(parents=True, exist_ok=True)
        with open(sf, "w", encoding="utf-8") as f:
            json.dump(self._state, f, ensure_ascii=False, indent=2)

    # ── 依赖解析 ──

    def _get_upstream_from_edges(self, role_id: str) -> list[str]:
        """从 route_table.edges 反查 role_id 的直接上游（限当前激活集）。"""
        return [e["from"] for e in self._edges
                if e.get("to") == role_id and e.get("from") in self._active_roles]

    def _get_depends_on(self, role_id: str) -> list[str]:
        """有效依赖：[B2] 动态差事图依赖优先(差事节点 + 回奏 join)→ 场景
        dependency_overrides → 否则从 edges 反查直接上游。
        统一"有效依赖图"来源（SDAN 06 §apply_scenario：剪枝 + 重写后唯一一份）。"""
        if role_id in self._dynamic_deps:
            return [d for d in self._dynamic_deps[role_id] if d in self._active_roles]
        if role_id in self._dep_overrides:
            deps = self._dep_overrides[role_id].get("depends_on", [])
            if isinstance(deps, list):
                return [d for d in deps if d in self._active_roles]
        return self._get_upstream_from_edges(role_id)

    def _deps_satisfied(self, role_id: str) -> bool:
        deps = self._get_depends_on(role_id)
        for dep in deps:
            if dep not in self._active_roles:
                continue
            dep_status = self._state["roles"].get(dep, {}).get("status")
            if dep_status != RoleStatus.COMPLETED.value:
                return False
        return True

    # ── 状态查询 ──

    def get_status(self, role_id: str) -> str:
        return self._state["roles"].get(role_id, {}).get("status", "unknown")

    def next_actionable(self) -> list[str]:
        """返回当前可以开始执行的角色列表。"""
        actionable = []
        for rid in self._active_roles:
            rs = self._state["roles"].get(rid, {})
            status = rs.get("status", "pending")
            if status in (RoleStatus.PENDING.value, RoleStatus.READY.value, RoleStatus.STALE.value):
                if self._deps_satisfied(rid):
                    actionable.append(rid)
        return actionable

    def all_completed(self) -> bool:
        for rid in self._active_roles:
            s = self.get_status(rid)
            if s not in (RoleStatus.COMPLETED.value, RoleStatus.SKIPPED.value):
                return False
        return True

    # ── 状态转移 ──

    def _is_per_page(self, role_id: str) -> bool:
        return (self._reg.get(role_id, {}).get("dispatch", {}) or {}).get("mode") == "per_page"

    def _per_page_total(self, role_id: str) -> int:
        """per_page 节点总实例数兜底（仅 start_role 未显式传 batch_total 时用）。
        当前没有注册任何枚举器(原 outline.slides/html_image_slots 是 legacy-ppt-workflow 专属,
        2026-06-11 随该工作流存档)——harness 派批时总是显式传 batch_total,这里只剩
        兜底语义,返回 0。新工作流要 per_page 时与 scheduler.build_tasks_for 一起注册。"""
        _ = role_id
        return 0

    def start_role(self, role_id: str, batch_total: int | None = None):
        rs = self._state["roles"][role_id]
        rs["status"] = RoleStatus.RUNNING.value
        rs["started_at"] = _now_iso()
        # [per_page] 纵向 fan-out 节点：跑批期记 join 计数（N 实例全到才算节点 done）。
        # total 优先取调度方显式传入的本批任务数（幂等跳过后的真实 N）。
        if self._is_per_page(role_id):
            total = batch_total if batch_total is not None else self._per_page_total(role_id)
            rs["batch"] = {"total": total, "done": []}

    def record_page_done(self, role_id: str, page: int) -> dict:
        """[per_page] 记一页 SubAgent 完成。返回 {done,total,complete}。
        complete=True ⇒ N 实例全到，可对【单一逻辑节点】验收一次。
        事件循环串行调用此方法（无并发写竞争）。"""
        rs = self._state["roles"].get(role_id, {})
        batch = rs.get("batch")
        if not batch:
            return {"done": 1, "total": 1, "complete": True}  # 非 per_page/状态异常 → 当单页
        done = batch.setdefault("done", [])
        if page not in done:
            done.append(page)
        total = batch.get("total", 0)
        return {"done": len(done), "total": total,
                "complete": total > 0 and len(done) >= total}

    def complete_role(self, role_id: str, gate_result: str = "passed"):
        rs = self._state["roles"][role_id]
        rs["status"] = RoleStatus.COMPLETED.value
        rs["completed_at"] = _now_iso()
        rs["gate_result"] = gate_result
        rs["output_hashes"] = self._compute_output_hashes(role_id)

    def fail_role(self, role_id: str, error: str = "", fatal: bool = False):
        rs = self._state["roles"][role_id]
        max_retries = self._roles_by_id.get(role_id, {}).get("max_retries", 3)
        if fatal:
            # 鉴权/授权错误不会因重派角色而恢复，直接落失败终态，避免浪费调用。
            rs["retries"] = max_retries
            rs["status"] = RoleStatus.FAILED.value
            rs["error"] = error
            return
        rs["retries"] = rs.get("retries", 0) + 1
        if rs["retries"] >= max_retries:
            rs["status"] = RoleStatus.FAILED.value
            rs["error"] = error
        else:
            rs["status"] = RoleStatus.READY.value
            rs["error"] = f"retry {rs['retries']}: {error}"

    def gate_waiting(self, role_id: str):
        self._state["roles"][role_id]["status"] = RoleStatus.GATE_WAITING.value

    def skip_role(self, role_id: str):
        self._state["roles"][role_id]["status"] = RoleStatus.SKIPPED.value

    def reset_role(self, role_id: str):
        """重置角色为可重跑:标回 PENDING + 清重试计数 + 清 error。
        用于"从失败节点续跑"——失败(retries 耗尽)或想重来的角色,reset 后
        next_actionable 会重新选中它(依赖满足时)。上游 completed 节点不动。
        旧产出不清,靠重跑直接覆盖(SDAN 11-validation 走查结论5)。"""
        rs = self._state["roles"][role_id]
        rs["status"] = RoleStatus.PENDING.value
        rs["retries"] = 0
        rs["error"] = ""
        rs["started_at"] = None
        rs["completed_at"] = None

    # ── 变更传播 ──

    def _compute_output_hashes(self, role_id: str) -> dict[str, str]:
        role_def = self._roles_by_id.get(role_id, {})
        outputs = role_def.get("outputs", [])
        if isinstance(outputs, dict):
            outputs = [v for vals in outputs.values() for v in (vals if isinstance(vals, list) else [vals])]

        hashes = {}
        for out_pattern in outputs:
            if "*" in out_pattern:
                parent = self.project_dir / Path(out_pattern).parent
                if parent.is_dir():
                    for f in sorted(parent.glob(Path(out_pattern).name)):
                        rel = str(f.relative_to(self.project_dir))
                        hashes[rel] = _file_hash(f)
            else:
                fp = self.project_dir / out_pattern
                if fp.exists():
                    hashes[out_pattern] = _file_hash(fp)
        return hashes

    def check_stale(self) -> list[str]:
        """检查哪些已完成的角色的输出发生了变化，标记下游为 stale。"""
        stale_roles = []
        for rid in self._active_roles:
            rs = self._state["roles"].get(rid, {})
            if rs.get("status") != RoleStatus.COMPLETED.value:
                continue
            old_hashes = rs.get("output_hashes", {})
            new_hashes = self._compute_output_hashes(rid)
            if old_hashes != new_hashes:
                rs["output_hashes"] = new_hashes
                downstream = self._find_downstream(rid)
                for d in downstream:
                    ds = self._state["roles"].get(d, {})
                    if ds.get("status") == RoleStatus.COMPLETED.value:
                        ds["status"] = RoleStatus.STALE.value
                        stale_roles.append(d)
        return stale_roles

    def apply_rollback(self, violation_type: str) -> dict:
        """SDAN 声明式回滚（06-route-table）。入参 = violation_type（dispatch key，
        如 density_violation）。查 rollback_dispatch[violation_type] 得 rollback_to + max_rollback：
        - cascade = rollback_to + 其 edges 下游传递闭包（_find_downstream 自动算，**不手写**
          —— 旧手写 cascade 漏 illustrator 是 bug 源）。起点标 READY（立即可重派），下游标 STALE。
        - per-(rollback_to:violation_type) 计数整轮累计（complete/reset 不清，反 livelock）；
          超 max_rollback 则起点转 BLOCKED + 不再回滚（对称 local retry 的 max_retries→blocked 逃生）。
        返回 dict：成功 {ok:True, rolled_back_to, cascade, count, max_rollback}；
                  耗尽 {ok:False, blocked, count, max_rollback}；未知 {ok:False, reason}。"""
        dispatch = self._rt.get("rollback_rules", {}).get("rollback_dispatch", {})
        rule = dispatch.get(violation_type)
        if not isinstance(rule, dict):
            return {"ok": False, "reason": f"unknown violation_type: {violation_type}"}
        rollback_to = rule["rollback_to"]
        max_rollback = rule.get("max_rollback", 2)
        counts = self._state.setdefault("rollback_counts", {})
        key = f"{rollback_to}:{violation_type}"
        count = counts.get(key, 0)
        if count >= max_rollback:
            # 上界耗尽：不再回滚，起点转 blocked（系统消息上浮由调度层处理）
            if rollback_to in self._state["roles"]:
                self._state["roles"][rollback_to]["status"] = RoleStatus.BLOCKED.value
                self._state["roles"][rollback_to]["error"] = (
                    f"rollback exhausted: {violation_type} x{count} >= max {max_rollback}"
                )
            return {"ok": False, "blocked": rollback_to, "violation_type": violation_type,
                    "count": count, "max_rollback": max_rollback}
        counts[key] = count + 1
        # 自动闭包：起点 + edges 下游（限激活集，_find_downstream 已过滤）
        closure = [rollback_to] + self._find_downstream(rollback_to)
        affected = []
        for rid in closure:
            if rid not in self._state["roles"]:
                continue
            self._state["roles"][rid]["status"] = (
                RoleStatus.READY.value if rid == rollback_to else RoleStatus.STALE.value
            )
            self._state["roles"][rid]["error"] = f"rollback: {violation_type}"
            affected.append(rid)
        return {"ok": True, "rolled_back_to": rollback_to, "violation_type": violation_type,
                "cascade": affected, "count": counts[key], "max_rollback": max_rollback}

    def _find_downstream(self, role_id: str) -> list[str]:
        """从解析后的依赖图找到所有直接或间接依赖 role_id 的角色。"""
        direct = set()
        for rid in self._active_roles:
            if role_id in self._get_depends_on(rid):
                direct.add(rid)
        all_downstream = set(direct)
        for d in direct:
            all_downstream.update(self._find_downstream(d))
        return list(all_downstream)

    # ── 摘要 ──

    def summary(self) -> dict:
        counts = {}
        for rid in self._active_roles:
            s = self.get_status(rid)
            counts[s] = counts.get(s, 0) + 1

        return {
            "scenario": self.scenario,
            "total_roles": len(self._active_roles),
            "status_counts": counts,
            "actionable": self.next_actionable(),
            "all_completed": self.all_completed(),
            "roles": {
                rid: {
                    "name": self._roles_by_id.get(rid, {}).get("name", rid),
                    "status": self.get_status(rid),
                    "depends_on": self._get_depends_on(rid),
                    # [B2 E1] 差事节点带 wave(分层用)+ bu(取头像/配色用);非差事为 None
                    "wave": self._task_nodes.get(rid, {}).get("wave"),
                    "bu": self._task_nodes.get(rid, {}).get("bu"),
                }
                for rid in self._active_roles
            },
        }


def _now_iso() -> str:
    return datetime.now(TZ_CN).isoformat(timespec="seconds")


def _file_hash(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    return h.hexdigest()[:16]


# ── CLI ──

def main():
    import argparse
    parser = argparse.ArgumentParser(description="SDAN 工作流状态机(通用引擎)")
    parser.add_argument("project", help="项目目录")
    parser.add_argument("--scenario", required=True, help="场景 id(须在该工作流 route_table.scenarios 里)")
    parser.add_argument("--status", action="store_true", help="显示当前状态")
    parser.add_argument("--init", action="store_true", help="初始化状态文件")
    parser.add_argument("--next", action="store_true", help="显示下一步可执行角色")
    args = parser.parse_args()

    ws = WorkflowState(args.project, scenario=args.scenario)

    if args.init:
        ws.save()
        print(f"初始化完成: {ws._state_file()}")
        return

    if args.next:
        actionable = ws.next_actionable()
        print(json.dumps(actionable, ensure_ascii=False))
        return

    summary = ws.summary()
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
