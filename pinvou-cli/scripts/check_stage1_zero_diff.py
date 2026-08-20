#!/usr/bin/env python3
"""Keep stage 1 changes out of Desktop, CodeWhale, and release inputs."""

from __future__ import annotations

import argparse
import subprocess
import sys
from dataclasses import dataclass


@dataclass(frozen=True)
class ChangedPath:
    status: str
    paths: tuple[str, ...]


def parse_name_status_z(output: str) -> list[ChangedPath]:
    fields = output.split("\0")
    if fields and fields[-1] == "":
        fields.pop()
    records = []
    index = 0
    while index < len(fields):
        status_field = fields[index]
        index += 1
        if "\t" in status_field:
            status, first_path = status_field.split("\t", 1)
            paths = [first_path]
        else:
            status = status_field
            if index >= len(fields):
                raise ValueError(f"缺少 {status!r} 对应的路径")
            paths = [fields[index]]
            index += 1
        if status.startswith(("R", "C")):
            if index >= len(fields):
                raise ValueError(f"缺少 rename/copy {status!r} 的目标路径")
            paths.append(fields[index])
            index += 1
        records.append(ChangedPath(status, tuple(paths)))
    return records


def _is_allowed(path: str) -> bool:
    normalized = path.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return (
        normalized == "AGENTS.md"
        or normalized == ".github/workflows/pr-check.yml"
        or normalized == "pinvou-cli"
        or normalized.startswith("pinvou-cli/")
    )


def find_violations(changes: list[ChangedPath]) -> list[str]:
    violations = []
    for change in changes:
        for path in change.paths:
            if not _is_allowed(path):
                violations.append(f"{change.status}: {path}")
    return violations


def _git_diff(base: str, head: str) -> str:
    completed = subprocess.run(
        ["git", "diff", "--name-status", "-z", "--find-renames", f"{base}...{head}"],
        check=True,
        capture_output=True,
    )
    return completed.stdout.decode("utf-8", errors="surrogateescape")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-ref", required=True)
    parser.add_argument("--head-ref", default="HEAD")
    args = parser.parse_args(argv)

    try:
        changes = parse_name_status_z(_git_diff(args.base_ref, args.head_ref))
    except (subprocess.CalledProcessError, ValueError) as error:
        print(f"stage 1 zero-diff guard 无法读取 diff: {error}", file=sys.stderr)
        return 2
    violations = find_violations(changes)
    if violations:
        print("stage 1 zero-diff boundary failed:", file=sys.stderr)
        for violation in violations:
            print(f"- {violation}", file=sys.stderr)
        return 1
    print("stage 1 zero-diff boundary passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
