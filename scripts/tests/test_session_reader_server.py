#!/usr/bin/env python3
"""session_reader_server 的纯逻辑与 stdio 协议契约测试。

覆盖「引用对话(Session Mention)」P2 验收点:
- read_session 分页(最新在前、cursor 翻页连续无重复无遗漏);
- 进行中的轮次(尾部无 assistant 消息)不返回;
- 裁剪(max_output_chars_per_item)与 include_outputs 差量;
- 无效 id / sched- / eval_ / 不存在的会话 / 坏文件的显式报错;
- list_sessions 标题搜索与隔离语义;
- stdio 契约:initialize / tools/list / tools/call(newline-delimited JSON-RPC 2.0)。

运行:python3 -m unittest discover -s scripts/tests -p 'test_*.py'
"""
from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER_PATH = (
    ROOT
    / "pinvou3-app"
    / "src-tauri"
    / "resources"
    / "common"
    / "bundle"
    / "mcp-servers"
    / "session_reader_server.py"
)

spec = importlib.util.spec_from_file_location("session_reader_server", SERVER_PATH)
server = importlib.util.module_from_spec(spec)
spec.loader.exec_module(server)


def _msg(role, *blocks):
    return {"role": role, "content": list(blocks)}


def _text(value):
    return {"type": "text", "text": value}


def _tool_use(name, value):
    return {"type": "tool_use", "name": name, "input": {"cmd": value}}


def _tool_result(value):
    return {"type": "tool_result", "content": value}


def _write_session(directory, session_id, messages, title="demo", **metadata):
    payload = {
        "schema_version": 1,
        "metadata": {
            "id": session_id,
            "title": title,
            "created_at": "2026-09-01T00:00:00Z",
            "updated_at": metadata.pop("updated_at", "2026-09-02T00:00:00Z"),
            "message_count": len(messages),
            "model": "test-model",
            "workspace": "D:\\work\\demo",
        },
        "messages": messages,
    }
    payload["metadata"].update(metadata)
    (Path(directory) / f"{session_id}.json").write_text(
        json.dumps(payload, ensure_ascii=False), encoding="utf-8"
    )


def _three_turn_messages():
    """三个完整 turn + 一个进行中 turn(用户已发、模型未答)。"""
    return [
        _msg("user", _text("第一问")),
        _msg("assistant", _text("第一答"), _tool_use("exec_shell", "ls"), _text("答完一")),
        _msg("user", _tool_result("file list")),
        _msg("user", _text("第二问")),
        _msg("assistant", _text("第二答")),
        _msg("user", _text("第三问")),
        _msg("assistant", _text("第三答")),
        _msg("user", _text("进行中的问题")),
    ]


class ReadSessionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = self.tmp.name
        _write_session(self.dir, "abc123", _three_turn_messages(), title="引用源会话")

    def read(self, session_id="abc123", **kwargs):
        payload, error = server.read_session_history(self.dir, session_id, **kwargs)
        self.assertIsNone(error)
        return payload

    def test_newest_first_and_in_progress_turn_excluded(self):
        payload = self.read(turn_limit=10)
        self.assertEqual(payload["title"], "引用源会话")
        self.assertEqual(payload["totalTurns"], 3)
        texts = [turn["userText"] for turn in payload["turns"]]
        self.assertEqual(texts, ["第三问", "第二问", "第一问"])
        self.assertEqual([turn["turnIndex"] for turn in payload["turns"]], [2, 1, 0])
        self.assertFalse(payload["hasMore"])
        self.assertIsNone(payload["nextCursor"])
        self.assertTrue(payload["untrusted"])

    def test_pagination_is_contiguous_without_gaps_or_duplicates(self):
        first = self.read(turn_limit=2)
        self.assertTrue(first["hasMore"])
        self.assertEqual([t["userText"] for t in first["turns"]], ["第三问", "第二问"])
        second = self.read(turn_limit=2, cursor=first["nextCursor"])
        self.assertEqual([t["userText"] for t in second["turns"]], ["第一问"])
        self.assertFalse(second["hasMore"])
        self.assertIsNone(second["nextCursor"])

    def test_invalid_cursor_reports_error(self):
        payload, error = server.read_session_history(self.dir, "abc123", cursor="@@bad@@")
        self.assertIsNone(payload)
        self.assertIn("cursor", error)

    def test_tool_blocks_hidden_by_default_with_count(self):
        payload = self.read(turn_limit=10)
        oldest = payload["turns"][-1]
        self.assertEqual(oldest.get("toolCalls"), 1)
        self.assertFalse(any(item["type"] == "tool_use" for item in oldest["items"]))

    def test_include_outputs_returns_tool_details(self):
        payload = self.read(turn_limit=10, include_outputs=True)
        oldest = payload["turns"][-1]
        kinds = [item["type"] for item in oldest["items"]]
        self.assertIn("tool_use", kinds)
        self.assertIn("tool_result", kinds)
        tool_use = next(item for item in oldest["items"] if item["type"] == "tool_use")
        self.assertEqual(tool_use["name"], "exec_shell")

    def test_max_output_chars_per_item_truncates(self):
        _write_session(self.dir, "longtext", [
            _msg("user", _text("q")),
            _msg("assistant", _text("x" * 5000)),
        ])
        payload, error = server.read_session_history(
            self.dir, "longtext", max_output_chars_per_item=100)
        self.assertIsNone(error)
        text = payload["turns"][0]["items"][0]["text"]
        self.assertEqual(len(text), 100 + len(server.TRUNCATED_MARK))
        self.assertTrue(text.endswith("[truncated]"))

    def test_unknown_block_types_are_skipped(self):
        _write_session(self.dir, "driftcase", [
            _msg("user", _text("q"), {"type": "future_block", "payload": 1}),
            _msg("assistant", {"type": "another_future"}, _text("a")),
        ])
        payload, error = server.read_session_history(self.dir, "driftcase")
        self.assertIsNone(error)
        self.assertEqual(payload["turns"][0]["items"], [{"type": "assistant", "text": "a"}])

    def test_system_role_blocks_become_notes(self):
        _write_session(self.dir, "compact", [
            _msg("user", _text("q")),
            _msg("system", _text("压缩摘要")),
            _msg("assistant", _text("a")),
        ])
        payload, error = server.read_session_history(self.dir, "compact")
        self.assertIsNone(error)
        note = next(item for item in payload["turns"][0]["items"] if item["type"] == "note")
        self.assertEqual(note["role"], "system")

    def test_rejected_session_ids(self):
        for bad in ["", "../etc", "a/b", "sched-xyz", "eval_secret", "with space"]:
            payload, error = server.read_session_history(self.dir, bad)
            self.assertIsNone(payload, bad)
            self.assertIsNotNone(error, bad)

    def test_missing_session_reports_explicit_error(self):
        payload, error = server.read_session_history(self.dir, "nosuchid")
        self.assertIsNone(payload)
        self.assertIn("not found", error)

    def test_corrupt_file_reports_error(self):
        (Path(self.dir) / "broken1.json").write_text("{not json", encoding="utf-8")
        payload, error = server.read_session_history(self.dir, "broken1")
        self.assertIsNone(payload)
        self.assertIn("unreadable", error)


class ListSessionsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = self.tmp.name
        _write_session(self.dir, "aaa111", [], title="修复登录页样式",
                       updated_at="2026-09-10T00:00:00Z")
        _write_session(self.dir, "bbb222", [], title="销量PPT制作",
                       updated_at="2026-09-12T00:00:00Z")
        _write_session(self.dir, "sched-task1", [], title="定时任务",
                       updated_at="2026-09-13T00:00:00Z")
        _write_session(self.dir, "eval_gaia1", [], title="评测",
                       updated_at="2026-09-14T00:00:00Z")

    def test_list_excludes_scheduled_and_eval_sessions(self):
        payload, error = server.list_sessions(self.dir)
        self.assertIsNone(error)
        ids = [entry["sessionId"] for entry in payload["sessions"]]
        self.assertEqual(ids, ["bbb222", "aaa111"])  # 新→旧
        self.assertEqual(payload["total"], 2)

    def test_query_filters_by_title_case_insensitive(self):
        payload, error = server.list_sessions(self.dir, query="PPT")
        self.assertIsNone(error)
        self.assertEqual([e["sessionId"] for e in payload["sessions"]], ["bbb222"])

    def test_limit_is_respected(self):
        payload, error = server.list_sessions(self.dir, limit=1)
        self.assertIsNone(error)
        self.assertEqual(len(payload["sessions"]), 1)
        self.assertEqual(payload["total"], 2)

    def test_metadata_head_extraction_matches_full_parse(self):
        path = Path(self.dir) / "aaa111.json"
        head = server._extract_metadata_head(str(path))
        self.assertIsNotNone(head)
        self.assertEqual(head["title"], "修复登录页样式")


class StdioContractTests(unittest.TestCase):
    """真实拉起 stdio server,验证 initialize/tools/list/tools/call 协议形态。"""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = self.tmp.name
        _write_session(self.dir, "stdio01", [
            _msg("user", _text("你好")),
            _msg("assistant", _text("你好!")),
        ], title="协议会话")

    def _rpc(self, proc, method, params=None, req_id=[0]):
        req_id[0] += 1
        request = {"jsonrpc": "2.0", "id": req_id[0], "method": method}
        if params is not None:
            request["params"] = params
        proc.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        proc.stdin.flush()
        line = proc.stdout.readline()
        self.assertTrue(line, "server 应有响应")
        return json.loads(line)

    def test_stdio_roundtrip(self):
        proc = subprocess.Popen(
            [sys.executable, str(SERVER_PATH), "--sessions-dir", self.dir],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        try:
            init = self._rpc(proc, "initialize", {
                "protocolVersion": server.PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            })
            self.assertEqual(init["result"]["protocolVersion"], server.PROTOCOL_VERSION)
            self.assertEqual(
                init["result"]["serverInfo"]["name"], "pinvou3-session-reader")

            tools = self._rpc(proc, "tools/list")
            names = [tool["name"] for tool in tools["result"]["tools"]]
            self.assertEqual(names, ["read_session", "list_sessions"])

            call = self._rpc(proc, "tools/call", {
                "name": "read_session",
                "arguments": {"session_id": "stdio01"},
            })
            content = call["result"]["content"][0]
            self.assertEqual(content["type"], "text")
            payload = json.loads(content["text"])
            self.assertTrue(payload["ok"])
            self.assertEqual(payload["turns"][0]["userText"], "你好")
            self.assertFalse(call["result"]["isError"])

            missing = self._rpc(proc, "tools/call", {
                "name": "read_session",
                "arguments": {"session_id": "nosuch"},
            })
            error_payload = json.loads(missing["result"]["content"][0]["text"])
            self.assertFalse(error_payload["ok"])
            self.assertTrue(missing["result"]["isError"])
        finally:
            proc.kill()
            proc.communicate()


class SessionsDirResolutionTests(unittest.TestCase):
    def test_cli_arg_wins(self):
        self.assertEqual(
            server.resolve_sessions_dir(["--sessions-dir", "/tmp/x"]), "/tmp/x")

    def test_env_fallback(self):
        old = os.environ.get("PINVOU3_HOME")
        os.environ["PINVOU3_HOME"] = "/tmp/pinvou3home"
        try:
            self.assertEqual(
                server.resolve_sessions_dir([]),
                os.path.join("/tmp/pinvou3home", "sessions"),
            )
        finally:
            if old is None:
                os.environ.pop("PINVOU3_HOME", None)
            else:
                os.environ["PINVOU3_HOME"] = old


if __name__ == "__main__":
    unittest.main()

