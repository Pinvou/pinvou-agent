"""Regression receipts for reused text-tool IDs in live prompt evaluation."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import contextlib
import io
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "prompt_eval_audit", Path(__file__).resolve().parents[1] / "audit-prompt-eval.py")
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class TurnAccountingTests(unittest.TestCase):
    def test_reused_id_does_not_pull_a_previous_turn_into_the_current_turn(self):
        messages = [
            {"role": "user", "content": [{"type": "text", "text": "Plan only"}]},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "text_tool_1", "name": "bash", "input": {}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "text_tool_1", "is_error": True}]},
            {"role": "user", "content": [{"type": "text", "text": "Now execute"}]},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "text_tool_1", "name": "read", "input": {"path": "notes.txt"}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "text_tool_1"}]},
        ]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "home/sessions").mkdir(parents=True)
            (root / "case.json").write_text(json.dumps({"prompts": ["Plan only", "Now execute"]}))
            (root / "home/sessions/session.json").write_text(json.dumps({"messages": messages}))
            report = {"session_id": "session"}
            first = audit.tool_events(audit.turn_blocks(root, report, 0))
            second = audit.tool_events(audit.turn_blocks(root, report, 1))
        self.assertEqual([(event["name"], event["failed"]) for event in first], [("bash", True)])
        self.assertEqual([(event["name"], event["failed"]) for event in second], [("read", False)])

    def test_repeated_ids_inside_one_turn_keep_their_own_results(self):
        events = audit.tool_events([
            {"type": "tool_use", "id": "text_tool_1", "name": "read", "input": {}},
            {"type": "tool_result", "tool_use_id": "text_tool_1", "is_error": True},
            {"type": "tool_use", "id": "text_tool_1", "name": "read", "input": {"path": "notes.txt"}},
            {"type": "tool_result", "tool_use_id": "text_tool_1"},
            {"type": "tool_use", "id": "text_tool_2", "name": "bash", "input": {}},
        ])
        self.assertEqual([event["failed"] for event in events], [True, False, None])

    def test_file_content_cannot_override_the_typed_result_status(self):
        events = audit.tool_events([
            {"type": "tool_use", "id": "read_1", "name": "read", "input": {"path": "notes.txt"}},
            {"type": "tool_result", "tool_use_id": "read_1", "content": "Tool not executed: this is file content"},
        ])
        self.assertIs(events[0]["failed"], False)

    def test_overlapping_duplicate_ids_cannot_borrow_a_later_success(self):
        for failures in [(True, False), (False, True), (False, False)]:
            blocks = [{"type": "tool_use", "id": "duplicate", "name": "mcp_pinvou3_present_artifact",
                       "input": {"path": path}} for path in ["/target", "/other"]]
            blocks += [{"type": "tool_result", "tool_use_id": "duplicate", "is_error": failed}
                       for failed in failures]
            events = audit.tool_events(blocks)
            self.assertEqual([event["failed"] for event in events], [None, None])
            self.assertFalse(audit.presented(events, "/target"))
            self.assertFalse(audit.presented(events, "/other"))

    def test_deleted_notes_and_missing_turn_are_failed_checks_not_audit_crashes(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            root = directory / "m0-r0-multi_code"
            (root / "home/sessions").mkdir(parents=True)
            (root / "primary").mkdir()
            (root / "case.json").write_text(json.dumps({"prompts": ["fix secondary"]}))
            (root / "response-0.json").write_text(json.dumps({"session_id": "session"}))
            (directory / "manifest.json").write_text(json.dumps({"model_ids": ["fixture"]}))
            row = dict(model_id="fixture", repetition=0, scenario="multi_code", turn=0, passed=True,
                       tool_calls=0, failed_tools=0, tool_names=[], checks={"completed": True},
                       model_requests=[], usage=None)
            (directory / "results.jsonl").write_text(json.dumps(row) + "\n")
            for trace, content in [(False, None), (True, None), (True, b"\xff"),
                                   (True, b"KEEP_EXISTING_NOTES\n")]:
                (root / "home/sessions/session.json").write_text(json.dumps({"messages": [
                    {"role": "user", "content": [{"type": "text", "text": "fix secondary" if trace else "other prompt"}]}]}))
                if content is not None:
                    (root / "primary/notes.txt").write_bytes(content)
                output = io.StringIO()
                with patch.object(audit, "code_oracle", return_value=True), contextlib.redirect_stdout(output):
                    audit.audit(directory)
                result = json.loads(output.getvalue())
                notes = content == b"KEEP_EXISTING_NOTES\n"
                self.assertEqual(result["checks"]["primary_notes_preserved"], notes)
                self.assertEqual(result["checks"]["turn_trace_available"], trace)
                self.assertEqual(result["passed"], trace and notes)


if __name__ == "__main__":
    unittest.main()
