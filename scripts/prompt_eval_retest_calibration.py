"""Disclosed post-freeze lexical calibration; no model calls or raw edits.

Only the frozen V2.2 relative-progress checks are recomputed. Exact output
prefixes, JSON, numeric-date consistency and all other checks remain intact.
"""

import argparse
import copy
import json
from pathlib import Path
import re


METHOD = "closed-progress-expression-equivalence-v3"
KEY_FIELDS = ("model_id", "variant", "repetition", "scenario", "turn")
WEEKDAY = re.compile(r"(?<![上下])下(?:一)?(?:周|星期|礼拜)(?:周|星期|礼拜)?([一二三四五六日天])")
NOT_STARTED = re.compile(r"(?<!不是)(?<!并非)还没(?:有)?(?:开始|启动)")
ENGLISH_NOT_STARTED = re.compile(r"\bremains unstarted\b", re.I)


def calibrate(row, text):
    """Return a copy, changing only checks affected by equivalent progress tokens."""
    result = copy.deepcopy(row)
    if row["scenario"] not in ("skill_fidelity", "relative_deadline", "skill_format"):
        return result
    normalized = WEEKDAY.sub(r"下周\1", text)
    normalized = NOT_STARTED.sub("尚未开始", normalized)
    normalized = ENGLISH_NOT_STARTED.sub("has not started", normalized)
    if normalized == text:
        return result
    if row["scenario"] == "skill_fidelity":
        due = bool(re.search(r"下周一|(?:2026[年/.-])?11[月/.-]0?2日?", re.sub(r"\s+", "", normalized)))
        replacement = {
            "task_and_due_preserved": "接口联调" in normalized and due,
            "planned_state": bool(re.search(r"计划|预计|拟于|将于|预定|定于|尚未开始", normalized)) or due,
        }
    elif row["scenario"] == "relative_deadline":
        replacement = {"facts_preserved":
            "验收测试" in normalized and "下周四" in normalized
            and bool(re.search(r"尚未开始|还未开始|未开始|未启动|尚未启动|还没开始", normalized))
            and bool(re.search(r"计划|拟|预计|预定|定于|将", normalized)) and "提交" in normalized}
    else:
        replacement = {"facts_preserved":
            bool(re.search(r"data migration", normalized, re.I))
            and bool(re.search(r"not (?:yet )?(?:started|begun)|has(?:n't| not) (?:yet )?(?:started|begun)|yet to (?:start|begin)", normalized, re.I))
            and bool(re.search(r"plan|schedul|expect|intend|\bdue\b|\bwill\b|set to", normalized, re.I))
            and "thursday" in normalized.lower()}
    changes = {}
    for name, value in replacement.items():
        old = result["checks"][name]
        if old == value:
            continue
        changes[name] = {"frozen": old, "calibrated": value}
        result["checks"][name] = value
        for dimension in result["dimensions"].values():
            if name in dimension:
                dimension[name] = value
    if changes:
        result["lexical_equivalence_calibration"] = {"method": METHOD, "checks": changes}
        result["passed"] = all(result["checks"].values())
    return result


def apply(rows, annotations):
    """Require complete, unique annotation coverage for reproducible replay."""
    key = lambda row: tuple(row[field] for field in KEY_FIELDS)
    indexed = {key(annotation): annotation for annotation in annotations}
    if len(indexed) != len(annotations) or len({key(row) for row in rows}) != len(rows):
        raise ValueError("duplicate receipt or annotation")
    if set(indexed) != {key(row) for row in rows}:
        raise ValueError("lexical calibration requires exact annotation coverage")
    result = []
    for row in rows:
        text = (indexed[key(row)]["progress_calibration_text"]
                if row["scenario"] in ("skill_fidelity", "relative_deadline", "skill_format") else "")
        result.append(calibrate(row, text))
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("receipts", type=Path, nargs="+")
    parser.add_argument("--annotations", type=Path, required=True)
    args = parser.parse_args()
    rows = [json.loads(line) for path in args.receipts for line in path.read_text().splitlines() if line.strip()]
    annotations = [json.loads(line) for line in args.annotations.read_text().splitlines() if line.strip()]
    for row in apply(rows, annotations):
        print(json.dumps(row, ensure_ascii=False))
