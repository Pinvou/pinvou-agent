import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "session-replay-audit.py"
SPEC = importlib.util.spec_from_file_location("session_replay_audit", SCRIPT)
AUDIT = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(AUDIT)


class SessionReplayAuditTests(unittest.TestCase):
    def write_json(self, path: Path, value: object, mtime: int = 1) -> None:
        path.write_text(json.dumps(value), encoding="utf-8")
        os.utime(path, (mtime, mtime))

    def test_load_latest_ignores_metadata_and_restore_backups(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_json(root / "older.json", {}, 10)
            self.write_json(root / "newer.json", {}, 20)
            self.write_json(root / "_session_models.json", {}, 30)
            self.write_json(root / "abc.pre-restore-123.json", {}, 40)

            self.assertEqual(
                [path.name for path in AUDIT.load_latest(5, root)],
                ["newer.json", "older.json"],
            )

    def test_audit_counts_tools_user_input_and_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "session.json"
            self.write_json(
                path,
                {
                    "metadata": {"id": "s1", "title": "fixture", "message_count": 3},
                    "messages": [
                        {"role": "user", "content": "生成报告"},
                        {
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "tool_use",
                                    "id": "ask-1",
                                    "name": "request_user_input",
                                    "input": {},
                                },
                                {
                                    "type": "tool_use",
                                    "id": "write-1",
                                    "name": "write_file",
                                    "input": {"path": "relative/report.md"},
                                },
                                {
                                    "type": "tool_use",
                                    "id": "present-1",
                                    "name": "mcp_pinvou3_present_artifact",
                                    "input": {"path": "relative/report.md"},
                                },
                            ],
                        },
                        {
                            "role": "user",
                            "content": [
                                {"type": "tool_result", "tool_use_id": "ask-1"},
                                {"type": "tool_result", "tool_use_id": "write-1"},
                                {"type": "tool_result", "tool_use_id": "present-1"},
                            ],
                        },
                    ],
                },
            )

            summary, warnings, errors = AUDIT.audit_session(path, strict_fs=True)

            self.assertEqual(errors, [])
            self.assertEqual(warnings, [])
            self.assertEqual(summary["tool_uses"], 3)
            self.assertEqual(summary["tool_results"], 3)
            self.assertEqual(summary["request_user_input"], 1)
            self.assertEqual(summary["request_user_input_answered"], 1)
            self.assertEqual(summary["artifact_refs"], 2)

    def test_audit_rejects_unknown_tool_result(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "broken.json"
            self.write_json(
                path,
                {
                    "metadata": {"id": "broken", "message_count": 1},
                    "messages": [
                        {
                            "role": "user",
                            "content": [
                                {"type": "tool_result", "tool_use_id": "missing"}
                            ],
                        }
                    ],
                },
            )

            _, _, errors = AUDIT.audit_session(path, strict_fs=False)

            self.assertEqual(
                errors,
                ["tool_result references unknown tool_use id: missing"],
            )

    def test_audit_corrupt_json_returns_render_safe_summary(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "corrupt.json"
            path.write_text("{not-json", encoding="utf-8")

            summary, warnings, errors = AUDIT.audit_session(path, strict_fs=False)

            self.assertEqual(warnings, [])
            self.assertEqual(summary["messages"], 0)
            self.assertEqual(summary["tool_uses"], 0)
            self.assertEqual(summary["tool_results"], 0)
            self.assertEqual(summary["errors"], 1)
            self.assertEqual(len(errors), 1)

    def test_missing_file_text_mode_fails_without_traceback(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            missing = Path(tmp) / "missing.json"
            completed = subprocess.run(
                [sys.executable, str(SCRIPT), str(missing)],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(completed.returncode, 1)
            self.assertIn("FAIL missing", completed.stdout)
            self.assertIn("cannot read JSON", completed.stdout)
            self.assertNotIn("Traceback", completed.stderr)


if __name__ == "__main__":
    unittest.main()
