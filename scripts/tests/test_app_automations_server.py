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

    def test_length_caps_boundary(self):
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
        for bad in ("../escape", "with space", "a" * 300, "sched-run1", "eval_b1", "aux-x1"):
            payload, error = self.create(from_session=bad)
            self.assertIsNone(payload, bad)
            self.assertIsNotNone(error, bad)

    def test_normalization_lands_in_spool(self):
        payload, error = self.create(
            rrule=" freq=hourly;interval=6;byhour=8;byminute=30 ",
            model_id="  model-7  ",
        )
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

    def _finish_in_background(self, task_id="task-1", ok=True, delay=0.3):
        """Simulates the Rust watcher: drain the spool, write the marker, remove the file."""

        def worker():
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                files = self._spooled()
                if files:
                    spool_id = files[0].stem
                    payload = (
                        {"ok": True, "task_id": task_id, "task_name": "早报"}
                        if ok
                        else {"ok": False, "error": "simulated watcher failure"}
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

    def test_digest_mismatch_poll_path_sleeps(self):
        """Round-12 MAJOR-5: the keyed poll's digest-mismatch arm sleeps
        between reads at the poll cadence — the bare `continue` used to
        busy-spin the marker re-read for the whole wait window. Counting
        time.sleep during the mismatch window pins the cadence; deleting
        the sleep turns this red (count 0)."""
        import hashlib

        digest = server.spool_request_digest(
            "create", name="旧名字", prompt="汇总新闻",
            rrule="FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30",
        )
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
        sleep_calls = []
        real_sleep = server.time.sleep

        def counting_sleep(seconds):
            sleep_calls.append(seconds)

        server.time.sleep = counting_sleep
        old_wait = server.RESULT_WAIT_SECONDS
        server.RESULT_WAIT_SECONDS = 0.4
        try:
            payload, error = server.create_scheduled_task(
                **self._create_kwargs(
                    name="新名字", from_session="reqsrc01", idempotency_key="k-spin"
                )
            )
        finally:
            server.time.sleep = real_sleep
            server.RESULT_WAIT_SECONDS = old_wait
        self.assertIsNone(error)
        self.assertEqual(payload.get("delivery"), "pending", payload)
        self.assertGreaterEqual(
            len(sleep_calls), 1,
            "the mismatch arm must sleep between marker reads (no busy-spin)"
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
            "ee13de67c7837ccbb71afbf4c376e6f58bf80509bf7e79744c60c9ed6cd639fe",
        )
        self.assertEqual(
            server.spool_request_digest(
                "update", model_id="m1", paused=True, task_id="t-9",
            ),
            "3e5a7833ac3bd752cf7cc5dd72b0ff4347a65c4155bbe2b69c0fb61673d9bf53",
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

    def test_read_corrupt_store_entry(self):
        (Path(self.automations, "automations")).mkdir(parents=True)
        (Path(self.automations, "automations", "broken.json")).write_text(
            "{not json", encoding="utf-8"
        )
        payload, error = server.read_scheduled_task(str(self.automations), "broken")
        self.assertIsNone(payload)
        self.assertIn("not found", error)


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

    def test_delete_rejects_extra_fields_and_unknown_target(self):
        payload, error = self._schedule("delete")
        self.assertIsNone(error)
        self.assertEqual(payload["kind"], "delete")
        payload, error = self._schedule("delete", name="不应有名字")
        self.assertIsNone(payload)
        self.assertIn("no extra fields", error)
        payload, error = self._schedule("delete", task_id="nosuch")
        self.assertIsNone(payload)
        self.assertIn("not found", error)

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
        stems = [path.stem for path in self._spooled()]
        self.assertIn(expected, stems)

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

    def _spawn(self):
        env = dict(os.environ)
        env["PINVOU3_HOME"] = self.tmp
        return subprocess.Popen(
            [sys.executable, str(SERVER_PATH)],
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


if __name__ == "__main__":
    unittest.main()
