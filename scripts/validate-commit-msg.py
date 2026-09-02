#!/usr/bin/env python3
"""Validate commit messages against the pinvou3 mandatory convention."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


ALLOWED_TYPES = (
    "feat",
    "fix",
    "refactor",
    "perf",
    "docs",
    "style",
    "test",
    "build",
    "ci",
    "chore",
    "revert",
)

SUBJECT_RE = re.compile(
    r"^(?P<type>{types})(?:\((?P<scope>[^()\s:]+)\))?(?P<breaking>!)?: (?P<desc>.+)$".format(
        types="|".join(ALLOWED_TYPES)
    )
)

GITHUB_PR_SUFFIX_RE = re.compile(r" \(#\d+\)$")
PROHIBITED_DESCRIPTIONS = {
    "update",
    "fix bug",
    "modify code",
    "test",
    "修改代码",
    "测试",
}
FORBIDDEN_ENDING_PUNCTUATION = set("。.，,；;、！!？?：:")
DOC_PATH = "docs/commit-message-convention.md"
# #235 introduced this gate on main. Older Windows branch history remains
# grandfathered so generated merge commits and pre-gate commits do not fail main.
LEGACY_HISTORY_CUTOFF = "deae3ca0141390c06e14aa93610645088b8966d4"
TRUSTED_BOT_AUTHORS = {
    (
        "dependabot[bot]",
        "49699333+dependabot[bot]@users.noreply.github.com",
    ),
    (
        "github-actions[bot]",
        "41898282+github-actions[bot]@users.noreply.github.com",
    ),
}


def clean_commit_message(raw: str) -> list[str]:
    return [line.rstrip() for line in raw.splitlines() if not line.startswith("#")]


def first_subject(raw: str) -> str:
    for line in clean_commit_message(raw):
        if line.strip():
            return line.lstrip("\ufeff")
    return ""


def validate_text(raw: str, label: str) -> list[str]:
    subject = first_subject(raw)
    errors: list[str] = []

    if not subject:
        return [f"{label}: commit message is empty"]

    match = SUBJECT_RE.match(subject)
    if not match:
        return [
            f"{label}: first line must be '<type>(<scope>): <description>'",
            f"  allowed types: {', '.join(ALLOWED_TYPES)}",
            f"  example: fix(installer): preserve runtime resources",
            f"  actual: {subject}",
        ]

    desc = match.group("desc")
    semantic_desc = GITHUB_PR_SUFFIX_RE.sub("", desc)
    normalized_desc = semantic_desc.strip().lower()

    if semantic_desc != semantic_desc.strip():
        errors.append(f"{label}: description must not start or end with whitespace")

    if len(semantic_desc) > 50:
        errors.append(
            f"{label}: description must be 50 characters or fewer, got {len(semantic_desc)}"
        )

    if not any(char.isalpha() for char in semantic_desc):
        errors.append(f"{label}: description must contain a letter")

    if semantic_desc and semantic_desc[-1] in FORBIDDEN_ENDING_PUNCTUATION:
        errors.append(f"{label}: description must not end with punctuation")

    if normalized_desc in PROHIBITED_DESCRIPTIONS:
        errors.append(f"{label}: description is too vague: {semantic_desc}")

    return errors


def git(*args: str) -> str:
    return subprocess.check_output(("git", *args), text=True, encoding="utf-8").strip()


def git_commit_exists(revision: str) -> bool:
    return (
        subprocess.run(
            ("git", "cat-file", "-e", f"{revision}^{{commit}}"),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        ).returncode
        == 0
    )


def validate_file(path: Path) -> list[str]:
    return validate_text(path.read_text(encoding="utf-8"), str(path))


def validate_range(base: str, head: str) -> list[str]:
    revision = f"{base}..{head}" if git_commit_exists(base) else head
    log_args = [
        "log",
        "--no-merges",
        "--format=%H",
        revision,
    ]
    if git_commit_exists(LEGACY_HISTORY_CUTOFF):
        log_args.extend(("--not", LEGACY_HISTORY_CUTOFF))

    commit_list = git(*log_args)
    commits = [line for line in commit_list.splitlines() if line]
    errors: list[str] = []

    for commit in commits:
        entry = git("log", "-1", "--format=%an%x00%ae%x00%B", commit)
        author_name, author_email, raw = entry.split("\x00", 2)
        if (author_name, author_email) in TRUSTED_BOT_AUTHORS:
            continue
        label = f"{commit[:12]} {first_subject(raw)}"
        errors.extend(validate_text(raw, label))

    return errors


def main() -> int:
    parser = argparse.ArgumentParser(
        description=f"Validate commit messages. See {DOC_PATH}."
    )
    parser.add_argument("message_file", nargs="?", type=Path)
    parser.add_argument("--range", nargs=2, metavar=("BASE", "HEAD"))
    args = parser.parse_args()

    if bool(args.message_file) == bool(args.range):
        parser.error("provide either a commit message file or --range BASE HEAD")

    errors = validate_range(*args.range) if args.range else validate_file(args.message_file)

    if errors:
        print("Commit message does not follow the mandatory Pinvou Agent convention.", file=sys.stderr)
        print(f"See {DOC_PATH}", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
