"""Regression receipts for reused text-tool IDs in live prompt evaluation."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


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


if __name__ == "__main__":
    unittest.main()
