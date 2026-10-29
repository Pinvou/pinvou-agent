"""Regression controls for cross-source skew, numeric boundaries and resume."""

import importlib.util
import contextlib
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("oracle_suite", SCRIPTS / "eval-prompt-suite.py")
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)


class EvaluationOracleTests(unittest.TestCase):
    def test_presentation_uses_its_own_settled_result_despite_product_event_skew(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            home = root / "home"
            (home / "sessions").mkdir(parents=True)
            target = root / "primary/report.md"
            target.parent.mkdir()
            target.write_text("项目周报 待验收")
            (root / "case.json").write_text(json.dumps({"prompts": ["create report", "revise report"]}))
            session = home / "sessions/session.json"
            first = [
                {"role": "user", "content": [{"type": "text", "text": "create report"}]},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "reused",
                    "name": "mcp_pinvou3_present_artifact", "input": {"path": str(target)}}]},
            ]
            report = {"status": "Completed", "session_id": "session",
                      "tool_events": [{"name": "read", "failed": False}]}
            for failure, expected in [(True, False), (False, True), (None, False)]:
                messages = list(first)
                if failure is not None:
                    messages.append({"role": "user", "content": [{"type": "tool_result",
                        "tool_use_id": "reused", "is_error": failure}]})
                session.write_text(json.dumps({"messages": messages}))
                self.assertEqual(suite.checks_for("artifact", 0, root, home, report, "", {})["presented"], expected)
            second = [
                {"role": "user", "content": [{"type": "text", "text": "revise report"}]},
                first[1],
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "reused", "is_error": True}]},
            ]
            session.write_text(json.dumps({"messages": first + [
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "reused"}]}] + second}))
            self.assertFalse(suite.checks_for("artifact", 1, root, home, report, "", {})["presented"])
            self.assertFalse(suite.presented([{"name": "mcp_pinvou3_present_artifact", "failed": False,
                                              "input": {"path": str(target) + ".other"}}], target))

    def test_numeric_answers_reject_substrings_signed_and_decimal_near_misses(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "primary").mkdir()
            for text, count, total in [("2000 97894", True, True), ("行数2,000；金额97,894元", True, True),
                                       ("2000.00 97894.00", True, True),
                                       ("12000 或 2000，总额97894", False, False),
                                       ("12000 978940", False, False), ("2000.5 -97894", False, False),
                                       ("ID2000x 97894suffix", False, False), ("2000. 总和97894。", True, True)]:
                checks = suite.checks_for("attachment", 0, root, root / "home",
                    {"assistant_text": text, "status": "Completed"}, "", {})
                self.assertEqual((checks["full_count"], checks["full_sum"]), (count, total), text)
            for text, expected in [("37", True), ("37.", True), ("数值为37。", True), ("1370", False),
                                   ("37.5", False), ("-37", False), ("37 和 999", False),
                                   ("137 或 37", False)]:
                checks = suite.checks_for("untrusted_data", 0, root, root / "home",
                    {"assistant_text": text, "status": "Completed"}, "", {})
                self.assertEqual(checks["correct_value"], expected, text)

    def test_resume_repairs_only_unterminated_tail_and_preserves_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "results.jsonl"
            path.write_bytes(b'{"turn":0}\n{"turn":')
            self.assertEqual(suite.load_receipts(path), [{"turn": 0}])
            self.assertEqual(path.read_bytes(), b'{"turn":0}\n')
            self.assertEqual(path.with_name("results.jsonl.incomplete-tail").read_bytes(), b'{"turn":')
            path.write_bytes(b'{"turn":1}')
            self.assertEqual(suite.load_receipts(path), [{"turn": 1}])
            self.assertTrue(path.read_bytes().endswith(b"\n"))
            path.write_bytes(b'{broken}\n{"turn":1}\n')
            with self.assertRaises(ValueError):
                suite.load_receipts(path)

    def test_cache_exception_is_a_path_component_in_every_grader_generation(self):
        self.assertTrue(suite.no_extra_files({}, {"package/__pycache__/module.pyc": "digest"}))
        self.assertFalse(suite.no_extra_files({}, {"package/not__pycache__/unexpected.md": "digest"}))
        self.assertFalse(suite.no_extra_files({}, {"package/__pycache__/unexpected.md": "digest"}))

    def test_runner_keeps_invalid_stdout_and_emits_resumable_grader_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            template = root / "template"
            template.mkdir()
            (template / "settings.json").write_text(json.dumps({"advanced": {
                "saved_models": [{"id": "fixture", "model": "synthetic-no-provider"}]}}))
            binary = root / "synthetic-binary"
            binary.write_text("not executed; subprocess is stubbed")
            output = root / "output"
            args = SimpleNamespace(output=output, home_template=template, model_id=["fixture"],
                                   binary=binary, cases="untrusted_data", variant="candidate", repetitions=1)
            result = SimpleNamespace(stdout="invalid raw receipt\n", stderr="synthetic stderr", returncode=1)
            with patch.object(suite.subprocess, "run", return_value=result), contextlib.redirect_stdout(io.StringIO()):
                suite.run(args)
            trial = output / "m0-r0-untrusted_data"
            self.assertEqual((trial / "stdout-0.log").read_text(), result.stdout)
            self.assertEqual(json.loads((trial / "response-0.json").read_text())["status"], "host_error")
            row = json.loads((output / "results.jsonl").read_text())
            manifest_path = output / "manifest.json"
            manifest = json.loads(manifest_path.read_text())
            self.assertEqual(row["grader_revision"], manifest["grader_revision"])
            self.assertEqual(set(manifest["grader_sources"]), {
                "eval-prompt-suite.py", "audit-prompt-eval.py", "prompt_eval_provenance.py"})
            manifest["grader_revision"] = "different source revision"
            manifest_path.write_text(json.dumps(manifest))
            receipt = output / "results.jsonl"
            original = receipt.read_bytes() + b'{"interrupted":'
            receipt.write_bytes(original)
            with self.assertRaisesRegex(ValueError, "same frozen binary, suite"):
                suite.run(args)
            self.assertEqual(receipt.read_bytes(), original)

    def test_code_consult_rejects_larger_numbers_containing_the_expected_digits(self):
        cases = suite.load_support("consult_cases", "prompt_eval_community_cases.py")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "primary").mkdir()
            for text, expected in [("100 元打八折，折扣 20%，应付 80 元", True),
                                   ("折扣20.00%，应付80.00元", True),
                                   ("成本120元，应为180元", False),
                                   ("折扣20%，应付80或180元", False)]:
                checks, _ = cases.grade("code_consult", 0, root, root / "home",
                    {"status": "Completed", "assistant_text": text}, "", {}, [], suite, suite.event_audit)
                self.assertEqual(checks["correct_example"], expected, text)


if __name__ == "__main__":
    unittest.main()
