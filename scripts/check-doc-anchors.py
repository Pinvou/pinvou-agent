#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Check that "a symbol of some file" references in docs actually resolve.

**Why this script exists.** Docs used to carry `lifecycle.rs:760`-style
"file:line" references. Line numbers drift, and nothing errors when they do —
a reader who follows one lands on an unrelated line and assumes they misread.
A review spot-check of 12 references found ~10 drifted by +40~+134 lines, one
past the end of the file.

So references should be written as "file + symbol name": symbols move with the
code, and a rename is caught by this script immediately, while moving a symbol
within the file is no longer a problem. Line references are treated as
regressions — written back, they will drift.

**Recognized anchor spellings.** Only file-then-symbol forms are checked:
`` `foo.rs` 的 `bar` ``, `` `foo.rs` 的 `bar()` ``, and the same without
backticks. The English prose order "bar() of foo.rs" is NOT an anchor for this
checker — only the file-first form resolves a file to check against. A bare
"foo.rs bar" (no particle, no call parentheses) is ordinary prose, not an
anchor, and is skipped by check().

**Extension coverage.** Pairs and line references recognize
`.rs .js .jsx .mjs .json .ts .tsx .sh .py` (`.json` is in the alternation after
`.js` so the trailing `\b` makes it a distinct match, never a half-match of
`.js`). Extensions outside that set are not anchors.

**Default scope.** With no arguments the DEFAULT_DOCS list is checked: the two
recovery docs (no anchors yet, kept as the home of the anchor style) plus
anchor-carrying docs that pass today, so the default run actually verifies
references instead of trivially passing. Line-ref-style and time-point docs
(tool-governance, the system-prompt history sections, …) are intentionally out
of scope — adding them requires rewriting their references to the anchor style
first.

Usage: python scripts/check-doc-anchors.py [doc-path...]
Paths are repo-root-relative. A non-zero exit code means some reference failed
to resolve.
"""
import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_DOCS = [
    "docs/chat-steer-recovery.md",
    "docs/workspace-rebind-recovery.md",
    "docs/capability-governance.md",
    "docs/记忆功能-架构设计.md",
    "docs/底座升级验收清单.md",
]
SKIP_DIRS = {".git", "node_modules", "target", "dist", ".venv", "__pycache__"}

# `bar()` of `foo.rs` / bar() of foo.rs — only the file-then-symbol form is an
# anchor (see the docstring); the bare "foo.rs bar" prose form is skipped by
# check(). The trailing \b keeps `.json` from matching as `.js`.
PAIR = re.compile(
    r"`?([A-Za-z_][A-Za-z0-9_./-]*\.(?:rs|js|jsx|mjs|json|ts|tsx|sh|py))\b`?\s*(的?)\s*`?"
    r"([A-Za-z_][A-Za-z0-9_:]*)\s*(\(\))?`?"
)
# Any line-number reference is treated as a regression and reported.
LINEREF = re.compile(
    r"[A-Za-z_][A-Za-z0-9_./-]*\.(?:rs|js|jsx|mjs|json|ts|tsx|sh|py):\d+"
)

_resolved = {}


def resolve(rel):
    """Resolve a short path written in a doc to a real path in the repo."""
    if rel in _resolved:
        return _resolved[rel]
    got = rel if os.path.isfile(os.path.join(ROOT, rel)) else None
    if got is None:
        name = rel.split("/")[-1]
        hits = []
        for dp, dn, fn in os.walk(ROOT):
            dn[:] = [d for d in dn if d not in SKIP_DIRS]
            if name in fn:
                p = os.path.relpath(os.path.join(dp, name), ROOT).replace("\\", "/")
                if rel in p or p.endswith("/" + rel):
                    hits.append(p)
        hits.sort(key=lambda p: (len(p), p))
        got = hits[0] if hits else None
    _resolved[rel] = got
    return got


_src = {}


def body(path):
    if path not in _src:
        with open(os.path.join(ROOT, path), encoding="utf-8", errors="replace") as f:
            _src[path] = f.read()
    return _src[path]


def check(doc):
    bad = []
    with open(os.path.join(ROOT, doc), encoding="utf-8", errors="replace") as f:
        lines = f.read().splitlines()
    for i, line in enumerate(lines, 1):
        for m in LINEREF.finditer(line):
            bad.append(
                f"{doc}:{i} line-number reference {m.group(0)} — line numbers "
                f"drift; use \"file + symbol name\" instead"
            )
        for m in PAIR.finditer(line):
            rel, sym = m.group(1), m.group(3)
            if not m.group(2) and not m.group(4):
                # No particle and no call parentheses: ordinary prose.
                continue
            real = resolve(rel)
            if real is None:
                bad.append(f"{doc}:{i} referenced file does not exist: {rel}")
                continue
            if sym not in body(real):
                bad.append(
                    f"{doc}:{i} `{sym}` not found in {real} (renamed or removed?)"
                )
    return bad


def main():
    parser = argparse.ArgumentParser(
        description="Check that doc anchors (file + symbol references) "
        "resolve in this repository."
    )
    parser.add_argument(
        "docs",
        nargs="*",
        help="doc paths to check, repo-root-relative "
        "(default: the DEFAULT_DOCS list in this script)",
    )
    args = parser.parse_args()
    docs = args.docs or DEFAULT_DOCS
    bad = []
    for d in docs:
        if not os.path.isfile(os.path.join(ROOT, d)):
            bad.append(f"doc does not exist: {d}")
            continue
        bad += check(d)
    if bad:
        print("doc anchor check failed:")
        for b in bad:
            print("  -", b)
        return 1
    print(f"doc anchor check passed ({len(docs)} docs)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
