#!/usr/bin/env python3
"""Pure-logic and stdio protocol contract tests for the session-reader (preset marketplace package) server.py.

Covers the session-mention P2 acceptance points:
- read_session pagination (newest first; cursor paging is contiguous without
  duplicates or gaps);
- in-flight turns (a trailing turn with no assistant message) are not returned;
- clipping (max_output_chars_per_item) and the include_outputs delta;
- explicit errors for invalid ids / sched- / eval_ / aux- / missing sessions /
  corrupt files;
- list_sessions title search and isolation semantics;
- stdio contract: initialize / ping / tools/list / tools/call
  (newline-delimited JSON-RPC 2.0);
- aggregate response budget (truncated: true stays pageable) and per-turn
  item cap;
- containment (symlink escape rejected), file-size ceiling, and sanitized
  error messages (no absolute paths);
- feature-switch fallback (docs/builtin-toolset-contract.md §3.3): a
  structured feature_disabled error when all dependent features are off;
  union semantics; missing/corrupt manifest or state file allows the call.

Run: python3 -m unittest discover -s scripts/tests -p 'test_*.py'
"""
from __future__ import annotations

import builtins
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER_PATH = (
    ROOT
    / "pinvou3-app"
    / "resources"
    / "mcp-servers"
    / "session-reader"
    / "server.py"
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
    """Three complete turns + one in-flight turn (user sent, model has not answered)."""
    return [
        _msg("user", _text("first question")),
        _msg("assistant", _text("first answer"), _tool_use("exec_shell", "ls"), _text("first answer done")),
        _msg("user", _tool_result("file list")),
        _msg("user", _text("second question")),
        _msg("assistant", _text("second answer")),
        _msg("user", _text("third question")),
        _msg("assistant", _text("third answer")),
        _msg("user", _text("in-flight question")),
    ]


class ReadSessionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = self.tmp.name
        _write_session(self.dir, "abc123", _three_turn_messages(), title="referenced session")

    def read(self, session_id="abc123", **kwargs):
        payload, error = server.read_session_history(self.dir, session_id, **kwargs)
        self.assertIsNone(error)
        return payload

    def test_newest_first_and_in_progress_turn_excluded(self):
        payload = self.read(turn_limit=10)
        self.assertEqual(payload["title"], "referenced session")
        self.assertEqual(payload["totalTurns"], 3)
        texts = [turn["userText"] for turn in payload["turns"]]
        self.assertEqual(texts, ["third question", "second question", "first question"])
        self.assertEqual([turn["turnIndex"] for turn in payload["turns"]], [2, 1, 0])
        self.assertFalse(payload["hasMore"])
        self.assertIsNone(payload["nextCursor"])
        self.assertTrue(payload["untrusted"])

    def test_pagination_is_contiguous_without_gaps_or_duplicates(self):
        first = self.read(turn_limit=2)
        self.assertTrue(first["hasMore"])
        self.assertEqual([t["userText"] for t in first["turns"]], ["third question", "second question"])
        second = self.read(turn_limit=2, cursor=first["nextCursor"])
        self.assertEqual([t["userText"] for t in second["turns"]], ["first question"])
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
            _msg("system", _text("compaction summary")),
            _msg("assistant", _text("a")),
        ])
        payload, error = server.read_session_history(self.dir, "compact")
        self.assertIsNone(error)
        note = next(item for item in payload["turns"][0]["items"] if item["type"] == "note")
        self.assertEqual(note["role"], "system")

    def test_rejected_session_ids(self):
        for bad in ["", "../etc", "a/b", "sched-xyz", "eval_secret", "with space",
                    "A" * 300, "abc\n"]:
            payload, error = server.read_session_history(self.dir, bad)
            self.assertIsNone(payload, bad)
            self.assertIsNotNone(error, bad)

    def test_session_id_length_cap_boundary(self):
        # The Rust validator enforces charset only; the Python side adds a
        # length cap so filesystem probes cannot raise ENAMETOOLONG, and
        # anchors with \Z so a trailing newline never passes.
        self.assertIsNone(server.validate_session_id("A" * 128))
        self.assertIsNotNone(server.validate_session_id("A" * 129))
        self.assertIsNotNone(server.validate_session_id("abc\n"))

    def test_missing_session_reports_explicit_error(self):
        payload, error = server.read_session_history(self.dir, "nosuchid")
        self.assertIsNone(payload)
        self.assertIn("not found", error)

    def test_corrupt_file_reports_error(self):
        (Path(self.dir) / "broken1.json").write_text("{not json", encoding="utf-8")
        payload, error = server.read_session_history(self.dir, "broken1")
        self.assertIsNone(payload)
        self.assertIn("unreadable", error)

    def test_aux_prefix_sessions_are_rejected_case_insensitive(self):
        # Isolation prefixes (sched-/eval_/aux-) are rejected
        # case-insensitively in the read path (contract §4.3/§5).
        for bad in ["aux-chat1", "AUX-chat1", "SCHED-x", "EVAL_x"]:
            payload, error = server.read_session_history(self.dir, bad)
            self.assertIsNone(payload, bad)
            self.assertIn("not readable", error, bad)

    def test_symlink_escape_is_rejected(self):
        # A symlink inside the sessions directory pointing outside must not be
        # followed: the resolved path leaves the store and reads as not found.
        outside = Path(self.tmp.name).parent / f"outside-{os.getpid()}.json"
        outside.write_text(json.dumps({"metadata": {"title": "secret"}, "messages": []}),
                           encoding="utf-8")
        self.addCleanup(outside.unlink, True)
        link = Path(self.dir) / "escape1.json"
        try:
            os.symlink(outside, link)
        except (OSError, NotImplementedError) as exc:
            self.skipTest(f"symlinks unavailable on this platform: {exc}")
        payload, error = server.read_session_history(self.dir, "escape1")
        self.assertIsNone(payload)
        self.assertIn("not found", error)

    def test_oversize_session_file_is_rejected(self):
        _write_session(self.dir, "bigone", [
            _msg("user", _text("q")),
            _msg("assistant", _text("a")),
        ])
        old_limit = server.MAX_SESSION_FILE_BYTES
        server.MAX_SESSION_FILE_BYTES = 16  # shrink the cap instead of writing 64 MiB
        try:
            payload, error = server.read_session_history(self.dir, "bigone")
        finally:
            server.MAX_SESSION_FILE_BYTES = old_limit
        self.assertIsNone(payload)
        self.assertIn("too large", error)

    def test_error_messages_do_not_leak_absolute_paths(self):
        # OSError text embeds the absolute path; the tool response must not.
        # (Reliably raising OSError from a real file is platform-dependent —
        # chmod is a no-op on Windows — so inject the failure instead.)
        _write_session(self.dir, "locked1", [
            _msg("user", _text("q")),
            _msg("assistant", _text("a")),
        ])
        real_open = builtins.open

        def boom(*args, **kwargs):
            raise OSError("[Errno 13] Permission denied: '%s'"
                          % os.path.join(self.dir, "locked1.json"))

        builtins.open = boom
        try:
            payload, error = server.read_session_history(self.dir, "locked1")
        finally:
            builtins.open = real_open
        self.assertIsNone(payload)
        self.assertIn("unreadable", error)
        self.assertNotIn(self.dir, error)

    def test_response_budget_truncates_page_but_stays_pageable(self):
        # With a tiny aggregate budget the page stops filling after the first
        # (always-included) turn, reports truncated: true, and nextCursor
        # still walks through the remaining turns without gaps.
        old_budget = server.MAX_RESPONSE_BYTES
        server.MAX_RESPONSE_BYTES = 1
        try:
            first = self.read(turn_limit=10)
            self.assertTrue(first["truncated"])
            self.assertEqual(len(first["turns"]), 1)
            self.assertEqual(first["turns"][0]["userText"], "third question")
            self.assertTrue(first["hasMore"])
            second = self.read(turn_limit=10, cursor=first["nextCursor"])
            self.assertEqual([t["userText"] for t in second["turns"]], ["second question"])
            self.assertTrue(second["truncated"])
            third = self.read(turn_limit=10, cursor=second["nextCursor"])
            self.assertEqual([t["userText"] for t in third["turns"]], ["first question"])
            # The last page's single turn also exceeded the degenerate budget
            # and was shrunk — truncated stays honest even when hasMore is
            # already false.
            self.assertTrue(third["truncated"])
            self.assertFalse(third["hasMore"])
            self.assertIsNone(third["nextCursor"])
        finally:
            server.MAX_RESPONSE_BYTES = old_budget

    def test_single_oversized_turn_is_shrunk_to_budget(self):
        # One turn of 200 assistant blocks at the per-item ceiling reaches
        # ~4 MB: the first-turn-always-included rule must not bypass the
        # aggregate budget — the turn is shrunk to fit instead.
        _write_session(self.dir, "hugeturn", [
            _msg("user", _text("q")),
            _msg("assistant", *[_text("x" * 20000) for _ in range(200)]),
        ])
        payload, error = server.read_session_history(
            self.dir, "hugeturn", turn_limit=5, max_output_chars_per_item=20000)
        self.assertIsNone(error)
        turns_blob = json.dumps(payload["turns"], ensure_ascii=False).encode("utf-8")
        self.assertLessEqual(
            len(turns_blob), server.MAX_RESPONSE_BYTES,
            "the shaped turns must respect the aggregate response budget")
        self.assertTrue(payload["truncated"])
        self.assertTrue(payload["turns"][0].get("itemsTruncated"))
        # The only turn was included: paging is complete, no cursor.
        self.assertFalse(payload["hasMore"])
        self.assertIsNone(payload["nextCursor"])

    def test_oversized_first_turn_keeps_paging_gap_free(self):
        # Two oversized turns: page 1 carries the shrunk newest turn with
        # truncated: true and a strictly advancing cursor; page 2 carries the
        # remaining turn (also shrunk) and terminates — no infinite loop.
        big_blocks = [_text("x" * 20000) for _ in range(200)]
        _write_session(self.dir, "twobig", [
            _msg("user", _text("first")),
            _msg("assistant", *big_blocks),
            _msg("user", _text("second")),
            _msg("assistant", *big_blocks),
        ])
        kwargs = {"turn_limit": 5, "max_output_chars_per_item": 20000}
        first, error = server.read_session_history(self.dir, "twobig", **kwargs)
        self.assertIsNone(error)
        self.assertEqual([t["userText"] for t in first["turns"]], ["second"])
        self.assertTrue(first["truncated"])
        self.assertTrue(first["hasMore"])
        self.assertLessEqual(
            len(json.dumps(first["turns"], ensure_ascii=False).encode("utf-8")),
            server.MAX_RESPONSE_BYTES)
        second, error = server.read_session_history(
            self.dir, "twobig", cursor=first["nextCursor"], **kwargs)
        self.assertIsNone(error)
        self.assertEqual([t["userText"] for t in second["turns"]], ["first"])
        self.assertTrue(second["truncated"])
        self.assertFalse(second["hasMore"])
        self.assertIsNone(second["nextCursor"])
        # Gap-free: the two pages cover both turns exactly once.
        self.assertNotEqual(
            first["turns"][0]["turnIndex"], second["turns"][0]["turnIndex"])

    def test_is_file_oserror_is_sanitized(self):
        # Path.is_file() can raise (e.g. ENAMETOOLONG); the response must be
        # the same sanitized error as the stat/open failures — the raw OSError
        # embeds the absolute host path and must never leak.
        _write_session(self.dir, "locked2", [
            _msg("user", _text("q")),
            _msg("assistant", _text("a")),
        ])
        real_is_file = Path.is_file

        def boom(_self):
            raise OSError("[Errno 36] File name too long: '%s'"
                          % os.path.join(self.dir, "locked2.json"))

        Path.is_file = boom
        try:
            payload, error = server.read_session_history(self.dir, "locked2")
        finally:
            Path.is_file = real_is_file
        self.assertIsNone(payload)
        self.assertIn("unreadable", error)
        self.assertNotIn(self.dir, error)

    def test_deeply_nested_json_reports_unreadable(self):
        # RecursionError from pathologically nested JSON must honor the
        # sanitized `unreadable` contract, not escape as a raw -32603.
        # Whether json.load actually raises RecursionError on deep input is
        # platform-dependent (interpreter limit / C stack), so drive the
        # contract deterministically by forcing the raise.
        import unittest.mock as mock

        (Path(self.dir) / "nested1.json").write_text("{}", encoding="utf-8")
        with mock.patch.object(
            server.json, "load", side_effect=RecursionError("too deep")):
            payload, error = server.read_session_history(self.dir, "nested1")
        self.assertIsNone(payload)
        self.assertIn("unreadable", error)
        self.assertNotIn(self.dir, error)

        # The real 50000-deep input must never crash or leak paths either;
        # both parser outcomes (RecursionError, or a successfully parsed
        # non-dict) land on sanitized errors.
        (Path(self.dir) / "nested2.json").write_text(
            "[" * 50000 + "]" * 50000, encoding="utf-8")
        payload, error = server.read_session_history(self.dir, "nested2")
        self.assertIsNone(payload)
        self.assertIsNotNone(error)
        self.assertNotIn(self.dir, error)

    def test_strict_bool_arg_parsing(self):
        # bool("false") would be True; the strict parser must not be fooled.
        self.assertFalse(server._parse_bool_arg("false"))
        self.assertTrue(server._parse_bool_arg("true"))
        self.assertTrue(server._parse_bool_arg(True))
        self.assertFalse(server._parse_bool_arg(False))
        self.assertFalse(server._parse_bool_arg(1))
        self.assertFalse(server._parse_bool_arg(None))

    def test_normal_page_is_not_marked_truncated(self):
        payload = self.read(turn_limit=10)
        self.assertFalse(payload["truncated"])

    def test_per_turn_item_cap_marks_turn(self):
        _write_session(self.dir, "manyitems", [
            _msg("user", _text("q")),
            _msg("assistant", _text("a1"), _text("a2"), _text("a3")),
        ])
        old_cap = server.MAX_ITEMS_PER_TURN
        server.MAX_ITEMS_PER_TURN = 1
        try:
            payload, error = server.read_session_history(self.dir, "manyitems")
        finally:
            server.MAX_ITEMS_PER_TURN = old_cap
        self.assertIsNone(error)
        turn = payload["turns"][0]
        self.assertEqual(len(turn["items"]), 1)
        self.assertTrue(turn["itemsTruncated"])


class ListSessionsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = self.tmp.name
        _write_session(self.dir, "aaa111", [], title="fix login page styles",
                       updated_at="2026-09-10T00:00:00Z")
        _write_session(self.dir, "bbb222", [], title="sales PPT deck",
                       updated_at="2026-09-12T00:00:00Z")
        _write_session(self.dir, "sched-task1", [], title="scheduled task",
                       updated_at="2026-09-13T00:00:00Z")
        _write_session(self.dir, "eval_gaia1", [], title="benchmark eval",
                       updated_at="2026-09-14T00:00:00Z")

    def test_list_excludes_scheduled_and_eval_sessions(self):
        payload, error = server.list_sessions(self.dir)
        self.assertIsNone(error)
        ids = [entry["sessionId"] for entry in payload["sessions"]]
        self.assertEqual(ids, ["bbb222", "aaa111"])  # newest first
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
        self.assertEqual(head["title"], "fix login page styles")

    def test_corrupt_message_count_does_not_kill_listing(self):
        # A corrupt app-written message_count must coerce to 0 for that entry,
        # not raise ValueError and kill the whole listing.
        _write_session(self.dir, "ccc333", [], title="corrupt count",
                       updated_at="2026-09-11T00:00:00Z", message_count="lots")
        payload, error = server.list_sessions(self.dir)
        self.assertIsNone(error)
        entry = next(e for e in payload["sessions"] if e["sessionId"] == "ccc333")
        self.assertEqual(entry["messageCount"], 0)
        self.assertEqual(payload["total"], 3)

    def test_list_excludes_aux_sessions_case_insensitive(self):
        # aux- side-chats join the sched-/eval_ isolation set, case-insensitive.
        _write_session(self.dir, "aux-side1", [], title="side chat",
                       updated_at="2026-09-15T00:00:00Z")
        _write_session(self.dir, "AUX-side2", [], title="side chat 2",
                       updated_at="2026-09-16T00:00:00Z")
        payload, error = server.list_sessions(self.dir)
        self.assertIsNone(error)
        ids = [entry["sessionId"] for entry in payload["sessions"]]
        self.assertEqual(ids, ["bbb222", "aaa111"])
        self.assertEqual(payload["total"], 2)

    def test_list_skips_symlink_escape(self):
        outside = Path(self.tmp.name).parent / f"outside-list-{os.getpid()}.json"
        outside.write_text(json.dumps({"metadata": {"title": "secret"}, "messages": []}),
                           encoding="utf-8")
        self.addCleanup(outside.unlink, True)
        try:
            os.symlink(outside, Path(self.dir) / "escape2.json")
        except (OSError, NotImplementedError) as exc:
            self.skipTest(f"symlinks unavailable on this platform: {exc}")
        payload, error = server.list_sessions(self.dir)
        self.assertIsNone(error)
        ids = [entry["sessionId"] for entry in payload["sessions"]]
        self.assertNotIn("escape2", ids)
        self.assertEqual(payload["total"], 2)

    def test_list_unreadable_dir_error_is_sanitized(self):
        missing = os.path.join(self.dir, "no-such-dir")
        payload, error = server.list_sessions(missing)
        self.assertIsNone(payload)
        self.assertIn("not readable", error)
        # The raw OSError embeds the absolute path; the response must not.
        self.assertNotIn(missing, error)


class FeatureGateTests(unittest.TestCase):
    """Direct pure-function tests of the contract §3.3 feature-switch fallback (union semantics + tolerant pass-through)."""

    TOOL_FEATURES = {
        "mcp_session-reader_read_session": ["session-mention", "long-memory"],
        "mcp_session-reader_list_sessions": ["session-mention", "long-memory"],
    }

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        self.sessions_dir = self.home / "sessions"
        self.sessions_dir.mkdir()
        (self.home / "marketplace").mkdir()

    def write_state(self, disabled):
        (self.home / "marketplace" / "builtin_features.json").write_text(
            json.dumps({"schema_version": 1, "disabled_features": disabled}),
            encoding="utf-8",
        )

    def gate(self, tool="read_session"):
        return server.feature_gate_error(
            tool, str(self.sessions_dir), self.TOOL_FEATURES)

    def test_all_features_disabled_returns_feature_disabled(self):
        self.write_state(["session-mention", "long-memory"])
        payload = self.gate()
        self.assertIsNotNone(payload)
        self.assertFalse(payload["ok"])
        self.assertEqual(payload["code"], "feature_disabled")
        self.assertIn("session-mention", payload["error"])
        self.assertIn("long-memory", payload["error"])
        self.assertTrue(payload["alternative"])

    def test_partial_disable_allows_call_union_semantics(self):
        # Many-to-many union semantics: disabling only some of the dependent
        # features keeps the tool callable.
        self.write_state(["session-mention"])
        self.assertIsNone(self.gate())
        self.assertIsNone(self.gate("list_sessions"))

    def test_no_features_disabled_allows_call(self):
        self.write_state([])
        self.assertIsNone(self.gate())

    def test_missing_state_file_allows_call(self):
        # Missing state file = all enabled (tolerant); even a registered
        # feature does not gate.
        self.assertIsNone(self.gate())

    def test_corrupt_state_file_allows_call(self):
        (self.home / "marketplace" / "builtin_features.json").write_text(
            "{not json", encoding="utf-8")
        self.assertIsNone(self.gate())

    def test_unregistered_tool_is_not_gated(self):
        self.write_state(["session-mention", "long-memory"])
        self.assertIsNone(self.gate("some_other_tool"))
        self.assertIsNone(server.feature_gate_error(
            "read_session", str(self.sessions_dir), {}))

    def test_load_tool_features_missing_or_corrupt_manifest(self):
        self.assertEqual(server.load_tool_features(self.home / "no-such.json"), {})
        bad = self.home / "bad.json"
        bad.write_text("{not json", encoding="utf-8")
        self.assertEqual(server.load_tool_features(bad), {})
        no_field = self.home / "nofield.json"
        no_field.write_text(json.dumps({"id": "x"}), encoding="utf-8")
        self.assertEqual(server.load_tool_features(no_field), {})

    def test_load_tool_features_reads_mapping(self):
        manifest = self.home / "manifest.json"
        manifest.write_text(json.dumps({"tool_features": self.TOOL_FEATURES}),
                            encoding="utf-8")
        self.assertEqual(server.load_tool_features(manifest), self.TOOL_FEATURES)

    def test_full_tool_name(self):
        self.assertEqual(
            server.full_tool_name("read_session"),
            "mcp_session-reader_read_session")


class StdioContractTests(unittest.TestCase):
    """Spawns the real stdio server and verifies the initialize/tools/list/tools/call protocol shapes."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = self.tmp.name
        _write_session(self.dir, "stdio01", [
            _msg("user", _text("hello")),
            _msg("assistant", _text("hello!")),
        ], title="protocol session")

    def _rpc(self, proc, method, params=None, req_id=[0]):
        req_id[0] += 1
        request = {"jsonrpc": "2.0", "id": req_id[0], "method": method}
        if params is not None:
            request["params"] = params
        proc.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        proc.stdin.flush()
        line = proc.stdout.readline()
        self.assertTrue(line, "server should have responded")
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
            self.assertEqual(payload["turns"][0]["userText"], "hello")
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

    def _spawn(self):
        return subprocess.Popen(
            [sys.executable, str(SERVER_PATH), "--sessions-dir", self.dir],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )

    def test_ping_returns_empty_result(self):
        # MCP keepalive convention: ping answers with an empty result object.
        proc = self._spawn()
        try:
            response = self._rpc(proc, "ping")
            self.assertEqual(response["result"], {})
        finally:
            proc.kill()
            proc.communicate()

    def test_unknown_tool_reports_invalid_params(self):
        # The method is tools/call; an unknown tool NAME is a bad parameter
        # (-32602), not an unknown method (-32601).
        proc = self._spawn()
        try:
            response = self._rpc(proc, "tools/call", {
                "name": "no_such_tool", "arguments": {}})
            self.assertEqual(response["error"]["code"], -32602)
        finally:
            proc.kill()
            proc.communicate()

    def test_non_utf8_bytes_on_stdin_do_not_kill_server(self):
        # A single non-UTF-8 byte must be skipped (tolerant decode), and the
        # next valid request still gets an answer.
        proc = self._spawn()
        try:
            proc.stdin.buffer.write(b"\xff\xfe not json\n")
            proc.stdin.buffer.flush()
            response = self._rpc(proc, "ping")
            self.assertEqual(response["result"], {})
        finally:
            proc.kill()
            proc.communicate()

    def test_oversized_session_id_is_rejected_without_path_leak(self):
        # A legal-charset id past the length cap must be rejected by
        # validation (never reaching the filesystem probes), and the response
        # must contain no filesystem path.
        proc = self._spawn()
        try:
            request = {"jsonrpc": "2.0", "id": 42, "method": "tools/call",
                       "params": {"name": "read_session",
                                  "arguments": {"session_id": "A" * 300}}}
            proc.stdin.write(json.dumps(request) + "\n")
            proc.stdin.flush()
            line = proc.stdout.readline()
            self.assertTrue(line, "server should have responded")
            self.assertNotIn(self.dir, line)
            response = json.loads(line)
            payload = json.loads(response["result"]["content"][0]["text"])
            self.assertFalse(payload["ok"])
            self.assertIn("invalid session_id", payload["error"])
        finally:
            proc.kill()
            proc.communicate()

    def test_catch_all_never_leaks_exception_text(self):
        # The -32603 catch-all must not interpolate the raw exception: its
        # message can embed absolute host paths. Only the exception type name
        # is reported.
        import io
        import unittest.mock as mock

        secret_path = os.path.join(self.dir, "secret-user-path")

        class FakeStdin:
            buffer = [b'{"jsonrpc":"2.0","id":7,"method":"ping"}\n']

        def boom(*_args):
            raise OSError("blew up at %s" % secret_path)

        out = io.StringIO()
        with mock.patch.object(server, "_handle", side_effect=boom), \
                mock.patch("sys.stdin", FakeStdin()), \
                mock.patch("sys.stdout", out):
            server.main()
        line = out.getvalue().strip()
        self.assertTrue(line, "the catch-all should have answered the request")
        response = json.loads(line)
        self.assertEqual(response["id"], 7)
        self.assertEqual(response["error"]["code"], -32603)
        self.assertIn("OSError", response["error"]["message"])
        self.assertNotIn(secret_path, line)
        self.assertNotIn(self.dir, line)


class FeatureGateStdioTests(unittest.TestCase):
    """Contract §3.3 at the stdio layer: spawns the real server and verifies the feature_disabled error channel.

    The server reads manifest.json next to its __file__, and the real
    manifest's tool_features field was added by parallel development; to keep
    this test self-contained, server.py is copied into a temp directory
    alongside a manifest carrying tool_features. The sessions directory is
    relocated via PINVOU3_HOME, and the switch state is written to
    <PINVOU3_HOME>/marketplace/builtin_features.json.
    """

    TOOL_FEATURES = FeatureGateTests.TOOL_FEATURES

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        sessions_dir = self.home / "sessions"
        sessions_dir.mkdir()
        _write_session(sessions_dir, "gate01", [
            _msg("user", _text("gated question")),
            _msg("assistant", _text("gated answer")),
        ], title="gated session")
        (self.home / "marketplace").mkdir()
        # Server copy to spawn + a manifest carrying tool_features
        self.server_dir = self.home / "pkg"
        self.server_dir.mkdir()
        self.server_copy = self.server_dir / "server.py"
        shutil.copyfile(SERVER_PATH, self.server_copy)
        (self.server_dir / "manifest.json").write_text(
            json.dumps({"id": "session-reader", "tool_features": self.TOOL_FEATURES}),
            encoding="utf-8",
        )

    def write_state(self, disabled):
        (self.home / "marketplace" / "builtin_features.json").write_text(
            json.dumps({"schema_version": 1, "disabled_features": disabled}),
            encoding="utf-8",
        )

    def _rpc(self, proc, method, params=None, req_id=[0]):
        req_id[0] += 1
        request = {"jsonrpc": "2.0", "id": req_id[0], "method": method}
        if params is not None:
            request["params"] = params
        proc.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        proc.stdin.flush()
        line = proc.stdout.readline()
        self.assertTrue(line, "server should have responded")
        return json.loads(line)

    def _start(self):
        env = dict(os.environ)
        env["PINVOU3_HOME"] = str(self.home)
        return subprocess.Popen(
            [sys.executable, str(self.server_copy)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            env=env,
        )

    def _call_read_session(self, proc):
        return self._rpc(proc, "tools/call", {
            "name": "read_session",
            "arguments": {"session_id": "gate01"},
        })

    def test_all_features_disabled_returns_feature_disabled(self):
        self.write_state(["session-mention", "long-memory"])
        proc = self._start()
        try:
            call = self._call_read_session(proc)
            self.assertTrue(call["result"]["isError"])
            payload = json.loads(call["result"]["content"][0]["text"])
            self.assertFalse(payload["ok"])
            self.assertEqual(payload["code"], "feature_disabled")
            self.assertIn("session-mention", payload["error"])
            self.assertTrue(payload["alternative"])
        finally:
            proc.kill()
            proc.communicate()

    def test_partial_disable_allows_normal_result(self):
        # Only one feature off (union semantics not triggered): the same tool
        # answers normally.
        self.write_state(["session-mention"])
        proc = self._start()
        try:
            call = self._call_read_session(proc)
            self.assertFalse(call["result"]["isError"])
            payload = json.loads(call["result"]["content"][0]["text"])
            self.assertTrue(payload["ok"])
            self.assertEqual(payload["turns"][0]["userText"], "gated question")
        finally:
            proc.kill()
            proc.communicate()

    def test_missing_state_file_allows_normal_result(self):
        proc = self._start()
        try:
            call = self._call_read_session(proc)
            self.assertFalse(call["result"]["isError"])
            payload = json.loads(call["result"]["content"][0]["text"])
            self.assertTrue(payload["ok"])
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

