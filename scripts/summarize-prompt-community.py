#!/usr/bin/env python3
"""Summarize complete paired trials; keep outcome, workflow and safety distinct."""

import argparse
from collections import Counter, defaultdict
import json
import math
from pathlib import Path
import random
import statistics


def percentile(values, q):
    values = sorted(values)
    if not values:
        return None
    index = (len(values) - 1) * q
    low = int(index)
    high = min(low + 1, len(values) - 1)
    return round(values[low] + (values[high] - values[low]) * (index - low), 4)


def pass_power(successes, attempts, k):
    return math.comb(successes, k) / math.comb(attempts, k) if successes >= k else 0.0


def family_groups(rows, repetitions=None):
    groups = defaultdict(list)
    keys = set()
    for row in rows:
        key = (row["variant"], row["model"], row["repetition"], row["scenario"], row["turn"])
        if key in keys:
            raise ValueError("duplicate user-turn receipt")
        keys.add(key)
        groups[(row["model"], row["variant"], row["scenario"], row["repetition"])].append(row)
    for (_, _, case, repetition), turns in groups.items():
        expected = 2 if case in ["artifact", "unbound_artifact", "plan_execute", "skill"] else 1
        if (sorted(row["turn"] for row in turns) != list(range(expected))
                or repetition < 0 or repetitions is not None and repetition >= repetitions):
            raise ValueError("incomplete task family")
    return groups


def summarize(rows, repetitions=5):
    families = family_groups(rows, repetitions)
    groups = defaultdict(list)
    for key, turns in families.items():
        model, variant, case, repetition = key
        groups[(model, variant)].append((case, repetition, turns))
    summaries = []
    task_success = {}
    for (model, variant), tasks in sorted(groups.items()):
        by_case = defaultdict(list)
        partitions = defaultdict(lambda: {"task_trials": 0, "passed_task_trials": 0})
        dimensions = {name: [0, 0] for name in ["outcome", "workflow", "safety"]}
        for case, repetition, turns in tasks:
            passed = all(row["passed"] for row in turns)
            by_case[case].append(passed)
            partition = partitions[turns[0]["partition"]]
            partition["task_trials"] += 1
            partition["passed_task_trials"] += passed
            task_success[(model, variant, case, repetition)] = int(passed)
            for dimension, counts in dimensions.items():
                checks = [v for row in turns for v in row["dimensions"][dimension].values()]
                if checks:
                    counts[1] += 1
                    counts[0] += all(checks) and all(row["checks"]["completed"] for row in turns)
        if any(len(values) != repetitions for values in by_case.values()):
            raise ValueError("missing repetitions")
        subset = [row for _, _, turns in tasks for row in turns]
        usages = [r.get("observed_usage", r.get("usage")) for r in subset]
        usages = [u for u in usages if u is not None]
        usage = {key: sum(u.get(key, 0) or 0 for u in usages) for key in
                 ["input_tokens", "output_tokens", "reasoning_tokens", "cache_hit_tokens", "cache_miss_tokens"]}
        requests = [request for row in subset for request in row["model_requests"]]
        latencies = [sum(r["elapsed_secs"] for r in turns) for _, _, turns in tasks]
        successes = sum(sum(values) for values in by_case.values())
        errors = Counter()
        for row in subset:
            errors.update(row.get("tool_failure_kinds", {}))
        summaries.append(dict(model=model, variant=variant, turns=len(subset),
                              passed_turns=sum(r["passed"] for r in subset), task_trials=len(tasks),
                              passed_task_trials=successes, dimensions={name: {"passed": n[0], "applicable": n[1]} for name, n in dimensions.items()},
                              partitions=dict(partitions),
                              per_case={case: {"passed": sum(values), "attempts": len(values)} for case, values in sorted(by_case.items())},
                              observed_pass_power={str(k): statistics.mean(pass_power(sum(v), repetitions, k) for v in by_case.values()) for k in [1, 2, 3, 5]},
                              all_five_cases=sum(all(v) for v in by_case.values()), cases=len(by_case),
                              task_latency_median_secs=percentile(latencies, .5), task_latency_p90_secs=percentile(latencies, .9),
                              main_request_ttft_median_ms=percentile([r["ttft_ms"] for r in requests if r.get("ttft_ms") is not None], .5),
                              tool_calls=sum(r["tool_calls"] for r in subset), failed_tools=sum(r["failed_tools"] for r in subset),
                              unsettled_tools=sum(r["unsettled_tools"] for r in subset), tool_failure_kinds=dict(errors),
                              usage=usage, turns_missing_any_observed_usage=len(subset)-len(usages),
                              turns_missing_full_usage=sum(r.get("usage") is None for r in subset),
                              observed_tokens_per_passing_task=(usage["input_tokens"]+usage["output_tokens"])/successes if successes else None,
                              semantic_warning_turns=sum(bool(r.get("semantic_warnings")) for r in subset)))
    comparisons = []
    for model in sorted({row["model"] for row in rows}):
        cases = sorted({case for m, _, case, _ in families if m == model})
        deltas = []
        improved = regressed = 0
        for case in cases:
            differences = []
            for repetition in range(repetitions):
                a = task_success[(model, "baseline", case, repetition)]
                b = task_success[(model, "candidate", case, repetition)]
                differences.append(b-a)
                improved += b > a
                regressed += a > b
            deltas.append(statistics.mean(differences))
        rng = random.Random(91457)
        bootstrap = [statistics.mean(rng.choices(deltas, k=len(deltas))) for _ in range(10000)]
        comparisons.append(dict(model=model, task_success_delta=statistics.mean(deltas),
                                improved_pairs=improved, regressed_pairs=regressed,
                                descriptive_scenario_cluster_bootstrap_95=[percentile(bootstrap, .025), percentile(bootstrap, .975)]))
    return {"groups": summaries, "paired_comparisons": comparisons,
            "interval_scope": "descriptive scenario-cluster bootstrap conditional on this synthetic task mix; not a population confidence guarantee"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("receipts", type=Path, nargs="+")
    parser.add_argument("--repetitions", type=int, default=5)
    args = parser.parse_args()
    rows = [json.loads(line) for p in args.receipts for line in p.read_text().splitlines() if line.strip()]
    print(json.dumps(summarize(rows, args.repetitions), ensure_ascii=False, indent=2))
