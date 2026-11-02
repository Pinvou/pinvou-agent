"""Source identity and receipt compatibility for current evaluation tools."""

import hashlib
import json
from pathlib import Path
from types import ModuleType


def case_source_files(module):
    """Follow case generations and lateral prompt_eval_* grader imports."""
    pending, seen, paths = [module], set(), set()
    while pending:
        current = pending.pop()
        if id(current) in seen:
            continue
        seen.add(id(current))
        paths.add(Path(current.__file__).resolve())
        pending.extend(value for value in vars(current).values()
                       if isinstance(value, ModuleType)
                       and Path(getattr(value, "__file__", "") or "").stem.startswith("prompt_eval_"))
    return sorted(paths)


def source_identity(paths):
    sources = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
               for path in sorted(set(paths))}
    revision = hashlib.sha256(json.dumps(sources, sort_keys=True).encode()).hexdigest()
    return sources, revision


def validate_revisions(rows, expected=None):
    """Allow wholly legacy corpora; never mix versions or resume stale rows."""
    values = [row.get("grader_revision") for row in rows]
    if any(value is not None and (not isinstance(value, str) or not value.strip())
           for value in values):
        raise ValueError("invalid grader_revision")
    revisions = set(values)
    if len(revisions) > 1:
        raise ValueError("mixed grader_revision receipts")
    if rows and expected is not None and revisions != {expected}:
        raise ValueError("resume requires the same grader_revision on every receipt")
    return next(iter(revisions), None)
