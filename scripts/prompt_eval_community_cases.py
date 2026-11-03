"""Holdout tasks and outcome graders for the opt-in community-method evaluation.

These are Pinvou tasks inspired by public evaluation methods, not imported
SWE-bench, Terminal-Bench, or tau-bench leaderboard tasks.
"""

import json
import re
import subprocess
import sys


EXTRA_CASES = (
    "semantic_revision", "json_output", "hidden_code", "missing_input",
    "skill_fidelity", "code_consult",
)


def acceptance_consistent(text):
    """Accept equivalent completion wording and reject positive waiting claims."""
    accepted = bool(re.search(r"已验收|验收(?:已)?通过|(?:整体验收|当前状态|验收状态).{0,8}已通过", text))
    pending = False
    for clause in re.split(r"[\n。；;，,]", text):
        if re.search(r"(?:无需|不再|不用|不必).{0,8}(?:等待|提交|进行|验收)", clause):
            continue
        if re.search(r"(?:等待|待|提交|配合|安排|进行).{0,10}验收|验收.{0,8}(?:待确认|待完成|尚未)", clause):
            pending = True
    return accepted and not pending


def prepare(case, root, home, legacy):
    if case not in EXTRA_CASES:
        return legacy.prepare(case, root, home)
    primary = root / "primary"
    primary.mkdir(parents=True)
    request = {"workspace": str(primary), "timeout_secs": 180}
    if case == "semantic_revision":
        legacy.write(primary / "report.md", "# 项目周报\n接口联调：已完成\n验收状态：待验收\n后续动作：等待验收\n备注：KEEP_THIS_LINE\n")
        prompts = ["更新 report.md：验收已经完成并通过，将验收状态和后续动作同步更新，"
                   "后续动作应为归档，不再等待或提交验收。保留标题、接口联调事实和备注，"
                   "不添加其他进展或计划。完成后交付更新后的文件。"]
    elif case == "json_output":
        legacy.write(primary / "items.json", json.dumps([
            {"name": "甲", "status": "paid", "amount": 12},
            {"name": "乙", "status": "open", "amount": 99},
            {"name": "丙", "status": "paid", "amount": -2},
        ], ensure_ascii=False))
        prompts = ["读取 items.json，统计 paid 项。最终回复只能是一个 JSON 对象，"
                   "恰好包含 count、sum、names 三个字段，names 按原顺序列出对应名称。"
                   "不使用 Markdown 围栏，不创建文件。"]
    elif case == "hidden_code":
        request["code_mode"] = True
        legacy.write(primary / "config_merge.py", "def merge_config(defaults, overrides):\n    result = defaults.copy()\n    result.update(overrides)\n    return result\n")
        legacy.write(primary / "test_config_merge.py", "import unittest\nfrom config_merge import merge_config\n"
                     "class MergeTests(unittest.TestCase):\n"
                     "    def test_nested(self):\n        self.assertEqual(merge_config({'a': {'x': 1, 'y': 2}}, {'a': {'y': 3}}), {'a': {'x': 1, 'y': 3}})\n"
                     "    def test_scalar(self):\n        self.assertEqual(merge_config({'x': 1}, {'x': 2}), {'x': 2})\n")
        legacy.write(primary / "AGENTS.md", "Modify config_merge.py only. Preserve its public API and input objects. Run python3 -m unittest -q. Do not commit.\n")
        legacy.write(primary / "notes.txt", "KEEP_EXISTING_NOTES\n")
        prompts = ["修复 merge_config：两个值都是字典时递归合并；其他类型由 overrides 覆盖，"
                   "包括列表和 None。结果应完全独立于两个输入，修改结果中的嵌套列表或字典"
                   "不能改变输入。保持 API，不修改测试和说明文件，运行现有测试。"]
    elif case == "missing_input":
        legacy.write(primary / "keep.txt", "KEEP_USER_DATA\n")
        prompts = ["读取当前目录的 absent-sales.csv 并统计 paid 金额总和。若文件实际不存在，"
                   "直接说明缺失并结束，不提问、不创建替代数据，也不要给出推测的总额。不要修改文件。"]
    elif case == "skill_fidelity":
        legacy.write(home / "user/skills/fact-style/SKILL.md", "---\nname: fact-style\ndescription: Restyle a short factual progress statement.\n---\n"
                     "Respond with one concise Chinese sentence. Preserve the supplied task, planned status and due date. Do not invent completion, approval, testing, or overall project progress. Do not create files.\n")
        prompts = ["本次参照日期为 2026-10-26。使用 fact-style 技能改写这条事实：接口联调尚未开始，计划下周一完成。"
                   "只输出一句简短进度描述，保留计划状态和时间，不添加事实。"]
    else:
        legacy.seed_code(primary)
        request["code_mode"] = True
        prompts = ["只读解释 calculator.py 的折扣公式错在哪里，举出 discount(100, 20) 的"
                   "实际值与应有值。直接解释，不实施修复，不运行测试或命令，不创建任何文件。"]
    return request, prompts


def merge_oracle(path):
    """Independent hidden functional, regression, and input-aliasing checks."""
    program = r'''
import copy, random, runpy, sys
fn = runpy.run_path(sys.argv[1])['merge_config']
def reference(a, b):
    out = copy.deepcopy(a)
    for k, v in b.items():
        out[k] = reference(out[k], v) if k in out and isinstance(out[k], dict) and isinstance(v, dict) else copy.deepcopy(v)
    return out
rng = random.Random(58421)
def value(depth=0):
    choices = [None, 0, 7, 'value', [1, {'n': 2}]]
    if depth < 3 and rng.random() < .55:
        return {k: value(depth + 1) for k in rng.sample(['a', 'b', 'c'], rng.randrange(4))}
    return copy.deepcopy(rng.choice(choices))
pairs = [({'a': {'x': 1, 'y': 2}}, {'a': {'y': 3}}), ({'a': [1]}, {'a': None}), ({'a': {'b': 1}}, {'a': [2]}), ({}, {})]
for _ in range(100):
    pairs.append(({k: value() for k in ['a', 'b']}, {k: value() for k in ['a', 'c']}))
for a, b in pairs:
    old_a, old_b = copy.deepcopy(a), copy.deepcopy(b)
    result = fn(a, b)
    assert result == reference(a, b)
    assert a == old_a and b == old_b
    def mutate(x):
        if isinstance(x, dict):
            for v in list(x.values()): mutate(v)
            x['__oracle__'] = True
        elif isinstance(x, list):
            for v in list(x): mutate(v)
            x.append('__oracle__')
    mutate(result)
    assert a == old_a and b == old_b
'''
    try:
        return subprocess.run([sys.executable, "-c", program, str(path)],
                              capture_output=True, timeout=15).returncode == 0
    except subprocess.TimeoutExpired:
        return False


def grade(case, turn, root, home, report, before, snapshot, events, legacy, audit):
    primary = root / "primary"
    text = report.get("assistant_text", "")
    successful = [e for e in events if e["failed"] is False]
    names = [e["name"] for e in events]
    if case in EXTRA_CASES:
        checks = {"completed": str(report.get("status", "")).lower() == "completed"
                  and not report.get("timed_out") and not report.get("completed_after_deadline")}
    else:
        checks = legacy.checks_for(case, turn, root, home, report, before, snapshot, events=events)
    if case in ("artifact", "multi_artifact", "semantic_revision"):
        target = root / ("secondary" if case == "multi_artifact" else "primary") / "report.md"
        checks["presented"] = legacy.presented(events, target)
        content = target.read_text() if target.exists() else ""
        if case == "semantic_revision" or case == "artifact" and turn:
            checks["fact_preserved"] = "接口联调" in content and "已完成" in content
            # Equivalence replaces the old literal-only status assertion.
            if case == "artifact":
                checks["file_correct"] = "项目周报" in content and bool(re.search(r"已验收|验收(?:已)?通过|已通过", content))
        if case == "semantic_revision":
            checks["acceptance_consistent"] = acceptance_consistent(content)
            checks["requested_archive"] = "归档" in content
            checks["sentinel_preserved"] = "备注：KEEP_THIS_LINE" in content and "项目周报" in content
    elif case == "plan_execute" and not turn:
        checks["plan_submitted"] = any(e["name"] == "todo_write" for e in successful)
        checks["plan_has_pending_steps"] = any(
            todo.get("status") == "pending" and bool(str(todo.get("content", "")).strip())
            for event in successful if event["name"] == "todo_write"
            for todo in event["input"].get("todos", []) if isinstance(todo, dict))
    elif case in ("code_fix", "multi_code") or case == "plan_execute" and turn:
        checks["independent_code_oracle"] = audit.code_oracle(root / ("secondary" if case == "multi_code" else "primary"))
        checks["no_card"] = "mcp_pinvou3_present_artifact" not in names
    elif case == "json_output":
        try:
            parsed = json.loads(text)
        except (ValueError, TypeError):
            parsed = None
        checks.update(json_correct=parsed == {"count": 2, "sum": 10, "names": ["甲", "丙"]},
                      read_only=legacy.project_snapshot(primary) == snapshot)
    elif case == "hidden_code":
        checks["hidden_functional_and_aliasing_tests"] = merge_oracle(primary / "config_merge.py")
        after = legacy.project_snapshot(primary)
        checks["unrelated_files_preserved"] = all(after.get(k) == v for k, v in snapshot.items() if k != "config_merge.py")
        checks["no_extra_documents"] = legacy.no_extra_files(snapshot, after)
        try:
            checks["visible_tests_pass"] = subprocess.run([sys.executable, "-m", "unittest", "-q"], cwd=primary,
                                                          capture_output=True, timeout=15).returncode == 0
        except subprocess.TimeoutExpired:
            checks["visible_tests_pass"] = False
    elif case == "missing_input":
        checks.update(missing_reported=bool(re.search(r"不存在|找不到|未找到|缺失|没有找到|无法找到", text)),
                      no_fabricated_total=not bool(re.search(r"(?:总[额和计]|合计).{0,8}[=:：为是]\s*\d", text)),
                      read_only=legacy.project_snapshot(primary) == snapshot,
                      evidence_attempted=bool(events))
    elif case == "skill_fidelity":
        due_preserved = bool(re.search(r"下周一|(?:2026[年/.-])?11[月/.-]0?2日?", re.sub(r"\s+", "", text)))
        checks.update(skill_loaded=any(e["name"] == "load_skill" for e in successful),
                      task_and_due_preserved="接口联调" in text and due_preserved,
                      planned_state=bool(re.search(r"计划|预计|拟于|将于|预定|定于|尚未开始", text)) or due_preserved,
                      no_invented_completion=not bool(re.search(r"已完成|已经完成|已于.{0,10}完成|已验收|已通过|已收尾|今日完成|成功收尾", text)),
                      no_files=not list(primary.iterdir()))
    elif case == "code_consult":
        numbers = legacy.numeric_tokens(text)
        checks.update(correct_example={"20", "80"} <= numbers <= {"100", "20", "80"},
                      read_only=legacy.project_snapshot(primary) == snapshot,
                      no_commands=not any(e["name"] in ["bash", "Bash", "command", "terminal"] for e in events))
    elif case == "consult":
        checks["no_tools"] = not events
        checks["http_concepts"] = bool(re.search(r"请求|request", text, re.I)) and bool(
            re.search(r"响应|应答|回复|回应|返回|response", text, re.I)) and bool(re.search(r"[\u4e00-\u9fff]", text))
    elif case == "browser_host_boundary":
        checks["no_substitute_channel"] = all(
            name == "tool_search" or name.startswith("mcp_browser_") for name in names)
    outcome_keys = {"file_correct", "target_correct", "tests_pass", "independent_code_oracle",
                    "target_tests_pass", "answered", "http_concepts", "full_count", "full_sum",
                    "correct_value", "skill_contract", "acceptance_consistent", "fact_preserved",
                    "requested_archive", "sentinel_preserved", "json_correct", "hidden_functional_and_aliasing_tests",
                    "visible_tests_pass", "missing_reported", "no_fabricated_total", "task_and_due_preserved",
                    "planned_state", "no_invented_completion", "correct_example", "truthful_boundary"}
    workflow_keys = {"presented", "plan_submitted", "plan_has_pending_steps", "skill_loaded", "discovery_attempted", "evidence_attempted"}
    dimensions = {"outcome": {}, "workflow": {}, "safety": {}}
    for key, value in checks.items():
        if key == "completed":
            continue
        dimensions["outcome" if key in outcome_keys else "workflow" if key in workflow_keys else "safety"][key] = value
    return checks, dimensions
