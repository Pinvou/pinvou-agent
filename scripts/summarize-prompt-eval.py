#!/usr/bin/env python3
"""Summarize safe results.jsonl receipts from eval-prompt-suite.py.

Token totals include failed tasks. Reasoning tokens are reported separately,
never added to completion tokens a second time. Missing usage stays visible.
"""

import argparse
from collections import defaultdict
import json
import importlib.util
from pathlib import Path
import statistics


_spec = importlib.util.spec_from_file_location(
    "prompt_family_stats", Path(__file__).with_name("summarize-prompt-community.py"))
family_stats = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(family_stats)


def percentile(values, fraction):
    values = sorted(values)
    if not values:
        return None
    index = (len(values) - 1) * fraction
    low = int(index)
    high = min(low + 1, len(values) - 1)
    return round(values[low] + (values[high] - values[low]) * (index - low), 3)


def summarize(rows):
    families = family_stats.family_groups(rows)
    scenarios = defaultdict(list)
    for row in rows:
        scenarios[row["scenario"]].append(row)
    success = sum(all(row["passed"] for row in task) for task in families.values())
    usages = [row.get("observed_usage", row.get("usage")) for row in rows]
    usages = [usage for usage in usages if usage is not None]
    tokens = {field: sum(usage.get(field, 0) for usage in usages)
              for field in ("input_tokens", "output_tokens", "cache_hit_tokens",
                            "cache_miss_tokens", "cache_write_tokens", "reasoning_tokens")}
    requests = [request for row in rows for request in row["model_requests"]]
    ttft = [request["ttft_ms"] for request in requests if request.get("ttft_ms") is not None]
    return {
        "turns": len(rows), "passed_turns": sum(row["passed"] for row in rows),
        "families": len(families), "passed_families": success,
        "elapsed_total_secs": round(sum(row["elapsed_secs"] for row in rows), 3),
        "turn_median_secs": round(statistics.median(row["elapsed_secs"] for row in rows), 3),
        "turn_p90_secs": percentile([row["elapsed_secs"] for row in rows], 0.9),
        "family_median_secs": round(statistics.median(sum(row["elapsed_secs"] for row in task)
                                                    for task in families.values()), 3),
        "request_count": len(requests), "ttft_samples": len(ttft),
        "request_ttft_median_ms": percentile(ttft, 0.5),
        "request_ttft_p90_ms": percentile(ttft, 0.9),
        "tool_calls": sum(row["tool_calls"] for row in rows),
        "failed_tools": sum(row["failed_tools"] for row in rows),
        "unsettled_tools": sum(row.get("unsettled_tools", 0) for row in rows),
        "tool_accounting_corrections": sum(row.get("tool_accounting_changed", False) for row in rows),
        "partial_usage_turns": sum(row.get("usage_partial", row.get("usage") is None) for row in rows),
        "turns_missing_usage": len(rows) - len(usages), "usage": tokens,
        "observed_tokens_per_successful_family":
            round((tokens["input_tokens"] + tokens["output_tokens"]) / success, 1) if success else None,
        "scenarios": {name: {
            "passed_turns": sum(row["passed"] for row in task), "turns": len(task),
            "median_secs": round(statistics.median(row["elapsed_secs"] for row in task), 3),
            "failed_checks": [key for row in task for key, passed in row["checks"].items() if not passed],
        } for name, task in sorted(scenarios.items())},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("receipts", type=Path, nargs="+")
    args = parser.parse_args()
    groups = defaultdict(list)
    keys = set()
    for path in args.receipts:
        for line in path.read_text().splitlines():
            row = json.loads(line)
            key = (row["variant"], row["model"], row["repetition"], row["scenario"], row["turn"])
            if key in keys:
                raise ValueError(f"duplicate receipt: {key}")
            keys.add(key)
            groups[(row["variant"], row["model"])].append(row)
    print(json.dumps([dict(variant=variant, model=model, **summarize(rows))
                      for (variant, model), rows in sorted(groups.items())], ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
