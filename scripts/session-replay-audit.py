#!/usr/bin/env python3
"""Audit real pinvou3 session JSON files as replay fixtures.

This is a deterministic companion to the real-model L1 harness: it does not
call a model. It checks that saved transcripts are structurally replayable and
summarizes user-visible interaction signals such as request_user_input and
artifact-producing tools.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


DEFAULT_SESSIONS_DIR = Path.home() / ".pinvou3" / "sessions"
ARTIFACT_TOOLS = {"write_file", "edit_file"}
DECISION_TOOL = "request_user_input"


def empty_summary(path: Path) -> dict[str, Any]:
    """Return a render-safe summary even when the session cannot be decoded."""
    return {
        "path": str(path),
        "session_id": None,
        "title": None,
        "messages": 0,
        "text_chars": 0,
        "tool_uses": 0,
        "tool_results": 0,
        "request_user_input": 0,
        "request_user_input_answered": 0,
        "artifact_refs": 0,
        "warnings": 0,
        "errors": 0,
    }


def iter_blocks(message: dict[str, Any]):
    content = message.get("content")
    if isinstance(content, list):
        for block in content:
            if isinstance(block, dict):
                yield block
    elif isinstance(content, str):
        yield {"type": "text", "text": content}


def block_text(block: dict[str, Any]) -> str:
    value = block.get("text", "")
    return value if isinstance(value, str) else ""


def is_artifact_tool(name: Any) -> bool:
    return isinstance(name, str) and (
        name in ARTIFACT_TOOLS or name.endswith("present_artifact")
    )


def load_latest(limit: int, sessions_dir: Path) -> list[Path]:
    if not sessions_dir.is_dir():
        return []
    files = [
        p
        for p in sessions_dir.glob("*.json")
        if p.is_file()
        and not p.name.startswith("_")
        and ".pre-restore-" not in p.name
    ]
    files.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    return files[:limit]


def audit_session(path: Path, strict_fs: bool) -> tuple[dict[str, Any], list[str], list[str]]:
    warnings: list[str] = []
    errors: list[str] = []

    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:
        summary = empty_summary(path)
        summary["errors"] = 1
        return summary, [], [f"cannot read JSON: {exc}"]

    metadata = data.get("metadata") if isinstance(data, dict) else None
    messages = data.get("messages") if isinstance(data, dict) else None
    if not isinstance(metadata, dict):
        errors.append("metadata is missing or not an object")
        metadata = {}
    if not isinstance(messages, list):
        errors.append("messages is missing or not an array")
        messages = []

    expected_count = metadata.get("message_count")
    if isinstance(expected_count, int) and expected_count != len(messages):
        warnings.append(f"metadata.message_count={expected_count} but messages={len(messages)}")

    tool_uses: dict[str, str] = {}
    tool_result_ids: list[str] = []
    duplicate_tool_ids: list[str] = []
    unknown_results: list[str] = []
    text_chars = 0
    request_user_input_count = 0
    request_user_input_answered = 0
    artifact_refs: list[str] = []

    for message in messages:
        if not isinstance(message, dict):
            warnings.append("non-object message found")
            continue
        for block in iter_blocks(message):
            kind = block.get("type")
            if kind == "text":
                text_chars += len(block_text(block))
            elif kind == "tool_use":
                tool_id = block.get("id")
                name = block.get("name")
                if not isinstance(tool_id, str) or not tool_id:
                    warnings.append("tool_use without string id")
                    continue
                if tool_id in tool_uses:
                    duplicate_tool_ids.append(tool_id)
                tool_uses[tool_id] = name if isinstance(name, str) else "?"
                if name == DECISION_TOOL:
                    request_user_input_count += 1
                if is_artifact_tool(name):
                    inp = block.get("input")
                    if isinstance(inp, dict):
                        ref = inp.get("path") or inp.get("artifact_path") or inp.get("file")
                        if isinstance(ref, str) and ref:
                            artifact_refs.append(ref)
            elif kind == "tool_result":
                tool_use_id = block.get("tool_use_id")
                if not isinstance(tool_use_id, str) or not tool_use_id:
                    warnings.append("tool_result without string tool_use_id")
                    continue
                tool_result_ids.append(tool_use_id)
                if tool_use_id not in tool_uses:
                    unknown_results.append(tool_use_id)
                if tool_uses.get(tool_use_id) == DECISION_TOOL:
                    request_user_input_answered += 1

    for tool_id in duplicate_tool_ids:
        errors.append(f"duplicate tool_use id: {tool_id}")
    for tool_id in unknown_results:
        errors.append(f"tool_result references unknown tool_use id: {tool_id}")

    open_tool_uses = [tool_id for tool_id in tool_uses if tool_id not in set(tool_result_ids)]
    if open_tool_uses:
        warnings.append(f"{len(open_tool_uses)} tool_use block(s) have no persisted tool_result")

    missing_artifacts: list[str] = []
    for ref in artifact_refs:
        p = Path(ref).expanduser()
        if p.is_absolute() and not p.exists():
            missing_artifacts.append(ref)
    if missing_artifacts:
        msg = f"{len(missing_artifacts)} absolute artifact path(s) do not exist"
        if strict_fs:
            errors.append(msg)
        else:
            warnings.append(msg)

    summary = empty_summary(path)
    summary.update({
        "session_id": metadata.get("id"),
        "title": metadata.get("title"),
        "messages": len(messages),
        "text_chars": text_chars,
        "tool_uses": len(tool_uses),
        "tool_results": len(tool_result_ids),
        "request_user_input": request_user_input_count,
        "request_user_input_answered": request_user_input_answered,
        "artifact_refs": len(artifact_refs),
        "warnings": len(warnings),
        "errors": len(errors),
    })
    return summary, warnings, errors


def main() -> int:
    parser = argparse.ArgumentParser(description="Audit real pinvou3 session JSON transcripts")
    parser.add_argument("sessions", nargs="*", type=Path, help="session JSON files")
    parser.add_argument("--latest", type=int, default=0, help="audit N newest sessions from ~/.pinvou3/sessions")
    parser.add_argument("--sessions-dir", type=Path, default=DEFAULT_SESSIONS_DIR)
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    parser.add_argument("--strict-fs", action="store_true", help="fail when absolute artifact paths are missing")
    args = parser.parse_args()

    paths = list(args.sessions)
    if args.latest:
        paths.extend(load_latest(args.latest, args.sessions_dir))
    paths = list(dict.fromkeys(path.expanduser() for path in paths))
    if not paths:
        parser.error("provide session JSON paths or --latest N")

    reports = []
    total_errors = 0
    for path in paths:
        summary, warnings, errors = audit_session(path, args.strict_fs)
        reports.append({"summary": summary, "warnings": warnings, "errors": errors})
        total_errors += len(errors)

    if args.json:
        print(json.dumps(reports, ensure_ascii=False, indent=2))
    else:
        for report in reports:
            s = report["summary"]
            status = "FAIL" if report["errors"] else "PASS"
            print(
                f"{status} {s.get('session_id') or Path(s['path']).stem}: "
                f"messages={s['messages']} tools={s['tool_uses']}/{s['tool_results']} "
                f"rui={s['request_user_input']}/{s['request_user_input_answered']} "
                f"artifacts={s['artifact_refs']} warnings={s['warnings']}"
            )
            for warning in report["warnings"]:
                print(f"  WARN {warning}")
            for error in report["errors"]:
                print(f"  ERROR {error}")

    return 1 if total_errors else 0


if __name__ == "__main__":
    sys.exit(main())
