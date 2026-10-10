#!/usr/bin/env python3
"""Pure-logic and stdio protocol contract tests for the app-automations (preset marketplace package) server.py.

Covers the design doc's acceptance matrix
(docs/app-automations-定时任务创建工具-设计与验收.md §5/§6):
- rrule product subset (B1/B5/B8): HOURLY/WEEKLY/ONCE accepted and normalized
  (trim + uppercase); CRON (B3), minute-granular (B2), unknown keys, out-of-range
  values rejected with actionable errors;
- ONCE AT strictly local YYYY-MM-DDTHH:MM, no timezone suffix, future only (B4);
- field caps and required-field validation (B6/B7);
- create spool write: atomic JSON, idempotency-key dedup (C1), unique uuid
  names without a key (C2), sanitized errors (D4);
- short synchronous wait: marker hit returns the task ids (A2); timeout
  returns delivery:"pending", never an error (A3); a failed creation surfaces
  through the marker (C4 server side);
- list: projections only (never the prompt), tolerant of corrupt files
  written concurrently (C5), lazy/missing store (C6);
- feature-switch fallback (F2): structured feature_disabled for the family's
  feature (scheduled-task-automation), union semantics, tolerant missing state;
- stdio contract (D6): initialize/ping/tools/list/tools/call, unknown tool
  -32602, bad lines skipped, catch-all never leaks exception text.

Run: python3 -m unittest discover -s scripts/tests -p 'test_*.py'
"""
from __future__ import annotations

import builtins
import datetime
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER_PATH = (
    ROOT
    / "pinvou3-app"
    / "resources"
    / "mcp-servers"
    / "app-automations"
    / "server.py"
)

spec = importlib.util.spec_from_file_location("app_automations_server", SERVER_PATH)
server = importlib.util.module_from_spec(spec)
spec.loader.exec_module(server)


def _future_at(minutes=60):
    at = datetime.datetime.now() + datetime.timedelta(minutes=minutes)
    return at.strftime("%Y-%m-%dT%H:%M")


def _write_automation(directory, task_id, **overrides):
    record = {
        "schema_version": 3,
        "id": task_id,
        "name": "早报任务",
        "prompt": "机密的提示词内容不应出现在 list 输出",
        "rrule": "FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30",
        "model": "deepseek-chat",
        "status": "active",
        "created_at": "2026-09-01T00:00:00Z",
        "updated_at": "2026-09-02T00:00:00Z",
        "next_run_at": "2026-09-29T08:30:00Z",
    }
    record.update(overrides)
    (Path(directory) / "automations").mkdir(parents=True, exist_ok=True)
    (Path(directory) / "automations" / f"{task_id}.json").write_text(
        json.dumps(record, ensure_ascii=False), encoding="utf-8"
    )


class RruleValidationTests(unittest.TestCase):
    """B1-B5/B8: the product subset, normalization, and rejections."""

    def test_valid_product_shapes_pass_and_normalize(self):
        cases = [
            ("FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
             "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30"),
            ("  freq=weekly;byday=mo,we;byhour=9;byminute=30  ",
             "FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30"),
            ("FREQ=ONCE;AT=%s" % _future_at(), None),
            ("FREQ=HOURLY", None),
            ("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=0;BYMINUTE=59", None),
        ]
        for rrule, expected in cases:
            normalized, error = server.validate_rrule(rrule)
            self.assertIsNone(error, rrule)
            self.assertEqual(normalized, expected or rrule.strip().upper())

    def test_minute_granular_and_daily_rejected_with_guidance(self):
        for rrule in [
            "FREQ=MINUTELY;INTERVAL=5",
            "FREQ=SECONDLY;INTERVAL=30",
            "FREQ=DAILY;BYHOUR=8;BYMINUTE=30",
        ]:
            _, error = server.validate_rrule(rrule)
            self.assertIsNotNone(error, rrule)
            self.assertIn("not supported", error, rrule)
            # B2: the error must guide toward the supported shapes.
            self.assertIn("HOURLY", error, rrule)

    def test_cron_rejected(self):
        _, error = server.validate_rrule("FREQ=CRON;EXPR=*/5 * * * *")
        self.assertIsNotNone(error)
        self.assertIn("CRON", error)

    def test_once_at_past_and_timezone_suffix_rejected(self):
        for rrule in [
            "FREQ=ONCE;AT=2020-01-01T09:30",
            "FREQ=ONCE;AT=%sZ" % _future_at(),
            "FREQ=ONCE;AT=%s+08:00" % _future_at(),
            "FREQ=ONCE;AT=%s:30" % _future_at(),
            "FREQ=ONCE;AT=2099-02-30T09:30",
            "FREQ=ONCE",
            "FREQ=ONCE;AT=%s;EXTRA=1" % _future_at(),
        ]:
            _, error = server.validate_rrule(rrule)
            self.assertIsNotNone(error, rrule)

    def test_once_gap_and_ambiguous_wall_times_rejected(self):
        """Round-24 minor 2: DST parity with the watcher's chrono
        `.single()` — a spring-forward gap and a fall-back overlap are both
        rejected. The old naive strptime accepted both and the spooled
        record deterministically poisoned after the full wait (the model
        got an opaque ok:false). Skipped where the IANA database is
        absent (plain Windows); the watcher remains the authority there."""
        import zoneinfo

        try:
            tz = zoneinfo.ZoneInfo("America/New_York")
        except Exception:
            self.skipTest("no IANA tz database on this host")
        # 2026-03-08 02:30 US/Eastern does not exist (spring forward);
        # 2026-11-01 01:30 exists twice (fall back).
        gap_error = server._once_wall_time_gap_or_ambiguous(
            datetime.datetime(2026, 3, 8, 2, 30), tz=tz
        )
        self.assertIsNotNone(gap_error)
        self.assertIn("does not exist", gap_error)
        amb_error = server._once_wall_time_gap_or_ambiguous(
            datetime.datetime(2026, 11, 1, 1, 30), tz=tz
        )
        self.assertIsNotNone(amb_error)
        self.assertIn("ambiguous", amb_error)
        self.assertIsNone(
            server._once_wall_time_gap_or_ambiguous(
                datetime.datetime(2026, 6, 1, 9, 30), tz=tz
            ),
            "an ordinary local minute must pass",
        )

    def test_validate_rrule_drives_the_dst_gate_end_to_end(self):
        """Round-26 M6(a): the gap/ambiguous parity above is pinned on the
        HELPER only — deleting the production call inside `validate_rrule`
        (the round-24 minor 2 fix site) kept the whole suite green. This
        drives a DST ONCE wall time through the production entry with the
        zone injected the way production resolves it ($TZ)."""
        import zoneinfo

        try:
            zoneinfo.ZoneInfo("America/New_York")
        except Exception:
            self.skipTest("no IANA tz database on this host")
        old_tz = os.environ.get("TZ")
        os.environ["TZ"] = "America/New_York"
        try:
            _, gap_error = server.validate_rrule("FREQ=ONCE;AT=2026-03-08T02:30")
            _, amb_error = server.validate_rrule("FREQ=ONCE;AT=2026-11-01T01:30")
            # Round-27 minor 3: the passing arm uses the file's
            # _future_at() helper — a hardcoded wall time failed the
            # future check once it passed, a deterministic suite red.
            future_at = _future_at(minutes=365)
            normalized, ok_error = server.validate_rrule("FREQ=ONCE;AT=%s" % future_at)
        finally:
            if old_tz is None:
                os.environ.pop("TZ", None)
            else:
                os.environ["TZ"] = old_tz
        self.assertIsNotNone(gap_error, "the spring-forward gap must be rejected")
        self.assertIn("does not exist", gap_error)
        self.assertIsNotNone(amb_error, "the fall-back overlap must be rejected")
        self.assertIn("ambiguous", amb_error)
        self.assertIsNone(ok_error, "an ordinary future local minute must pass")
        self.assertEqual(normalized, "FREQ=ONCE;AT=%s" % future_at)

    def test_out_of_range_and_unknown_fields_rejected(self):
        for rrule in [
            "FREQ=HOURLY;INTERVAL=0",
            "FREQ=HOURLY;INTERVAL=abc",
            "FREQ=HOURLY;BYHOUR=24",
            "FREQ=HOURLY;BYMINUTE=60",
            "FREQ=WEEKLY;BYDAY=XX;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=8",
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=24;BYMINUTE=0",
            "FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=60",
            # Round-30 REQUIRED-1: empty BYDAY tokens (trailing/double
            # comma) are invalid, not skippable — filtering them admitted
            # shapes the domain deterministically poisons behind an
            # already-answered ok:true.
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU,;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO,,TU;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=MO,;BYHOUR=8;BYMINUTE=30",
            "FREQ=WEEKLY;BYDAY=,;BYHOUR=8;BYMINUTE=30",
            "FREQ=HOURLY;WILDCARD=1",
            "FREQ=MONTHLY;BYDAY=1MO",
            "INTERVAL=5",
            "FREQ=HOURLY;noseparator",
        ]:
            _, error = server.validate_rrule(rrule)
            self.assertIsNotNone(error, rrule)


class CreateValidationTests(unittest.TestCase):
    """B6/B7: field caps and required fields, at and beyond the boundary."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="pinvou-create-test-")
        self.requests = Path(self.tmp) / "task-requests"

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def create(self, **overrides):
        args = {
            "requests_dir": str(self.requests),
            "name": "早报",
            "prompt": "汇总新闻",
            "rrule": "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
        }
        args.update(overrides)
        return server.create_scheduled_task(**args)

    def test_required_fields_are_validated(self):
        for override in ({"name": ""}, {"name": "   "}, {"prompt": ""}, {"rrule": ""}):
            payload, error = self.create(**override)
            self.assertIsNone(payload, override)
            self.assertIn("invalid", error, override)

    def test_create_rejects_caller_supplied_task_id(self):
        """Round-24 minor 6: the server assigns task ids — a caller's
        task_id on create is REJECTED, not silently ignored (the shared
        validator's guard became reachable when the tool binding started
        threading task_id through; contrast delete, which already rejects
        extra fields)."""
        payload, error = self.create(task_id="task-123")
        self.assertIsNone(payload)
        self.assertIn("invalid task_id", error)

    def test_length_caps_boundary(self):
        # Round-24 minor 11: the success arms wait out the result window —
        # patch it (UpdateDeleteRequestTests does the same) so the class
        # stops burning ~20 s of dead wall clock.
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.2
        try:
            payload, error = self.create(name="n" * server.MAX_NAME_CHARS)
            self.assertIsNone(error)
            self.assertIsNotNone(payload)
            payload, error = self.create(name="n" * (server.MAX_NAME_CHARS + 1))
            self.assertIsNone(payload)
            self.assertIn("200", error)
            payload, error = self.create(prompt="p" * server.MAX_PROMPT_CHARS)
            self.assertIsNone(error)
            payload, error = self.create(prompt="p" * (server.MAX_PROMPT_CHARS + 1))
            self.assertIsNone(payload)
            self.assertIn("character limit", error)
            payload, error = self.create(
                idempotency_key="k" * server.MAX_IDEMPOTENCY_KEY_CHARS, from_session="reqsrc01"
            )
            self.assertIsNone(error)
            payload, error = self.create(
                idempotency_key="k" * (server.MAX_IDEMPOTENCY_KEY_CHARS + 1),
                from_session="reqsrc01",
            )
            self.assertIsNone(payload)
        finally:
            server.RESULT_WAIT_SECONDS = old_wait

    def test_rrule_parsing_matches_the_watcher(self):
        # Full-width digits are a classic LLM artifact: Python's int() would
        # accept them while the Rust watcher's u32 parse rejects the record —
        # parse-parity poison. The server must reject them up front.
        payload, error = self.create(rrule="FREQ=HOURLY;INTERVAL=１０")
        self.assertIsNone(payload)
        self.assertIn("must be an integer", error)
        # PEP 515 underscores are legal for int(), never for the watcher.
        payload, error = self.create(rrule="FREQ=HOURLY;INTERVAL=1_0")
        self.assertIsNone(payload)
        self.assertIn("must be an integer", error)
        # Duplicate rrule fields are ambiguous across the language boundary
        # (Python dict last-wins, Rust find first-wins): reject outright.
        payload, error = self.create(rrule="FREQ=ONCE;FREQ=HOURLY;AT=2027-06-01T09:30")
        self.assertIsNone(payload)
        self.assertIn("duplicate field", error)
        # An oversize rrule is a validation error here, not watcher poison.
        payload, error = self.create(rrule="FREQ=HOURLY;INTERVAL=2;" + "X" * 300)
        self.assertIsNone(payload)
        self.assertIn("rrule: exceeds", error)

    def test_invalid_sender_ids_are_rejected(self):
        # Round-15 REQUIRED-1: the case variants pin the server's
        # case-insensitive isolation matching on aux/eval origins.
        for bad in ("../escape", "with space", "a" * 300, "eval_b1", "aux-x1",
                    "EVAL_b1", "AUX-x1"):
            payload, error = self.create(from_session=bad)
            self.assertIsNone(payload, bad)
            self.assertIsNotNone(error, bad)

    def test_interactive_scheduled_origin_supports_keyed_retries(self):
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            for sender in ("sched-run1", "SCHED-run1"):
                payload, error = self.create(from_session=sender, idempotency_key="interactive")
                self.assertIsNone(error)
                self.assertEqual(payload["delivery"], "pending")
        finally:
            server.RESULT_WAIT_SECONDS = old_wait

    def test_normalization_lands_in_spool(self):
        # Round-24 minor 11: patch the result window like
        # test_length_caps_boundary — the success wait here is dead time.
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.2
        try:
            payload, error = self.create(
                rrule=" freq=hourly;interval=6;byhour=8;byminute=30 ",
                model_id="  model-7  ",
            )
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error)
        spooled = list(Path(self.requests, "spool").glob("*.json"))
        self.assertEqual(len(spooled), 1)
        record = json.loads(spooled[0].read_text(encoding="utf-8"))
        self.assertEqual(record["rrule"], "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30")
        self.assertEqual(record["model_id"], "model-7")
        self.assertEqual(record["schema_version"], 1)


class CreateSpoolAndResultTests(unittest.TestCase):
    """A2/A3/C1/C2/C4(spool side)/C6/D4: spool identity, dedup, and the short wait."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="pinvou-spool-test-")
        self.requests = Path(self.tmp) / "task-requests"

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_idempotency_key_requires_from_session(self):
        payload, error = server.create_scheduled_task(**self._create_kwargs(idempotency_key="anon"))
        self.assertIsNone(payload)
        self.assertIn("from_session", error)
        self.assertEqual(len(self._spooled()), 0)

    def test_retry_after_failure_gets_fresh_result_not_stale_error(self):
        # A stale ok:false marker from a previous attempt must not make the
        # retry replay the old error: the server unlinks it on re-spool, so
        # the poll waits for the fresh outcome.
        import hashlib
        import threading

        spool_id = hashlib.sha256(b"reqsrc01|create||k-fresh").hexdigest()
        marker = Path(self.requests, "spool", ".done", "%s.json" % spool_id)
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text(
            json.dumps({"ok": False, "error": "old failure"}), encoding="utf-8"
        )

        def fresh_watcher():
            time.sleep(0.3)
            # Round-9 M1: the fresh marker carries the retry's digest — the
            # real watcher always writes request_digest, and the poll loop
            # now refuses digest-less ok:true markers.
            digest = server.spool_request_digest(
                "create", name="早报", prompt="汇总新闻",
                rrule="FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30", paused=False,
            )
            marker.write_text(
                json.dumps({
                    "ok": True, "task_id": "task-fresh", "task_name": "新任务",
                    "request_digest": digest,
                }),
                encoding="utf-8",
            )

        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 2.0
        thread = threading.Thread(target=fresh_watcher)
        try:
            thread.start()
            payload, error = server.create_scheduled_task(
                **self._create_kwargs(from_session="reqsrc01", idempotency_key="k-fresh")
            )
        finally:
            thread.join()
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error)
        self.assertTrue(payload["ok"], payload)
        self.assertEqual(payload["taskId"], "task-fresh")
        self.assertNotIn("old failure", str(payload))
        # Round-7 minor 1: the recovered attempt is NOT a duplicate — the
        # failure-marker entry must not answer "returning its recorded
        # result"; pin the recovery note.
        self.assertFalse(payload.get("duplicate", False), payload)
        self.assertIn("applied afresh", payload.get("note", ""), payload)

    def test_preexisting_marker_reports_duplicate_result(self):
        import hashlib

        # Round-9 M1: a DIGEST-BOUND preexisting marker answers the
        # recorded duplicate result.
        digest = server.spool_request_digest(
            "create", name="早报", prompt="汇总新闻",
            rrule="FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30", paused=False,
        )
        spool_id = hashlib.sha256(b"reqsrc01|create||k9").hexdigest()
        marker_dir = Path(self.requests, "spool", ".done")
        marker_dir.mkdir(parents=True, exist_ok=True)
        (marker_dir / ("%s.json" % spool_id)).write_text(
            json.dumps({
                "ok": True, "task_id": "task-9", "task_name": "旧任务",
                "request_digest": digest,
            }),
            encoding="utf-8",
        )
        payload, error = server.create_scheduled_task(
            **self._create_kwargs(from_session="reqsrc01", idempotency_key="k9")
        )
        self.assertIsNone(error)
        self.assertTrue(payload["ok"])
        self.assertEqual(payload["taskId"], "task-9")
        self.assertTrue(payload["duplicate"])
        self.assertIn("recorded result", payload["note"])

    def test_digestless_ok_marker_answers_pending_not_a_false_duplicate(self):
        """Round-9 M1: a digest-less ok:true marker at the key's path is
        NOT terminal at entry and NOT acceptable in the poll — the call
        re-spools and answers the honest pending, instead of the round-8
        shape's "no second task was created" while the watcher's digest
        gate re-applied the divergent body (a second task)."""
        import hashlib

        spool_id = hashlib.sha256(b"reqsrc01|create||k-ghost").hexdigest()
        marker_dir = Path(self.requests, "spool", ".done")
        marker_dir.mkdir(parents=True, exist_ok=True)
        (marker_dir / ("%s.json" % spool_id)).write_text(
            json.dumps({"ok": True, "task_id": "task-x", "task_name": "幽灵"}),
            encoding="utf-8",
        )
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.2
        try:
            payload, error = server.create_scheduled_task(
                **self._create_kwargs(from_session="reqsrc01", idempotency_key="k-ghost")
            )
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error)
        self.assertTrue(payload["ok"], payload)
        self.assertEqual(payload.get("delivery"), "pending", payload)
        self.assertIsNone(payload.get("taskId"), payload)

    def test_unkeyed_call_refuses_digestless_ok_marker(self):
        """Round-26 M1: the poll-loop digest gate used to fire on keyed
        calls only, so a planted digest-less {"ok":true} marker answered an
        unkeyed create with a forged receipt ("taskId":"ATTACKER-ID")
        while the watcher — which requires the binding on the same shape —
        applied the request for real (a fabricated receipt plus a
        duplicate task). The gate now covers every call: the forged
        marker keeps the honest pending answer."""
        forged_id = "0f9c1d2e3a4b5c6d7e8f0a1b2c3d4e5f"
        marker_dir = Path(self.requests, "spool", ".done")
        marker_dir.mkdir(parents=True, exist_ok=True)
        (marker_dir / ("%s.json" % forged_id)).write_text(
            json.dumps({"ok": True, "task_id": "ATTACKER-ID", "task_name": "伪造"}),
            encoding="utf-8",
        )

        class _FixedUuid4:
            hex = forged_id

        real_uuid4 = server.uuid.uuid4
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.2
        try:
            server.uuid.uuid4 = lambda: _FixedUuid4
            payload, error = server.create_scheduled_task(
                **self._create_kwargs(from_session="reqsrc01")
            )
        finally:
            server.uuid.uuid4 = real_uuid4
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error)
        self.assertTrue(payload["ok"], payload)
        self.assertEqual(payload.get("delivery"), "pending", payload)
        self.assertIsNone(payload.get("taskId"), payload)

    def test_marker_reader_refuses_non_regular_and_oversize(self):
        """Round-15 REQUIRED-5: _read_result_marker's lstat/S_ISREG/cap gate
        is behaviorally pinned — a symlink (never followed), a FIFO-shaped
        non-regular entry, and an oversize marker are all refused unread (the
        pre-gate shape blocked the single-threaded stdio server forever on a
        FIFO and slurped a sparse file whole)."""
        done_dir = Path(self.requests, "spool", ".done")
        done_dir.mkdir(parents=True, exist_ok=True)
        real = done_dir / "real.json"
        real.write_text('{"ok": true}', encoding="utf-8")

        if os.name == "posix":
            # A symlink to a regular file: never followed (policy: refused).
            link = done_dir / "link.json"
            try:
                os.symlink(real, link)
            except (OSError, NotImplementedError):
                self.skipTest("symlinks unavailable")
            marker, error = server._read_result_marker(link)
            self.assertIsNone(marker)
            # A FIFO at the marker path: refused at lstat, open never runs.
            fifo = done_dir / "fifo.json"
            try:
                os.mkfifo(fifo)
            except (OSError, AttributeError):
                self.skipTest("mkfifo unavailable")
            marker, error = server._read_result_marker(fifo)
            self.assertIsNone(marker)

        # An oversize marker is refused at the stat gate.
        oversize = done_dir / "big.json"
        oversize.write_text("x" * (server.MAX_RESULT_MARKER_BYTES + 1), encoding="utf-8")
        marker, error = server._read_result_marker(oversize)
        self.assertIsNone(marker)

        # The real marker still reads.
        marker, error = server._read_result_marker(real)
        self.assertEqual(marker, {"ok": True})

    def _create_kwargs(self, **overrides):
        args = {
            "requests_dir": str(self.requests),
            "name": "早报",
            "prompt": "汇总新闻",
            "rrule": "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
        }
        args.update(overrides)
        return args

    def _spooled(self):
        return sorted(Path(self.requests, "spool").glob("*.json"))

    def _finish_in_background(self, task_id="task-1", ok=True, delay=0.3,
                              error="simulated watcher failure"):
        """Simulates the Rust watcher: drain the spool, write the marker, remove the file.
        Round-26 M1: the ok-true marker carries the canonical request
        digest (create-side paused coerced to false, like the watcher) —
        production markers always bind the digest and the poll loop now
        requires it on unkeyed calls too, so a digest-less double would
        honestly time out."""

        def worker():
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                files = self._spooled()
                if files:
                    path = files[0]
                    spool_id = path.stem
                    record = json.loads(path.read_text(encoding="utf-8"))
                    payload = (
                        {
                            "ok": True,
                            "task_id": task_id,
                            "task_name": "早报",
                            "request_digest": server.spool_request_digest(
                                record.get("kind"),
                                task_id=record.get("task_id"),
                                name=record.get("name"),
                                prompt=record.get("prompt"),
                                rrule=record.get("rrule"),
                                model_id=record.get("model_id"),
                                paused=(
                                    False
                                    if record.get("kind") == "create"
                                    and record.get("paused") is None
                                    else record.get("paused")
                                ),
                            ),
                        }
                        if ok
                        else {"ok": False, "error": error}
                    )
                    done_dir = Path(self.requests, "spool", ".done")
                    done_dir.mkdir(parents=True, exist_ok=True)
                    (done_dir / f"{spool_id}.json").write_text(
                        json.dumps(payload), encoding="utf-8"
                    )
                    return
                time.sleep(0.05)

        thread = threading.Thread(target=worker, daemon=True)
        thread.start()
        return thread

    def test_queued_create_retry_discloses_the_inflight_duplicate_window(self):
        from unittest.mock import patch
        old_wait = server.RESULT_WAIT_SECONDS
        original_replace = server.os.replace

        def publish_after_replace(source, target):
            original_replace(source, target)
            path = Path(target)
            record = json.loads(path.read_text(encoding="utf-8"))
            marker = path.parent / ".done" / path.name
            marker.parent.mkdir(parents=True, exist_ok=True)
            marker.write_text(json.dumps({
                "ok": True, "task_id": "retry-task", "task_name": record["name"],
                "request_digest": server.spool_request_digest(
                    "create", name=record["name"], prompt=record["prompt"],
                    rrule=record["rrule"], model_id=record.get("model_id"),
                    paused=bool(record.get("paused"))),
            }), encoding="utf-8")

        try:
            server.RESULT_WAIT_SECONDS = 0
            first, error = server.create_scheduled_task(
                **self._create_kwargs(from_session="reqsrc01", idempotency_key="inflight"))
            self.assertIsNone(error)
            self.assertEqual(first["delivery"], "pending")
            server.RESULT_WAIT_SECONDS = 1
            # Publish only AFTER the retry has observed a pending entry and
            # replaced it. No thread can publish before entry classification.
            with patch.object(server.os, "replace", side_effect=publish_after_replace):
                result, error = server.create_scheduled_task(
                    **self._create_kwargs(from_session="reqsrc01", idempotency_key="inflight"))
            self.assertIsNone(error)
            self.assertTrue(result["duplicate"])
            self.assertIn("earlier request may also have been applied", result["note"])
            self.assertNotIn("no second task", result["note"])
        finally:
            server.RESULT_WAIT_SECONDS = old_wait

    def test_failure_marker_redacts_private_roots_before_return(self):
        automations = Path(self.tmp) / "automations"
        raw = ("permission denied: %s/private.json; %s/private.json; %s/secret.txt" %
               (self.requests, automations, Path.home()))
        worker = self._finish_in_background(ok=False, error=raw)
        try:
            payload, error = server.create_scheduled_task(
                **self._create_kwargs(automations_dir=str(automations)))
        finally:
            worker.join(timeout=2)
        self.assertFalse(worker.is_alive())
        self.assertIsNone(payload)
        self.assertIn("permission denied", error)
        for root in (str(self.requests), str(automations), str(Path.home())):
            self.assertNotIn(root, error)
        self.assertIn("<task-requests>", error)
        self.assertIn("<automations>", error)

    def test_identical_keyed_replay_reports_no_mismatch(self):
        """R5-M1: a byte-identical keyed replay against a completed marker
        with the same request_digest answers payload_mismatch:False (and
        duplicate:True) — the earlier task_id-only comparison mis-fired
        here, telling the model its identical retry "was not applied"."""
        import hashlib

        kwargs = self._create_kwargs(from_session="reqsrc01", idempotency_key="k-same")
        digest = server.spool_request_digest(
            "create", name=kwargs["name"], prompt=kwargs["prompt"],
            rrule=kwargs["rrule"], paused=False,
        )
        spool_id = hashlib.sha256(b"reqsrc01|create||k-same").hexdigest()
        marker = Path(self.requests, "spool", ".done", "%s.json" % spool_id)
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text(json.dumps({
            "ok": True, "task_id": "task-1", "task_name": "早报",
            "request_digest": digest,
        }), encoding="utf-8")
        payload, error = server.create_scheduled_task(**kwargs)
        self.assertIsNone(error)
        self.assertTrue(payload["duplicate"])
        self.assertFalse(payload["payload_mismatch"], payload)
        self.assertFalse(payload["payload_mismatch"], payload)
        self.assertNotIn("mismatch_note", payload)

    def test_diverging_keyed_replay_flags_mismatch(self):
        """R5-M1: a keyed replay with a CHANGED payload against a completed
        marker flags payload_mismatch:True with the honest note — the
        earlier task_id-only comparison returned False exactly here."""
        import hashlib

        digest = server.spool_request_digest(
            "create", name="旧名字", prompt="汇总新闻",
            rrule="FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
        )
        spool_id = hashlib.sha256(b"reqsrc01|create||k-diff").hexdigest()
        marker = Path(self.requests, "spool", ".done", "%s.json" % spool_id)
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text(json.dumps({
            "ok": True, "task_id": "task-1", "task_name": "旧名字",
            "request_digest": digest,
        }), encoding="utf-8")
        payload, error = server.create_scheduled_task(
            **self._create_kwargs(
                name="新名字", from_session="reqsrc01", idempotency_key="k-diff"
            )
        )
        self.assertIsNone(error)
        self.assertTrue(payload["payload_mismatch"], payload)
        self.assertIn("DIFFERENT payload", payload["mismatch_note"])

    def test_forged_non_literal_ok_marker_is_not_terminal(self):
        """Round-22 M2: ok:1 / ok:"true" with a MATCHING digest used to be
        classified as success by the server's truthiness check (answering a
        fabricated duplicate while the watcher deleted the marker and
        re-applied). Both server sites bind is-True. Round-24 minor 5
        splits the poll arm by recency: a forged marker OLDER than this
        call's spool is a stale leftover — ignored, the honest answer is
        pending (the model is not told "failed" while the request is
        queued); a forged marker landing AFTER the spool (a live tamper)
        also stays pending. No terminal outcome is fabricated, and the
        record stays queued in either arm."""
        import hashlib

        digest = server.spool_request_digest(
            "create", name="新名字", prompt="汇总新闻",
            rrule="FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
        )
        spool_id = hashlib.sha256(b"reqsrc01|create||k-forged").hexdigest()
        marker = Path(self.requests, "spool", ".done", "%s.json" % spool_id)
        marker.parent.mkdir(parents=True, exist_ok=True)
        spool = Path(self.requests, "spool", "%s.json" % spool_id)
        record_body = json.dumps({
            "schema_version": 1, "id": spool_id, "kind": "create",
            "name": "新名字", "prompt": "汇总新闻",
            "rrule": "FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
            "from_session": "reqsrc01", "idempotency_key": "k-forged",
            "created_at": "2026-10-14T00:00:00Z",
        })
        forged_bodies = (
            {"ok": 1, "task_id": "FORGED-1", "task_name": "伪造",
             "request_digest": digest},
            {"ok": "true", "task_id": "FORGED-1", "task_name": "伪造",
             "request_digest": digest},
        )
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 1.0
        try:
            # Stale shape: planted BEFORE the call — the poll's recency
            # gate ignores it and the call answers pending.
            for forged in forged_bodies:
                marker.write_text(json.dumps(forged), encoding="utf-8")
                spool.write_text(record_body, encoding="utf-8")
                payload, error = server.create_scheduled_task(
                    **self._create_kwargs(
                        name="新名字", from_session="reqsrc01", idempotency_key="k-forged"
                    )
                )
                self.assertIsNone(error)
                self.assertIsNotNone(payload, "the stale forged marker must not hang the answer")
                self.assertEqual(payload.get("delivery"), "pending", payload)
                self.assertIsNone(payload.get("taskId"), payload)
                self.assertTrue(spool.exists(), "the record stays queued")

            # Live-tamper shape: lands MID-POLL (after this call's spool) —
            # newer than entry_seen_at; it must still remain pending.
            for forged in forged_bodies:
                if marker.exists():
                    marker.unlink()
                spool.write_text(record_body, encoding="utf-8")

                def tamper_late(body=forged):
                    time.sleep(0.2)
                    marker.write_text(json.dumps(body), encoding="utf-8")

                threading.Thread(target=tamper_late, daemon=True).start()
                payload, error = server.create_scheduled_task(
                    **self._create_kwargs(
                        name="新名字", from_session="reqsrc01", idempotency_key="k-forged"
                    )
                )
                self.assertIsNone(error, "a malformed marker must not fabricate a terminal failure")
                self.assertEqual(payload.get("delivery"), "pending", payload)
                self.assertTrue(spool.exists(), "the record stays queued")
        finally:
            server.RESULT_WAIT_SECONDS = old_wait

    def test_digest_mismatch_poll_path_sleeps(self):
        """Round-12 MAJOR-5: the keyed poll's digest-mismatch arm sleeps
        between reads at the poll cadence — the bare `continue` used to
        busy-spin the marker re-read for the whole wait window. The pin
        lives at the marker-read boundary (round-17 MAJOR-5): the loop
        must meet a digest-mismatched marker at least once, and the total
        marker reads must stay at the poll cadence — deleting the
        mismatch arm's sleep explodes the read count and turns this red."""
        import hashlib

        digest = server.spool_request_digest(
            "create", name="旧名字", prompt="汇总新闻",
            rrule="FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
        )
        # The digest THIS call's payload binds (新名字) — the late marker
        # carries the 旧名字 digest, so the poll-loop read of it is the
        # mismatch the sleeping continue exists for.
        requested_digest = server.spool_request_digest(
            "create", name="新名字", prompt="汇总新闻",
            rrule="FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
        )
        assert digest != requested_digest, "fixture guard: the two payloads must bind different digests"
        spool_id = hashlib.sha256(b"reqsrc01|create||k-spin").hexdigest()
        marker = Path(self.requests, "spool", ".done", "%s.json" % spool_id)
        # The marker lands MID-POLL (the watcher applying a stale-key apply,
        # say) with a non-matching digest — the entry check saw nothing, so
        # the poll loop is what meets the mismatch and must take the
        # sleeping continue, not a bare one.
        def late_marker():
            time.sleep(0.15)
            marker.parent.mkdir(parents=True, exist_ok=True)
            marker.write_text(json.dumps({
                "ok": True, "task_id": "task-1", "task_name": "旧名字",
                "request_digest": digest,
            }), encoding="utf-8")

        threading.Thread(target=late_marker, daemon=True).start()
        # Round-17 MAJOR-5: the pin lives at the READ boundary, not at
        # time.sleep — a sleep-time marker.exists() check races the
        # late-landing thread (the None arm's sleep can be misattributed
        # to the mismatch window), so the assertion was satisfiable with
        # the mismatch arm's sleep deleted. Count the marker reads
        # instead: the mismatch arm must be met at least once, and the
        # total reads must stay at the poll cadence — deleting the
        # mismatch arm's sleep hot-spins the read count into the tens of
        # thousands and turns this red deterministically.
        mismatch_reads = []
        total_reads = []
        real_read = server._read_result_marker

        def counting_read(done_marker_path):
            result = real_read(done_marker_path)
            total_reads.append(1)
            payload_read = result[0] if isinstance(result, tuple) else result
            if (
                isinstance(payload_read, dict)
                and payload_read.get("ok")
                and payload_read.get("request_digest") != requested_digest
            ):
                mismatch_reads.append(1)
            return result

        server._read_result_marker = counting_read
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.4
        try:
            payload, error = server.create_scheduled_task(
                **self._create_kwargs(
                    name="新名字", from_session="reqsrc01", idempotency_key="k-spin"
                )
            )
        finally:
            server._read_result_marker = real_read
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error)
        self.assertEqual(payload.get("delivery"), "pending", payload)
        self.assertGreaterEqual(
            len(mismatch_reads), 1,
            "the poll loop must meet the digest-mismatched marker in-loop"
        )
        self.assertLessEqual(
            len(total_reads), 25,
            "the poll must read at the poll cadence, not busy-spin: "
            "deleting the mismatch arm's sleep explodes this count "
            "(round-17 MAJOR-5)"
        )

    def test_diverging_replay_against_completed_key_does_not_respool(self):
        """Round-7 follow-up: a completed key is terminal — the divergent
        body must NOT be re-spooled. The prior shape os.replace'd it into the
        queue, where the watcher's digest gate (forged/surgery shape) would
        re-apply it and create a SECOND task while the mismatch note claimed
        the re-apply was suppressed."""
        import hashlib

        digest = server.spool_request_digest(
            "create", name="旧名字", prompt="汇总新闻",
            rrule="FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
        )
        spool_id = hashlib.sha256(b"reqsrc01|create||k-term").hexdigest()
        spool = Path(self.requests, "spool")
        marker = spool.joinpath(".done", "%s.json" % spool_id)
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text(json.dumps({
            "ok": True, "task_id": "task-1", "task_name": "旧名字",
            "request_digest": digest,
        }), encoding="utf-8")
        payload, error = server.create_scheduled_task(
            **self._create_kwargs(
                name="新名字", from_session="reqsrc01", idempotency_key="k-term"
            )
        )
        self.assertIsNone(error)
        self.assertTrue(payload["payload_mismatch"], payload)
        self.assertTrue(payload["duplicate"], payload)
        # The divergent body never entered the queue, and the recorded
        # result (task-1 / the original digest) is what answered.
        self.assertFalse(spool.joinpath("%s.json" % spool_id).exists())
        self.assertEqual(payload["taskId"], "task-1")
        self.assertEqual(
            json.loads(marker.read_text(encoding="utf-8"))["request_digest"],
            digest,
        )

    def test_digest_golden_vectors(self):
        """Round-6 MAJOR 4: committed golden vectors — canonicalization drift
        (separators, key set, coercion, encoding) turns these red because the
        expectations are literal hex, not suite-computed."""
        self.assertEqual(
            server.spool_request_digest(
                "create", name="早报", prompt="汇总",
                rrule="FREQ=DAILY;BYHOUR=8", paused=False,
            ),
            "e3ee5656710820acad7daa187ed50fe7a2295020d3d66f5bd37fc7afea42f755",
        )
        self.assertEqual(
            server.spool_request_digest(
                "update", model_id="m1", paused=True, task_id="t-9",
            ),
            "80d24d63000aa38aa3f4c1866155e42e91d57594189996443d336fc2754bfca6",
        )
        # Round-6 minor 6: non-null target_session vector (both sides —
        # the Rust twin pins the same hex).
        self.assertEqual(
            server.spool_request_digest(
                "update", model_id="m1", paused=True, task_id="t-9",
                target_session="sess-7777",
            ),
            "87f414a68c6034ca1d625fc05f682c8f0811aef01752f9b4cf1d140a5aac6af3",
        )

    def test_short_wait_returns_task_ids(self):
        self._finish_in_background()
        payload, error = server.create_scheduled_task(**self._create_kwargs())
        self.assertIsNone(error)
        self.assertTrue(payload["ok"])
        self.assertEqual(payload["taskId"], "task-1")
        self.assertEqual(payload["taskName"], "早报")
        self.assertFalse(payload["duplicate"])

    def test_wait_timeout_returns_pending_not_error(self):
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.5
        try:
            payload, error = server.create_scheduled_task(**self._create_kwargs())
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error, "a timeout is a pending delivery, not a failure")
        self.assertTrue(payload["ok"])
        self.assertIsNone(payload["taskId"])
        self.assertEqual(payload["delivery"], "pending")
        # The request stays queued for the watcher.
        self.assertEqual(len(self._spooled()), 1)

    def test_failed_marker_surfaces_as_error(self):
        self._finish_in_background(ok=False)
        _, error = server.create_scheduled_task(**self._create_kwargs())
        self.assertIsNotNone(error)
        self.assertIn("simulated watcher failure", error)

    def test_idempotency_key_reuses_one_spool_file(self):
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            first, _ = server.create_scheduled_task(
                **self._create_kwargs(idempotency_key="k1", from_session="reqsrc01")
            )
            second, _ = server.create_scheduled_task(
                **self._create_kwargs(idempotency_key="k1", from_session="reqsrc01")
            )
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertTrue(second["duplicate"])
        self.assertEqual(first["taskId"], second["taskId"])
        self.assertEqual(len(self._spooled()), 1)
        # Sender-scoped namespace (contract §6): another session reusing the
        # key gets its own file.
        try:
            third, _ = server.create_scheduled_task(
                **self._create_kwargs(idempotency_key="k1", from_session="other001")
            )
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertFalse(third["duplicate"])
        self.assertEqual(len(self._spooled()), 2)

    def test_no_key_uses_unique_files(self):
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            server.create_scheduled_task(**self._create_kwargs())
            server.create_scheduled_task(**self._create_kwargs())
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertEqual(len(self._spooled()), 2)

    def test_spool_file_name_is_sender_scoped_sha256(self):
        import hashlib

        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            server.create_scheduled_task(
                **self._create_kwargs(idempotency_key="k1", from_session="reqsrc01")
            )
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        expected = hashlib.sha256(b"reqsrc01|create||k1").hexdigest()
        self.assertEqual(self._spooled()[0].stem, expected)

    def test_spool_write_is_atomic_json(self):
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            server.create_scheduled_task(**self._create_kwargs())
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        record = json.loads(self._spooled()[0].read_text(encoding="utf-8"))
        self.assertEqual(record["schema_version"], 1)
        self.assertEqual(record["kind"], "create")

    def test_spool_errors_do_not_leak_host_paths(self):
        blocker = Path(self.tmp) / "blocked-file"
        blocker.write_bytes(b"x")  # a plain file where the spool dir must be
        _, error = server.create_scheduled_task(
            **self._create_kwargs(requests_dir=str(blocker))
        )
        self.assertIsNotNone(error)
        self.assertNotIn(str(self.tmp), error)

    def test_spool_dir_created_lazily(self):
        self.assertFalse(self.requests.exists())
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            server.create_scheduled_task(**self._create_kwargs())
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertTrue(Path(self.requests, "spool").is_dir())


class ReadScheduledTaskTests(unittest.TestCase):
    """I3: read projects the full detail including the prompt."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="pinvou-read-test-")
        self.automations = Path(self.tmp) / "automations"

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_read_projects_full_fields_including_prompt(self):
        _write_automation(self.automations, "task-1")
        payload, error = server.read_scheduled_task(str(self.automations), "task-1")
        self.assertIsNone(error)
        self.assertEqual(payload["id"], "task-1")
        self.assertEqual(payload["name"], "早报任务")
        self.assertEqual(payload["prompt"], "机密的提示词内容不应出现在 list 输出")
        self.assertEqual(payload["rrule"], "FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30")
        self.assertEqual(payload["status"], "active")
        self.assertEqual(payload["nextRunAt"], "2026-09-29T08:30:00Z")

    def test_read_unknown_and_invalid_ids(self):
        payload, error = server.read_scheduled_task(str(self.automations), "nosuch")
        self.assertIsNone(payload)
        self.assertIn("not found", error)
        for bad in ("../escape", "with space", "a" * 300, ""):
            payload, error = server.read_scheduled_task(str(self.automations), bad)
            self.assertIsNone(payload, bad)
            self.assertIn("invalid task_id", error, bad)

    def test_features_reader_refuses_non_regular_and_oversize(self):
        """Round-18 MAJOR-2: load_disabled_features' gate is behaviorally
        pinned — a symlink, a FIFO, and an oversize builtin_features.json
        are all refused (fail-open empty set). The round-17 gate shipped
        unpinned: deleting the three gate lines reverted to the ungated
        read_text (a planted FIFO at this path wedges the whole stdio
        server) and kept the full suite green — this fixture closes
        that."""
        mkt = Path(self.tmp) / "marketplace"
        mkt.mkdir(parents=True, exist_ok=True)
        gated_path = mkt / "builtin_features.json"
        base_dir = str(self.automations)
        real_payload = json.dumps(
            {"disabled_features": ["scheduled-task-automation"]}
        )

        if os.name == "posix":
            # A symlink at the gated path: refused (fail-open empty set).
            try:
                os.symlink(
                    mkt / "elsewhere.json",
                    gated_path,
                )
            except (OSError, NotImplementedError):
                self.skipTest("symlinks unavailable")
            (mkt / "elsewhere.json").write_text(
                json.dumps({"disabled_features": ["scheduled-task-automation"]})
            )
            self.assertEqual(server.load_disabled_features(base_dir), set())
            os.remove(gated_path)
            # A FIFO at the gated path: refused at lstat, open never runs
            # (the pre-gate shape blocked the single-threaded stdio server
            # forever).
            try:
                os.mkfifo(gated_path)
            except (OSError, AttributeError):
                self.skipTest("mkfifo unavailable")
            self.assertEqual(server.load_disabled_features(base_dir), set())
            os.remove(gated_path)

        # An oversize regular file is refused at the stat gate.
        gated_path.write_text("x" * (server.MAX_FEATURES_FILE_BYTES + 1), encoding="utf-8")
        self.assertEqual(server.load_disabled_features(base_dir), set())
        os.remove(gated_path)

        # The real mapping still reads.
        gated_path.write_text(real_payload, encoding="utf-8")
        self.assertEqual(
            server.load_disabled_features(base_dir),
            {"scheduled-task-automation"},
        )

    def test_read_corrupt_store_entry(self):
        (Path(self.automations, "automations")).mkdir(parents=True)
        (Path(self.automations, "automations", "broken.json")).write_text(
            "{not json", encoding="utf-8"
        )
        payload, error = server.read_scheduled_task(str(self.automations), "broken")
        self.assertIsNone(payload)
        self.assertIn("not readable", error)

    def test_store_reader_refuses_non_regular_and_oversize(self):
        """Round-16 MAJOR-3: the store readers' lstat/S_ISREG/cap gate is
        behaviorally pinned — a symlink, a FIFO, and an oversize record are
        all refused (read -> not found, list -> skipped), and the 256 KiB
        store cap admits a legitimate max-size CJK prompt (~96 KB of UTF-8)
        that the 64 KiB marker cap used to reject as "not found"."""
        store = Path(self.automations, "automations")
        store.mkdir(parents=True, exist_ok=True)
        real = store / "task-real.json"
        real.write_text(
            json.dumps({"id": "task-real", "name": "n", "prompt": "p"}),
            encoding="utf-8",
        )

        if os.name == "posix":
            # A symlink to a regular record: never followed (policy: refused).
            link = store / "task-link.json"
            try:
                os.symlink(real, link)
            except (OSError, NotImplementedError):
                self.skipTest("symlinks unavailable")
            payload, error = server.read_scheduled_task(str(self.automations), "task-link")
            self.assertIsNone(payload)
            self.assertIn("not readable", error)
            # A FIFO at the record path: refused at lstat, open never runs.
            fifo = store / "task-fifo.json"
            try:
                os.mkfifo(fifo)
            except (OSError, AttributeError):
                self.skipTest("mkfifo unavailable")
            payload, error = server.read_scheduled_task(str(self.automations), "task-fifo")
            self.assertIsNone(payload)
            self.assertIn("not readable", error)

        # An oversize record is refused at the stat gate.
        oversize = store / "task-big.json"
        oversize.write_text(
            "x" * (server.MAX_STORE_RECORD_BYTES + 1), encoding="utf-8"
        )
        payload, error = server.read_scheduled_task(str(self.automations), "task-big")
        self.assertIsNone(payload)
        self.assertIn("not readable", error)

        # A legitimate max-size CJK prompt (~96 KB of UTF-8) reads fine
        # under the 256 KiB store cap — the exact record the 64 KiB marker
        # cap used to answer with "not found".
        max_prompt = "\u6c49" * server.MAX_PROMPT_CHARS
        cjk = store / "task-cjk.json"
        cjk.write_text(
            json.dumps(
                {"id": "task-cjk", "name": "早报", "prompt": max_prompt},
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        self.assertGreater(
            cjk.stat().st_size,
            server.MAX_RESULT_MARKER_BYTES,
            "fixture guard: the CJK record must exceed the old marker cap",
        )
        self.assertLessEqual(
            cjk.stat().st_size,
            server.MAX_STORE_RECORD_BYTES,
            "fixture guard: the CJK record must fit the store cap",
        )
        payload, error = server.read_scheduled_task(str(self.automations), "task-cjk")
        self.assertIsNone(error)
        self.assertEqual(payload["prompt"], max_prompt)

        # The list projection skips the refused entries and returns the
        # readable one.
        result, error = server.list_scheduled_tasks(str(self.automations))
        self.assertIsNone(error)
        listed = {task["id"] for task in result["tasks"]}
        self.assertIn("task-cjk", listed)
        self.assertNotIn("task-big", listed)
        if os.name == "posix":
            self.assertNotIn("task-fifo", listed)
            self.assertNotIn("task-link", listed)


class UpdateDeleteRequestTests(unittest.TestCase):
    """I4/I5: kind-aware validation for update and delete, and the spool record shape."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="pinvou-upd-test-")
        self.requests = Path(self.tmp) / "task-requests"
        self.automations = Path(self.tmp) / "automations"
        _write_automation(self.automations, "task-1")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _schedule(self, kind, **overrides):
        args = {
            "requests_dir": str(self.requests),
            "kind": kind,
            "automations_dir": str(self.automations),
            "task_id": "task-1",
        }
        args.update(overrides)
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            return server.schedule_task_request(**args)
        finally:
            server.RESULT_WAIT_SECONDS = old_wait

    def test_update_requires_target_and_at_least_one_field(self):
        payload, error = self._schedule("update", task_id=None)
        self.assertIsNone(payload)
        self.assertIn("invalid task_id", error)
        payload, error = self._schedule("update")
        self.assertIsNone(payload)
        self.assertIn("at least one field", error)
        for bad in ("../escape", "a" * 300):
            payload, error = self._schedule("update", task_id=bad)
            self.assertIsNone(payload, bad)
            self.assertIn("invalid task_id", error, bad)

    def test_update_validates_rrule_and_caps(self):
        payload, error = self._schedule("update", rrule="FREQ=CRON;EXPR=* * * * *")
        self.assertIsNone(payload)
        self.assertIn("CRON", error)
        payload, error = self._schedule("update", name="n" * (server.MAX_NAME_CHARS + 1))
        self.assertIsNone(payload)
        self.assertIn("character limit", error)
        payload, error = self._schedule("update", paused="sometimes")
        self.assertIsNone(payload)
        self.assertIn("invalid paused", error)

    def test_update_spools_kind_and_target(self):
        payload, error = self._schedule("update", name="新名字", paused=True)
        self.assertIsNone(error)
        self.assertEqual(payload["kind"], "update")
        record = json.loads(self._spooled()[0].read_text(encoding="utf-8"))
        self.assertEqual(record["kind"], "update")
        self.assertEqual(record["task_id"], "task-1")
        self.assertEqual(record["name"], "新名字")
        self.assertTrue(record["paused"])
        self.assertIsNone(record["rrule"])

    def test_update_paused_false_counts_as_a_field(self):
        payload, error = self._schedule("update", paused=False)
        self.assertIsNone(error)
        record = json.loads(self._spooled()[0].read_text(encoding="utf-8"))
        self.assertFalse(record["paused"])

    def test_delete_spools_exact_target_without_a_second_confirmation(self):
        target = self.automations / "automations" / "task-1.json"
        original = target.read_bytes()
        payload, error = self._schedule("delete")
        self.assertIsNone(error)
        self.assertEqual(payload["kind"], "delete")
        self.assertEqual(payload["delivery"], "pending")
        record = json.loads(self._spooled()[0].read_text(encoding="utf-8"))
        self.assertEqual(record["task_id"], "task-1")
        self.assertEqual(record["kind"], "delete")
        self.assertEqual(target.read_bytes(), original, "only the app watcher mutates the store")
        _, error = self._schedule("delete", name="unexpected")
        self.assertIn("no extra fields", error)

    def test_missing_delete_target_still_reaches_ensure_absent_domain(self):
        (self.automations / "automations" / "task-1.json").unlink()
        payload, error = self._schedule("delete", idempotency_key="replay", from_session="reqsrc01")
        self.assertIsNone(error)
        self.assertEqual(payload["kind"], "delete")
        record = json.loads(self._spooled()[0].read_text(encoding="utf-8"))
        self.assertEqual(record["task_id"], "task-1")

    def test_completed_delete_receipt_survives_missing_target(self):
        import hashlib
        key = hashlib.sha256(b"reqsrc01|delete|task-1|done").hexdigest()
        done = self.requests / "spool" / ".done"
        done.mkdir(parents=True)
        (done / (key + ".json")).write_text(json.dumps({
            "ok": True, "kind": "delete", "task_id": "task-1", "task_name": "Original",
            "request_digest": server.spool_request_digest("delete", task_id="task-1"),
        }), encoding="utf-8")
        (self.automations / "automations" / "task-1.json").unlink()
        payload, error = self._schedule("delete", idempotency_key="done", from_session="reqsrc01")
        self.assertIsNone(error)
        self.assertTrue(payload["ok"])
        self.assertTrue(payload["duplicate"])
        self.assertFalse(payload["payload_mismatch"])
        self.assertEqual(payload["taskId"], "task-1")
        self.assertEqual(self._spooled(), [], "a completed delete is not requeued")

    def test_overwriting_prompt_or_rrule_requires_panel(self):
        for fields in ({"prompt": "new body"}, {"rrule": "FREQ=HOURLY"}):
            payload, error = self._schedule("update", **fields)
            self.assertIsNone(payload)
            self.assertIn("confirmation_required", error)
            self.assertEqual(self._spooled(), [])

    def test_delete_rejects_stray_paused(self):
        payload, error = self._schedule("delete", paused=True)
        self.assertIsNone(payload)
        self.assertIn("no extra fields", error)

    def test_idempotency_key_scopes_operation_kind(self):
        self._schedule("update", name="a", idempotency_key="k1", from_session="reqsrc01")
        self._schedule("delete", idempotency_key="k1", from_session="reqsrc01")
        self.assertEqual(len(self._spooled()), 2, "same key, different kind: no clobber")
        import hashlib
        expected = hashlib.sha256(b"reqsrc01|delete|task-1|k1").hexdigest()
        self.assertIn(expected, [path.stem for path in self._spooled()])

    def _spooled(self):
        return sorted(Path(self.requests, "spool").glob("*.json"))


class MessageTargetTests(unittest.TestCase):
    """Scheduled-message mode: target validation and the spool field."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="pinvou-target-test-")
        self.requests = Path(self.tmp) / "task-requests"
        self.sessions = Path(self.tmp) / "sessions"
        self.sessions.mkdir()
        (self.sessions / "target001.json").write_text(
            json.dumps({"metadata": {"id": "target001", "title": "目标"}, "messages": []}),
            encoding="utf-8",
        )

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _create(self, **overrides):
        args = {
            "requests_dir": str(self.requests),
            "sessions_dir": str(self.sessions),
            "name": "定时询问",
            "prompt": "问一下进展",
            "rrule": "FREQ=HOURLY;INTERVAL=6",
            "target_session": "target001",
        }
        args.update(overrides)
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            return server.create_scheduled_task(**args)
        finally:
            server.RESULT_WAIT_SECONDS = old_wait

    def test_valid_target_spools_with_target_field(self):
        payload, error = self._create()
        self.assertIsNone(error)
        record = json.loads(self._spooled()[0].read_text(encoding="utf-8"))
        self.assertEqual(record["target_session"], "target001")
        self.assertEqual(record["kind"], "create")

    def test_isolated_and_unknown_and_junk_targets_rejected(self):
        # Round-7 minor 4: assert the REJECTION CLASS, not mere presence —
        # the old form stayed green if the charset branch was deleted (the
        # charset/length inputs then failed as "not found" instead).
        isolated = ("sched-run1", "SCHED-run1", "aux-side1", "eval_b1")
        junk = ("../escape", "with space", "a" * 300)
        for bad in isolated:
            payload, error = self._create(target_session=bad)
            self.assertIsNone(payload, bad)
            self.assertIsNotNone(error, bad)
            self.assertIn("isolated or unattended", error, bad)
        for bad in junk:
            payload, error = self._create(target_session=bad)
            self.assertIsNone(payload, bad)
            self.assertIsNotNone(error, bad)
            self.assertIn("invalid target_session", error, bad)
        payload, error = self._create(target_session="no-such-target")
        self.assertIsNone(payload)
        self.assertIsNotNone(error)
        self.assertIn("not found", error)

    def test_blank_target_errors_in_every_arm(self):
        """Round-5 M-E: the explicit-blank fix is pinned — create, update
        (alone and combined), and delete all error, nothing spools, and the
        error text names the field (the round-4 silent-downgrade mutant
        turns this red)."""
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            cases = [
                (dict(kind="create", name="n", prompt="p", rrule="FREQ=DAILY;BYHOUR=8", target_session=""), "create"),
                (dict(kind="update", task_id="t-1", target_session=""), "update alone"),
                (dict(kind="update", task_id="t-1", target_session="", name="n2"), "update combined"),
                (dict(kind="delete", task_id="t-1", target_session=""), "delete"),
            ]
            for kwargs, label in cases:
                payload, error = server.schedule_task_request(
                    requests_dir=str(self.requests),
                    sessions_dir=str(self.sessions), **kwargs)
                self.assertIsNone(payload, label)
                self.assertIsNotNone(error, label)
                # Round-6 nit: delete + blank now hits the delete arm FIRST
                # ("takes no extra fields") instead of the update-worded
                # blank message; create/update keep "cannot be blank".
                expected = (
                    "delete takes no extra fields"
                    if label == "delete" else "cannot be blank"
                )
                self.assertIn(expected, error, label)
                self.assertIn("target_session", error, label)
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertEqual(len(self._spooled()), 0, "no error path may spool")

    def test_uppercase_isolated_target_with_existing_session_rejects_with_reason(self):
        """Round-5 M-E: a case-variant isolated target that EXISTS as a
        session file is rejected by the isolation branch (not the existence
        probe) — the reason names isolation, pinning the case-insensitive
        prefix check the existence probe would otherwise mask."""
        (self.sessions / "SCHED-run1.json").write_text(
            json.dumps({"metadata": {"id": "SCHED-run1", "title": "s"}, "messages": []}),
            encoding="utf-8",
        )
        payload, error = self._create(target_session="SCHED-run1")
        self.assertIsNone(payload)
        self.assertIsNotNone(error)
        self.assertIn("isolated", error.lower(), error)

    def test_update_accepts_target_delete_rejects_it(self):
        self._create()
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.05
        try:
            payload, error = server.schedule_task_request(
                requests_dir=str(self.requests),
                kind="update",
                sessions_dir=str(self.sessions),
                task_id="task-1",
                target_session="target001",
            )
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error)
        # Round-5 M-E: select the UPDATE-kind record (uuid-ordered spool may
        # hold several files; [0] is a coin-flip mutation detector).
        upd = [f for f in self._spooled() if json.loads(f.read_text(encoding="utf-8")).get("kind") == "update"]
        self.assertEqual(len(upd), 1, upd)
        record = json.loads(upd[0].read_text(encoding="utf-8"))
        self.assertEqual(record["target_session"], "target001")

        payload, error = server.schedule_task_request(
            requests_dir=str(self.requests),
            kind="delete",
            sessions_dir=str(self.sessions),
            task_id="task-1",
            target_session="target001",
        )
        self.assertIsNone(payload)
        self.assertIn("no extra fields", error)

    def _spooled(self):
        return sorted(Path(self.requests, "spool").glob("*.json"))


class ListScheduledTasksTests(unittest.TestCase):
    """C5/C6 + prompt non-disclosure: tolerant projection of the store."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="pinvou-list-test-")
        self.automations = Path(self.tmp) / "automations"

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_projects_safe_fields_only(self):
        _write_automation(self.automations, "task-1")
        payload, error = server.list_scheduled_tasks(str(self.automations))
        self.assertIsNone(error)
        self.assertEqual(payload["total"], 1)
        entry = payload["tasks"][0]
        self.assertEqual(entry["id"], "task-1")
        self.assertEqual(entry["name"], "早报任务")
        self.assertEqual(entry["status"], "active")
        self.assertEqual(entry["model"], "deepseek-chat")
        blob = json.dumps(payload, ensure_ascii=False)
        self.assertNotIn("prompt", blob)
        self.assertNotIn("机密的提示词内容", blob)

    def test_missing_store_is_empty_not_error(self):
        payload, error = server.list_scheduled_tasks(os.path.join(self.tmp, "missing"))
        self.assertIsNone(error)
        self.assertEqual(payload, {"tasks": [], "total": 0})

    def test_corrupt_files_are_skipped(self):
        _write_automation(self.automations, "task-1")
        (Path(self.automations, "automations", "broken.json")).write_text(
            "{not json", encoding="utf-8"
        )
        # A torn write: valid JSON prefix, invalid tail.
        (Path(self.automations, "automations", "torn.json")).write_text(
            '{"id": "torn", "name": "x"', encoding="utf-8"
        )
        payload, error = server.list_scheduled_tasks(str(self.automations))
        self.assertIsNone(error)
        self.assertEqual([entry["id"] for entry in payload["tasks"]], ["task-1"])

    def test_limit_is_respected(self):
        for index in range(5):
            _write_automation(self.automations, f"task-{index}")
        payload, _ = server.list_scheduled_tasks(str(self.automations), limit=2)
        self.assertEqual(len(payload["tasks"]), 2)
        self.assertEqual(payload["total"], 5)
        payload, _ = server.list_scheduled_tasks(str(self.automations), limit=999)
        self.assertEqual(len(payload["tasks"]), 5)
        payload, _ = server.list_scheduled_tasks(str(self.automations), limit="junk")
        self.assertEqual(len(payload["tasks"]), 5, "default limit caps at 20, not below total")


class FeatureGateTests(unittest.TestCase):
    """F2: the family's feature gate (union semantics + tolerant pass-through)."""

    TOOL_FEATURES = {
        "mcp_app-automations_create_scheduled_task": ["scheduled-task-automation"],
        "mcp_app-automations_read_scheduled_task": ["scheduled-task-automation"],
        "mcp_app-automations_list_scheduled_tasks": ["scheduled-task-automation"],
        "mcp_app-automations_update_scheduled_task": ["scheduled-task-automation"],
        "mcp_app-automations_delete_scheduled_task": ["scheduled-task-automation"],
    }

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        self.base = self.home / "task-requests"
        self.base.mkdir()
        (self.home / "marketplace").mkdir()

    def write_state(self, disabled):
        (self.home / "marketplace" / "builtin_features.json").write_text(
            json.dumps({"schema_version": 1, "disabled_features": disabled}),
            encoding="utf-8",
        )

    def gate(self, tool="create_scheduled_task"):
        return server.feature_gate_error(tool, str(self.base), self.TOOL_FEATURES)

    def test_disabled_feature_returns_structured_error(self):
        self.write_state(["scheduled-task-automation"])
        payload = self.gate()
        self.assertIsNotNone(payload)
        self.assertFalse(payload["ok"])
        self.assertEqual(payload["code"], "feature_disabled")
        self.assertIn("scheduled-task-automation", payload["error"])
        self.assertTrue(payload["alternative"])

    def test_enabled_feature_allows_call(self):
        self.write_state([])
        self.assertIsNone(self.gate())
        self.assertIsNone(self.gate("list_scheduled_tasks"))

    def test_missing_or_corrupt_state_allows_call(self):
        self.assertIsNone(self.gate())
        (self.home / "marketplace" / "builtin_features.json").write_text(
            "{not json", encoding="utf-8"
        )
        self.assertIsNone(self.gate())

    def test_unregistered_tool_is_not_gated(self):
        self.write_state(["scheduled-task-automation"])
        self.assertIsNone(self.gate("some_other_tool"))

    def test_shared_tool_is_disabled_only_when_all_features_are_off(self):
        mapping = {
            server.full_tool_name("create_scheduled_task"):
                ["scheduled-task-automation", "other-feature"],
        }
        for disabled in ([], ["scheduled-task-automation"], ["other-feature"],
                         ["scheduled-task-automation", "other-feature"]):
            with self.subTest(disabled=disabled):
                self.write_state(disabled)
                payload = server.feature_gate_error(
                    "create_scheduled_task", str(self.base), mapping)
                if len(disabled) == 2:
                    self.assertEqual(payload["code"], "feature_disabled")
                    self.assertIn("at least one", payload["alternative"])
                else:
                    self.assertIsNone(payload,
                                      "one remaining feature must keep the shared tool available")

    def test_manifest_declares_the_family_feature(self):
        features = server.load_tool_features(str(SERVER_PATH.parent / "manifest.json"))
        self.assertEqual(
            features,
            self.TOOL_FEATURES,
            "manifest tool_features must stay in sync with the registry",
        )

    def test_full_tool_name(self):
        self.assertEqual(
            server.full_tool_name("create_scheduled_task"),
            "mcp_app-automations_create_scheduled_task",
        )


class DirectoryResolutionTests(unittest.TestCase):
    def test_cli_args_win(self):
        self.assertEqual(
            server.resolve_requests_dir(["--task-requests-dir", "/tmp/x"]), "/tmp/x"
        )
        self.assertEqual(
            server.resolve_automations_dir(["--automations-dir", "/tmp/y"]), "/tmp/y"
        )

    def test_env_fallback(self):
        old = os.environ.get("PINVOU3_HOME")
        os.environ["PINVOU3_HOME"] = "/tmp/pinvou3home"
        try:
            self.assertEqual(
                server.resolve_requests_dir([]),
                os.path.join("/tmp/pinvou3home", "task-requests"),
            )
            self.assertEqual(
                server.resolve_automations_dir([]),
                os.path.join("/tmp/pinvou3home", "automations"),
            )
        finally:
            if old is None:
                os.environ.pop("PINVOU3_HOME", None)
            else:
                os.environ["PINVOU3_HOME"] = old


class StdioContractTests(unittest.TestCase):
    """D6: spawns the real stdio server and verifies protocol robustness."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="pinvou-stdio-test-")
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)

    def _spawn(self, extra_args=()):
        env = dict(os.environ)
        env["PINVOU3_HOME"] = self.tmp
        return subprocess.Popen(
            [sys.executable, str(SERVER_PATH), *extra_args],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            env=env,
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

    def test_stdio_disabled_family_uses_the_request_root(self):
        Path(self.tmp, "marketplace").mkdir()
        Path(self.tmp, "marketplace", "builtin_features.json").write_text(
            json.dumps({"disabled_features": ["scheduled-task-automation"]}), encoding="utf-8")
        proc = self._spawn(["--automations-dir", str(Path(self.tmp, "automations", "automations"))])
        try:
            response = self._rpc(proc, "tools/call", {"name": "create_scheduled_task", "arguments": {}})
            self.assertTrue(response["result"]["isError"])
            self.assertIn("feature_disabled", response["result"]["content"][0]["text"])
            self.assertFalse(Path(self.tmp, "task-requests", "spool").exists())
        finally:
            proc.stdin.close()
            proc.wait(timeout=5)
            proc.stdout.close()
            proc.stderr.close()

    def test_stdio_roundtrip(self):
        proc = self._spawn()
        try:
            init = self._rpc(proc, "initialize", {
                "protocolVersion": server.PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            })
            self.assertEqual(init["result"]["protocolVersion"], server.PROTOCOL_VERSION)
            self.assertEqual(init["result"]["serverInfo"]["name"], "pinvou3-app-automations")

            tools = self._rpc(proc, "tools/list")
            names = [tool["name"] for tool in tools["result"]["tools"]]
            self.assertEqual(names, [
                "create_scheduled_task",
                "read_scheduled_task",
                "list_scheduled_tasks",
                "update_scheduled_task",
                "delete_scheduled_task",
            ])

            call = self._rpc(proc, "tools/call", {
                "name": "list_scheduled_tasks",
                "arguments": {},
            })
            payload = json.loads(call["result"]["content"][0]["text"])
            self.assertTrue(payload["ok"])
            self.assertEqual(payload["tasks"], [])
        finally:
            proc.kill()
            proc.communicate()

    def test_ping_returns_empty_result(self):
        proc = self._spawn()
        try:
            response = self._rpc(proc, "ping")
            self.assertEqual(response["result"], {})
        finally:
            proc.kill()
            proc.communicate()

    def test_unknown_tool_reports_invalid_params(self):
        proc = self._spawn()
        try:
            response = self._rpc(proc, "tools/call", {"name": "no_such_tool", "arguments": {}})
            self.assertEqual(response["error"]["code"], -32602)
        finally:
            proc.kill()
            proc.communicate()

    def test_unknown_method_reports_method_not_found(self):
        proc = self._spawn()
        try:
            response = self._rpc(proc, "no/such/method")
            self.assertEqual(response["error"]["code"], -32601)
        finally:
            proc.kill()
            proc.communicate()

    def test_invalid_create_arguments_report_explicit_errors(self):
        proc = self._spawn()
        try:
            response = self._rpc(proc, "tools/call", {
                "name": "create_scheduled_task",
                "arguments": {"name": "x", "prompt": "y", "rrule": "FREQ=CRON;EXPR=* * * * *"},
            })
            self.assertTrue(response["result"]["isError"])
            payload = json.loads(response["result"]["content"][0]["text"])
            self.assertFalse(payload["ok"])
            self.assertIn("CRON", payload["error"])
            self.assertNotIn(self.tmp, response["result"]["content"][0]["text"])
        finally:
            proc.kill()
            proc.communicate()

    def test_stdio_create_with_task_id_is_rejected_through_the_binding(self):
        """Round-26 M6(b): the create task_id rejection was pinned only
        BELOW the stdio binding — the round-24 regression (dropping
        `task_id=args.get("task_id")` from the `_handle_call` create arm)
        left every suite green while a caller-supplied task_id was
        silently ignored on the wire. Drive the real stdio server so the
        binding itself is covered."""
        proc = self._spawn()
        try:
            response = self._rpc(proc, "tools/call", {
                "name": "create_scheduled_task",
                "arguments": {
                    "name": "x", "prompt": "y",
                    "rrule": "FREQ=HOURLY;INTERVAL=1",
                    "task_id": "task-123",
                },
            })
            self.assertTrue(response["result"]["isError"])
            payload = json.loads(response["result"]["content"][0]["text"])
            self.assertFalse(payload["ok"])
            self.assertIn("invalid task_id", payload["error"])
        finally:
            proc.kill()
            proc.communicate()

    def test_bad_lines_do_not_kill_server(self):
        proc = self._spawn()
        try:
            proc.stdin.buffer.write(b"\xff\xfe not json\n")
            proc.stdin.buffer.write(b'{"jsonrpc":"2.0","id":1,"method":"tools/nope"}\n')
            proc.stdin.buffer.flush()
            response = json.loads(proc.stdout.readline())
            self.assertEqual(response["error"]["code"], -32601)
            response = self._rpc(proc, "ping")
            self.assertEqual(response["result"], {})
        finally:
            proc.kill()
            proc.communicate()

    def test_catch_all_never_leaks_exception_text(self):
        import io
        import unittest.mock as mock

        secret_path = os.path.join(self.tmp, "secret-user-path")

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

    def test_deeply_nested_json_on_stdin_does_not_kill_server(self):
        proc = self._spawn()
        try:
            proc.stdin.buffer.write(
                ("[" * 50000 + "]" * 50000 + "\n").encode("utf-8"))
            proc.stdin.buffer.flush()
            response = self._rpc(proc, "ping")
            self.assertEqual(response["result"], {})
        finally:
            proc.kill()
            proc.communicate()

    def test_result_marker_read_error_keeps_polling_not_crash(self):
        # A hostile/unreadable marker must not crash the server; the wait
        # degrades to pending. The marker is pre-corrupted for this exact
        # request's spool id so the poll loop actually reads it.
        import hashlib

        requests = Path(self.tmp, "task-requests")
        spool_id = hashlib.sha256(b"reqsrc01|create||hostile-k").hexdigest()
        marker_dir = Path(requests, "spool", ".done")
        marker_dir.mkdir(parents=True, exist_ok=True)
        (marker_dir / ("%s.json" % spool_id)).write_text("{not json", encoding="utf-8")

        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.4
        try:
            payload, error = server.create_scheduled_task(
                requests_dir=str(requests),
                name="早报",
                prompt="汇总新闻",
                rrule="FREQ=HOURLY;INTERVAL=6",
                from_session="reqsrc01",
                idempotency_key="hostile-k",
            )
        finally:
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error)
        self.assertEqual(payload["delivery"], "pending")


class ReviewFollowupInputTests(unittest.TestCase):
    def test_lone_surrogate_key_is_a_validation_error(self):
        with tempfile.TemporaryDirectory() as home:
            result, error = server.create_scheduled_task(
                home, name="test", prompt="test", rrule="FREQ=HOURLY;INTERVAL=1",
                from_session="reqsrc01", idempotency_key="bad-\ud800")
            self.assertIsNone(result)
            self.assertIn("invalid idempotency_key", error)
            self.assertFalse(Path(home, "spool").exists())

    def test_non_object_arguments_report_invalid_params(self):
        from unittest.mock import patch
        for args in ([], [1], "text", 0, False, None):
            with self.subTest(args=args), patch.object(server, "_error") as error:
                server._handle_call(1, {"name": "list_scheduled_tasks", "arguments": args},
                                    "/unused", "/unused", {})
                self.assertEqual(error.call_args.args[1], -32602)


class Round33BoundaryTests(unittest.TestCase):
    def test_dispatcher_feature_gate_blocks_every_tool(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as tmp:
            requests = str(Path(tmp, "task-requests"))
            automations = str(Path(tmp, "automations", "automations"))
            Path(tmp, "marketplace").mkdir()
            Path(tmp, "marketplace", "builtin_features.json").write_text(
                json.dumps({"disabled_features": ["scheduled-task-automation"]}), encoding="utf-8")
            for definition in server.TOOL_DEFS:
                replies = []
                with patch.object(server, "_send", side_effect=replies.append):
                    server._handle_call(1, {"name": definition["name"], "arguments": {}},
                                        requests, automations, server.load_tool_features())
                self.assertTrue(replies[0]["result"]["isError"])
                self.assertIn("feature_disabled", replies[0]["result"]["content"][0]["text"])
            self.assertFalse(Path(requests, "spool").exists())

    def test_non_string_and_surrogate_payloads_fail_before_io(self):
        with tempfile.TemporaryDirectory() as tmp:
            for field in ("name", "prompt", "rrule", "model_id", "from_session", "idempotency_key"):
                for bad in ({"a": 1}, "\ud800"):
                    args = {"name": "n", "prompt": "p", "rrule": "FREQ=HOURLY", field: bad}
                    payload, error = server.schedule_task_request(tmp, "create", **args)
                    self.assertIsNone(payload)
                    self.assertIn("invalid " + field, error)
                    self.assertFalse(Path(tmp, "spool").exists())

    def test_unknown_identifiers_are_bounded_protocol_errors(self):
        from unittest.mock import patch
        for value in ("x" * 200000, "\ud800", {"bad": "type"}):
            replies = []
            with patch.object(server, "_send", side_effect=replies.append):
                server._handle({"id": 1, "method": value}, "unused", "unused", {})
                server._handle_call(2, {"name": value}, "unused", "unused", {})
            self.assertEqual([r["error"]["code"] for r in replies], [-32601, -32602])
            self.assertLess(len(json.dumps(replies)), 500)

    def test_permission_errors_do_not_look_like_an_empty_store(self):
        from unittest.mock import patch
        with patch.object(server.os, "listdir", side_effect=PermissionError("fixture")):
            payload, error = server.list_scheduled_tasks("unused")
        self.assertIsNone(payload)
        self.assertIn("not readable", error)
        with patch.object(server, "_open_gated_regular", side_effect=PermissionError("fixture")):
            payload, error = server.read_scheduled_task("unused", "task-1")
        self.assertIsNone(payload)
        self.assertIn("not readable", error)


if __name__ == "__main__":
    unittest.main()
