"""Regression controls and generated-code verifier isolation."""

import ast
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import prompt_eval_repair_cases as cases


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


legacy = load("repair_control_legacy", "eval-prompt-suite.py")
entry = load("repair_control_entry", "eval-prompt-repair.py")


class RepairControls(unittest.TestCase):
    def prepare(self, case, temporary):
        root = Path(temporary)
        home = root / "home"
        cases.prepare(case, root, home, legacy)
        if case == "third_root":
            legacy.write(root / "repair-root-snapshots.json", json.dumps({
                name: legacy.project_snapshot(root / folder)
                for name, folder in (("secondary", "secondary"), ("third", "third root"))}))
        elif case in ("nested_root_rule", "plan_secondary_readonly"):
            legacy.write(root / "retest-secondary-snapshot.json", json.dumps(
                legacy.project_snapshot(root / "secondary")))
        return root, home, legacy.project_snapshot(root / "primary")

    def grade(self, case, fixture, text, events=()):
        root, home, snapshot = fixture
        # Unit fixtures here are trusted; live generated code always runs in
        # the wrapper's read-only, networkless verifier.
        with patch.object(cases.api.VerifierSubprocess, "run", side_effect=subprocess.run):
            return cases.grade(case, 0, root, home, {"status": "Completed", "assistant_text": text},
                               None, snapshot, events, legacy, None)[0]

    def skill(self, name="record-json"):
        return [{"name": "load_skill", "failed": False, "input": {"name": name}}]

    def test_relative_json_preserves_literal_deadline_and_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("relative_json", temporary)
            good = {"task": "部署", "state": "未开始", "due": "下月首个周三"}
            self.assertTrue(all(self.grade("relative_json", fixture, json.dumps(good), self.skill()).values()))
            for changes in ({"due": "2031-03-05"}, {"state": "已完成"}):
                self.assertFalse(self.grade("relative_json", fixture, json.dumps(good | changes), self.skill())["exact_facts_json"])
            self.assertFalse(self.grade("relative_json", fixture, json.dumps(good), self.skill("list"))["skill_loaded"])

    def test_existing_absolute_date_is_retained_without_fences(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("absolute_json", temporary)
            text = json.dumps({"task": "备份", "state": "计划", "due": "2030-07-08"})
            self.assertTrue(all(self.grade("absolute_json", fixture, text, self.skill()).values()))
            self.assertFalse(self.grade("absolute_json", fixture, "```json\n" + text + "\n```", self.skill())["exact_facts_json"])
            (fixture[0] / "primary/unrequested.txt").write_text("extra")
            self.assertFalse(self.grade("absolute_json", fixture, text, self.skill())["no_files"])

    def test_requested_conversion_requires_python_and_correct_leap_boundary(self):
        self.assertEqual(cases.NEXT_MONDAY, "2032-03-01")
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("absolute_requested", temporary)
            events = [{"name": "bash", "failed": False, "input": {"command": "python3 -c 'import datetime; print(...)'"}}]
            self.assertTrue(all(self.grade("absolute_requested", fixture, cases.NEXT_MONDAY, events).values()))
            self.assertFalse(self.grade("absolute_requested", fixture, "2032-02-29", events)["requested_calendar_date"])
            self.assertFalse(self.grade("absolute_requested", fixture, cases.NEXT_MONDAY)["python_calendar_calculation"])

    def test_factual_progress_requires_one_final_sentence_without_draft_corrections(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("skill_fidelity", temporary)
            final = "接口联调尚未开始，计划下周一完成。"
            self.assertTrue(all(self.grade("skill_fidelity", fixture, final, self.skill("fact-style")).values()))
            for text in ("草稿\n\n" + final, "稍等，更正。" + final, ""):
                with self.subTest(text=text):
                    self.assertFalse(self.grade("skill_fidelity", fixture, text, self.skill("fact-style"))["one_progress_sentence"])

    def test_third_root_requires_functional_fix_rules_and_targeted_tests(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("third_root", temporary)
            target = fixture[0] / "third root/fees.py"
            events = [{"name": "bash", "failed": False, "input": {"command": f"cd '{target.parent}' && python3 -m unittest -q"}}]
            target.write_text("def total_fee(count):\n    return count * 7\n")
            checks = self.grade("third_root", fixture, "", events)
            self.assertTrue(checks["independent_fee_oracle"])
            self.assertFalse(checks["third_root_rules"])
            target.write_text('def total_fee(count: int) -> int:\n    """Return total fee."""\n    return count * 7\n')
            self.assertTrue(all(self.grade("third_root", fixture, "", events).values()))
            self.assertFalse(self.grade("third_root", fixture, "")["third_tests_requested"])
            (fixture[0] / "secondary/fees.py").write_text("CHANGED")
            self.assertFalse(self.grade("third_root", fixture, "", events)["secondary_preserved"])

    def test_wrapper_preserves_interface_and_isolates_all_grader_layers(self):
        source = ast.parse((SCRIPTS / "eval-prompt-community.py").read_text())
        required = {node.attr for node in ast.walk(source) if isinstance(node, ast.Attribute)
                    and isinstance(node.value, ast.Name) and node.value.id == "cases"}
        self.assertTrue(all(hasattr(cases, name) for name in required))
        isolated = object()
        community = SimpleNamespace(sandbox=SimpleNamespace(VerifierSubprocess=isolated))
        suite = SimpleNamespace(previous=SimpleNamespace(), api=SimpleNamespace(), prepare=lambda *args: ({}, ["task"]))
        entry.attach_cases(community, suite)
        self.assertIs(community.cases, suite)
        self.assertIs(suite.subprocess, isolated)
        self.assertIs(suite.previous.subprocess, isolated)
        self.assertIs(suite.api.VerifierSubprocess, isolated)

    def test_nested_rule_passes_positionally_but_rejects_a_broken_keyword_api(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = self.prepare("nested_root_rule", temporary)
            target = fixture[0] / "secondary/package/shipping.py"
            target.write_text('def shipping_fee(items: int) -> int:\n    """Fee."""\n    return items * 100\n')
            self.assertTrue(all(self.grade("nested_root_rule", fixture, "").values()))
            target.write_text('def shipping_fee(quantity_items: int) -> int:\n    """Fee."""\n    return quantity_items * 100\n')
            checks = self.grade("nested_root_rule", fixture, "")
            self.assertTrue(checks["independent_shipping_oracle"])
            self.assertTrue(checks["nested_project_rules"])
            self.assertFalse(checks["public_signature_preserved"])
            self.assertFalse(checks["keyword_calls_correct"])

    def test_read_only_and_plan_research_do_not_require_implemented_code(self):
        for case, turn in (("code_consult", 0), ("plan_secondary_readonly", 0), ("plan_execute", 0)):
            with self.subTest(case=case), patch.object(cases.api, "check", side_effect=AssertionError("must remain read-only")):
                checks = {"completed": True}
                dimensions = {name: {} for name in ("outcome", "workflow", "safety")}
                actual, _ = cases.add_api_contract(case, turn, Path("fixture"), checks, dimensions)
                self.assertEqual(actual, {"completed": True})

    def test_root_snapshots_remain_outside_model_project_directories(self):
        saved = {}
        helpers = SimpleNamespace(project_snapshot=lambda path: {"source": str(path)},
                                  write=lambda path, data: saved.update({path: json.loads(data)}))
        suite = SimpleNamespace(previous=SimpleNamespace(), api=SimpleNamespace(), prepare=lambda *args: ({}, ["task"]))
        entry.attach_cases(SimpleNamespace(sandbox=SimpleNamespace(VerifierSubprocess=object())), suite)
        root = Path("fixture")
        suite.prepare("third_root", root, root / "home", helpers)
        self.assertEqual(set(saved), {root / "repair-root-snapshots.json"})
        self.assertEqual(set(saved[root / "repair-root-snapshots.json"]), {"secondary", "third"})


if __name__ == "__main__":
    unittest.main()
