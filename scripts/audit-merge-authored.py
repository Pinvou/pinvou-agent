#!/usr/bin/env python3
"""Report identifiers authored inside merge resolutions.

For every merge commit M reachable from HEAD but not from the base ref
(default origin/main), an identifier (>= 6 chars) the merge tree adds
relative to BOTH parents is resolution-authored: it never appears in any
reviewable non-merge diff, so per-commit review threads cannot see it
(review #455 round-23 MAJOR 1b — the file-list and deleted-symbol greps
missed exactly this class; the de04d54a9 consent-sync reorder motivated
the check). Per-file token sets keep the output attributable; it is a
review INPUT, not a verdict — comment-only identifiers will show up too.

Known limitation (round-26 minor 13, review #455): this check is
additions-only — a deletion-shaped merge resolution (code the resolution
removes relative to both parents) is structurally invisible to it; the
review's file-list and deleted-symbol greps carry that load. A clean
output here must not be read as "no resolution-authored content", only
as "no resolution-ADDED identifiers".

Usage: python3 scripts/audit-merge-authored.py [base-ref]
"""
import re
import subprocess
import sys

BASE = sys.argv[1] if len(sys.argv) > 1 else "origin/main"
TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_]{5,}")


def git(*args: str) -> str:
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def added_tokens(tree_a: str, tree_b: str) -> dict[str, set[str]]:
    result: dict[str, set[str]] = {}
    for path in git("diff", "--name-only", tree_a, tree_b).splitlines():
        toks: set[str] = set()
        for line in git("diff", "--unified=0", tree_a, tree_b, "--", path).splitlines():
            if line.startswith("+") and not line.startswith("+++"):
                toks.update(TOKEN.findall(line))
        result[path] = toks
    return result


def main() -> None:
    merges = git("rev-list", "--merges", f"{BASE}..HEAD").split()
    if not merges:
        print(f"no merge commits under {BASE}..HEAD")
        return
    findings = 0
    for m in merges:
        p1 = git("rev-parse", f"{m}^1").strip()
        p2 = git("rev-parse", f"{m}^2").strip()
        vs_branch = added_tokens(p1, m)
        vs_main = added_tokens(p2, m)
        for path in sorted(set(vs_branch) & set(vs_main)):
            authored = vs_branch[path] & vs_main[path]
            if authored:
                findings += len(authored)
                print(f"{m[:9]} {path}: {', '.join(sorted(authored))}")
    if not findings:
        print(f"no merge-authored identifiers under {BASE}..HEAD")


if __name__ == "__main__":
    main()
