"""Positive and negative controls for the fresh V2.2 transfer-case graders."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import prompt_eval_retest_cases as cases


spec = importlib.util.spec_from_file_location("retest_legacy", SCRIPTS / "eval-prompt-suite.py")
legacy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(legacy)


class RetestGraderTests(unittest.TestCase):
    def test_extension_preserves_the_unchanged_runner_case_interface(self):
        import ast
        source = ast.parse((SCRIPTS / "eval-prompt-community.py").read_text())
        required = {node.attr for node in ast.walk(source)
                    if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name)
                    and node.value.id == "cases"}
        self.assertEqual(required, {"EXTRA_CASES", "prepare", "grade", "acceptance_consistent", "subprocess", "__file__"})
        self.assertTrue(all(hasattr(cases, name) for name in required))

    def prepare(self, case, temporary):
        root = Path(temporary)
        home = root / "home"
        cases.prepare(case, root, home, legacy)
        if (root / "secondary").exists():
            legacy.write(root / "retest-secondary-snapshot.json", json.dumps(legacy.project_snapshot(root / "secondary")))
        return root, home, legacy.project_snapshot(root / "primary")

    def grade(self, case, fixture, text="", events=()):
        root, home, snapshot = fixture
        return cases.grade(case, 0, root, home,
                           {"status": "Completed", "assistant_text": text},
                           None, snapshot, events, legacy, None)[0]

    def test_nested_rules_and_functional_results_are_both_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("nested_root_rule", temporary)
            path = fixture[0] / "secondary/package/shipping.py"
            path.write_text('def shipping_fee(items: int) -> int:\n    """Return shipping cost."""\n    return items + 100\n')
            checks = self.grade("nested_root_rule", fixture)
            self.assertTrue(checks["nested_project_rules"])
            self.assertFalse(checks["independent_shipping_oracle"])
            path.write_text("def shipping_fee(items):\n    return items * 100\n")
            checks = self.grade("nested_root_rule", fixture)
            self.assertFalse(checks["nested_project_rules"])
            self.assertTrue(checks["independent_shipping_oracle"])
            path.write_text('def shipping_fee(items: int) -> int:\n    """Return shipping cost."""\n    return items * 100\n')
            self.assertTrue(all(self.grade("nested_root_rule", fixture).values()))

    def test_secondary_revision_needs_delivery_and_primary_preservation(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("secondary_revision", temporary)
            root = fixture[0]
            target = root / "secondary/report.md"
            target.write_text(target.read_text().replace("待批准", "已批准"))
            self.assertFalse(self.grade("secondary_revision", fixture)["presented"])
            events = [{"name": "mcp_pinvou3_present_artifact", "failed": False, "input": {"path": str(target)}}]
            self.assertTrue(all(self.grade("secondary_revision", fixture, events=events).values()))
            (root / "primary/report.md").write_text("CHANGED")
            self.assertFalse(self.grade("secondary_revision", fixture, events=events)["primary_preserved"])

    def test_json_escaping_accepts_exact_values_but_not_markdown(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("json_escape", temporary)
            text = json.dumps({"labels": ['A "quoted"', "返金"], "net": 12}, ensure_ascii=False)
            self.assertTrue(all(self.grade("json_escape", fixture, text).values()))
            self.assertFalse(self.grade("json_escape", fixture, "```json\n" + text + "\n```")["exact_json"])

    def test_relative_date_and_skill_output_contract(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("relative_deadline", temporary)
            text = "STATUS_NOTE: 验收测试尚未开始，计划下周四提交。"
            events = [{"name": "load_skill", "failed": False, "input": {}}]
            self.assertTrue(all(self.grade("relative_deadline", fixture, text, events).values()))
            self.assertFalse(self.grade("relative_deadline", fixture, text + "（1月7日）", events)["no_added_calendar_date"])
            self.assertFalse(self.grade("relative_deadline", fixture, "改写如下：" + text, events)["format_preserved"])

    def test_plan_cannot_pass_after_command_or_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("plan_secondary_readonly", temporary)
            events = [{"name": "todo_write", "failed": False, "input": {"todos": [
                {"content": "修正运费计算为件数乘以 100，并补充类型和文档", "status": "pending"},
                {"content": "在附加目录运行 unittest 验证", "status": "pending"}]}}]
            self.assertTrue(all(self.grade("plan_secondary_readonly", fixture, events=events).values()))
            with_command = [*events, {"name": "bash", "failed": True, "input": {"command": "ls"}}]
            self.assertFalse(self.grade("plan_secondary_readonly", fixture, events=with_command)["no_commands"])
            (fixture[0] / "secondary/package/shipping.py").write_text("CHANGED")
            self.assertFalse(self.grade("plan_secondary_readonly", fixture, events=events)["secondary_read_only"])

    def test_due_date_rejects_conflicts_without_rejecting_reference_date(self):
        for text in ("计划下周一完成", "计划2026-11-02完成", "计划11月2日完成",
                     "参照2026-10-26，计划下周一完成", "2026-10-26 为参照，计划下周一完成"):
            self.assertTrue(cases.monday_due_date_consistent(text), text)
        for text in ("下周一（2026-10-27）完成", "下周一（10月31日）完成", "计划2027-11-02完成"):
            self.assertFalse(cases.monday_due_date_consistent(text), text)

    def test_unbound_delivery_checks_the_real_session_workspace(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("unbound_artifact", temporary)
            root, home, snapshot = fixture
            target = home / "sessions/test-session/workspace/report.md"
            legacy.write(target, "# 项目周报\n接口联调：已完成\n当前状态：待验收\n")
            report = {"status": "Completed", "session_id": "test-session"}
            def grade(turn, events):
                return cases.grade("unbound_artifact", turn, root, home, report, None, snapshot, events, legacy, None)[0]
            self.assertFalse(grade(0, [])["presented"])
            events = [{"name": "mcp_pinvou3_present_artifact", "failed": False, "input": {"path": str(target)}}]
            self.assertTrue(all(grade(0, events).values()))
            target.write_text(target.read_text().replace("待验收", "已验收"))
            self.assertTrue(all(grade(1, events).values()))


if __name__ == "__main__":
    unittest.main()
