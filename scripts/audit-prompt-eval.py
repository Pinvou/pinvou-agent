#!/usr/bin/env python3
"""Audit frozen live-evaluation receipts without making new model calls.

Tool IDs can recur in the text-tool compatibility path. Bound each turn by
its persisted user message and pair results in sequence instead of filtering
the entire session by an ID set. Keep product-reported counts for comparison.
Only safe measurements are emitted; raw messages and arguments stay private.
"""

import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess
import sys


def turn_blocks(root, report, turn):
    session = root / "home/sessions" / f"{report.get('session_id', '')}.json"
    if not session.exists():
        return []
    prompts = json.loads((root / "case.json").read_text())["prompts"]
    messages = json.loads(session.read_text())["messages"]

    def matches(message, prompt):
        return message.get("role") == "user" and any(
            block.get("type") == "text" and prompt in block.get("text", "")
            for block in message.get("content", []))

    start = next((i for i, message in enumerate(messages) if matches(message, prompts[turn])), None)
    if start is None:
        return []
    end = next((i for i in range(start + 1, len(messages))
                if any(matches(messages[i], prompt) for prompt in prompts[turn + 1:])), len(messages))
    return [block for message in messages[start:end] for block in message.get("content", [])]


def tool_events(blocks):
    events = []
    for block in blocks:
        if block.get("type") == "tool_use":
            pending = [event for event in events
                       if event["id"] == block["id"] and event["failed"] is None]
            # Overlapping duplicate IDs cannot identify which call a result
            # settles. Keep every affected call unsettled for this turn.
            for event in pending:
                event["ambiguous_id"] = True
            events.append({"id": block["id"], "name": block["name"],
                           "input": block.get("input", {}), "failed": None,
                           "ambiguous_id": bool(pending)})
        elif block.get("type") == "tool_result":
            event = next((event for event in reversed(events)
                          if event["id"] == block["tool_use_id"] and event["failed"] is None), None)
            if event is not None:
                if event["ambiguous_id"]:
                    continue
                content = str(block.get("content", ""))
                event["failed"] = bool(block.get("is_error"))
                if event["failed"]:
                    content = content.lower()
                    event["failure_kind"] = next((kind for needle, kind in (
                        ("tool not executed:", "not_executed"),
                        ("plan mode", "plan_read_only"),
                        ("missing required field", "invalid_arguments"),
                        ("not in the allowed-tools list", "host_tool_denied"),
                        ("not found", "not_found"),
                    ) if needle in content), "tool_error")
    return events


def presented(events, path):
    """Require a settled successful call with its own exact path argument."""
    return any(event.get("name") == "mcp_pinvou3_present_artifact"
               and event.get("failed") is False
               and event.get("input", {}).get("path") == str(path)
               for event in events)


def code_oracle(root):
    program = """import math, runpy
f = runpy.run_path('calculator.py')['discount']
assert all(math.isclose(f(p, d), e, abs_tol=1e-9) for p, d, e in
           [(100, 20, 80), (50, 0, 50), (100, 100, 0), (125, 10, 112.5), (0, 50, 0)])
"""
    try:
        return subprocess.run([sys.executable, "-c", program], cwd=root,
                              capture_output=True, timeout=10).returncode == 0
    except subprocess.TimeoutExpired:
        return False


def audit(directory):
    rows = [json.loads(line) for line in (directory / "results.jsonl").read_text().splitlines()]
    manifest = json.loads((directory / "manifest.json").read_text())
    models = manifest["model_ids"]
    for original in rows:
        row = dict(original)
        root = directory / f"m{models.index(row['model_id'])}-r{row['repetition']}-{row['scenario']}"
        report_path = root / f"response-{row['turn']}.json"
        report = json.loads(report_path.read_text()) if report_path.exists() else {}
        blocks = turn_blocks(root, report, row["turn"])
        events = tool_events(blocks)
        names = [event["name"] for event in events]
        success = {event["name"] for event in events if event["failed"] is False}
        row["product_reported_tools"] = {key: row[key] for key in ("tool_calls", "failed_tools", "tool_names")}
        row["automated_passed"] = row["passed"]
        row["tool_calls"] = len(events)
        row["failed_tools"] = sum(event["failed"] is True for event in events)
        row["unsettled_tools"] = sum(event["failed"] is None for event in events)
        row["tool_names"] = names
        row["tool_failure_kinds"] = dict(Counter(event.get("failure_kind", "tool_error")
                                                  for event in events if event["failed"] is True))
        row["usage_partial"] = row.get("usage") is None
        row["model_request_scope"] = "main_session"
        row["usage_scope"] = "completed_main_requests_only" if row["usage_partial"] else "whole_turn_including_children"
        row["observed_usage"] = row.get("usage")
        if row["usage_partial"] and row["model_requests"]:
            row["observed_usage"] = {
                field: sum(request[field] for request in row["model_requests"]
                           if request.get(field) is not None)
                for field in ("input_tokens", "output_tokens", "cache_hit_tokens", "cache_miss_tokens")}
        row["tool_accounting_changed"] = row["product_reported_tools"] != {
            key: row[key] for key in ("tool_calls", "failed_tools", "tool_names")}
        checks = dict(row["checks"])
        checks["turn_trace_available"] = bool(blocks)
        case, turn = row["scenario"], row["turn"]
        if case == "consult":
            checks["no_tools"] = not events
        elif case in ("artifact", "multi_artifact"):
            target = root / ("secondary" if case == "multi_artifact" else "primary") / "report.md"
            checks["presented"] = presented(events, target)
            if case == "artifact" and turn:
                content = target.read_text() if target.exists() else ""
                checks["brief_fact_preserved"] = "接口联调" in content and "完成" in content
        elif case == "plan_execute" and not turn:
            checks["plan_submitted"] = "todo_write" in success
        elif case == "skill" and not turn:
            checks["skill_loaded"] = "load_skill" in success
        if case in ("code_fix", "multi_code") or case == "plan_execute" and turn:
            checks["no_card"] = "mcp_pinvou3_present_artifact" not in names
            target = root / ("secondary" if case == "multi_code" else "primary")
            checks["independent_code_oracle"] = code_oracle(target)
            if case == "multi_code":
                try:
                    checks["primary_notes_preserved"] = (root / "primary/notes.txt").read_text() == "KEEP_EXISTING_NOTES\n"
                except (OSError, UnicodeError):
                    checks["primary_notes_preserved"] = False
        if case == "browser_host_boundary":
            checks["discovery_attempted"] = "tool_search" in names or any(name.startswith("mcp_browser_") for name in names)
            checks["no_registry"] = not any(name in names for name in ("registry_sync", "start_registry_mcp_server"))
        row["checks"] = checks
        row["passed"] = all(checks.values())
        row["audit_version"] = "turn-boundary-v1"
        print(json.dumps(row, ensure_ascii=False))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    audit(parser.parse_args().directory.resolve())
