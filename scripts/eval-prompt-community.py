#!/usr/bin/env python3
"""Opt-in frozen, paired live Pinvou evaluation using community evaluation methods.

Public benchmark protocols inform this custom suite; these are not official
benchmark scores. Raw trajectories/settings remain in a private local directory.
"""

import argparse
from collections import Counter
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time

import prompt_eval_community_cases as cases
import prompt_eval_sandbox as sandbox
from prompt_eval_model_proxy import ModelProxy


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


legacy = load("legacy_prompt_eval", "eval-prompt-suite.py")
audit = load("prompt_audit", "audit-prompt-eval.py")
legacy.subprocess = sandbox.VerifierSubprocess
audit.subprocess = sandbox.VerifierSubprocess
cases.subprocess = sandbox.VerifierSubprocess


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True, mode=0o700)
    sandbox.self_test()
    proxy_socket = output / "model-proxy.sock"
    proxy = ModelProxy(args.credential_cli, args.home_template, args.model_id, socket_path=proxy_socket)
    settings = proxy.settings
    profile = next(m for m in settings["advanced"]["saved_models"] if m["id"] == args.model_id)
    variants = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    all_cases = (*legacy.CASES, *cases.EXTRA_CASES)
    chosen = args.cases.split(",") if args.cases else list(all_cases)
    if any(case not in all_cases for case in chosen):
        raise ValueError("unknown case")
    source_files = [Path(__file__), Path(cases.__file__), Path(legacy.__file__), Path(audit.__file__),
                    Path(sandbox.__file__), Path(__file__).with_name("prompt_eval_model_proxy.py"),
                    Path(__file__).with_name("prompt_eval_namespace_bootstrap.py")]
    # Include every delegated grader generation, not only the selected leaf.
    ancestor = getattr(cases, "previous", None)
    while ancestor is not None:
        source_files.append(Path(ancestor.__file__))
        ancestor = getattr(ancestor, "previous", None)
    manifest = {"suite": "pinvou-community-method-v2-isolated", "repetitions": args.repetitions,
                "model": profile["model"], "model_alias": args.model_alias,
                "model_id": args.model_id, "scenarios": chosen,
                "source_hashes": {p.name: digest(p) for p in source_files},
                "settings_template_sha256": digest(args.home_template / "settings.json"),
                "binary_hashes": {name: digest(p) for name, p in variants.items()},
                "runtime_hashes": {"credential_cli": digest(args.credential_cli), "xvfb": digest(args.xvfb)},
                "order": "Alternate AB/BA by repetition and case; one fresh home per trial; same-model pairs serialized",
                "scope": "custom Pinvou suite; not official public benchmark scores",
                "isolation": "bubblewrap mount/PID/network/IPC namespaces; one trial only; no harness, neighbors, real home or credentials; only scoped model proxy socket; generated-code verification read-only and networkless",
                "transport": "real configured endpoints via streaming credential-isolating pass-through; request bodies unchanged",
                "settings_policy": "configured routes and sampling, memory off, max_steps=40"}
    manifest["grader_revision"] = hashlib.sha256(
        json.dumps(manifest["source_hashes"], sort_keys=True).encode()).hexdigest()
    mf = output / "manifest.json"
    if mf.exists() and json.loads(mf.read_text()) != manifest:
        raise ValueError("resume requires identical protocol, sources, binaries and model")
    legacy.write(mf, json.dumps(manifest, indent=2))
    receipt = output / "results.jsonl"
    rows = legacy.load_receipts(receipt)
    done = {(r["variant"], r["repetition"], r["scenario"], r["turn"]) for r in rows}
    for repetition in range(args.repetitions):
        for index, case in enumerate(chosen):
            order = list(variants) if (repetition + index) % 2 == 0 else list(reversed(variants))
            for variant in order:
                trial_id = hashlib.sha256(f"{variant}:{repetition}:{case}".encode()).hexdigest()[:16]
                root = output / "trials" / f"trial-{trial_id}"
                home = root / "home"
                cf = root / "case.json"
                if cf.exists():
                    saved = json.loads(cf.read_text())
                    request, prompts, before, snapshot = (saved[k] for k in ["request", "prompts", "before", "snapshot"])
                    # A resumed process owns a fresh proxy capability. Refresh
                    # transport credentials without resetting the session/files.
                    if any((variant, repetition, case, turn) not in done for turn in range(len(prompts))):
                        config = json.loads((home / "settings.json").read_text())
                        config["advanced"]["saved_models"] = [profile]
                        config["advanced"]["active_model_id"] = args.model_id
                        config["advanced"]["custom_base_url"] = profile["base_url"]
                        legacy.write(home / "settings.json", json.dumps(config))
                        (home / "settings.json").chmod(0o600)
                else:
                    if root.exists():
                        raise ValueError("fixture exists without a manifest")
                    home.mkdir(parents=True, mode=0o700)
                    config = json.loads(json.dumps(settings))
                    config["advanced"]["saved_models"] = [profile]
                    config["advanced"]["active_model_id"] = args.model_id
                    config["advanced"]["max_steps"] = 40
                    config["memory_enabled"] = False
                    legacy.write(home / "settings.json", json.dumps(config))
                    (home / "settings.json").chmod(0o600)
                    request, prompts = cases.prepare(case, root, home, legacy)
                    request["model_id"] = args.model_id
                    p = root / "primary/calculator.py"
                    before = p.read_text() if p.exists() else ""
                    snapshot = legacy.project_snapshot(root / "primary")
                    legacy.write(cf, json.dumps(dict(request=request, prompts=prompts, before=before, snapshot=snapshot), ensure_ascii=False))
                for turn, prompt in enumerate(prompts):
                    key = (variant, repetition, case, turn)
                    if key in done:
                        prior = json.loads((root / f"response-{turn}.json").read_text())
                        if prior.get("session_id"):
                            request["session_id"] = prior["session_id"]
                        continue
                    request["prompt"] = prompt
                    if case == "plan_execute" and turn:
                        request["mode"] = "agent"
                    req = root / f"request-{turn}.json"
                    legacy.write(req, json.dumps(sandbox.translate(request, root), ensure_ascii=False))
                    env = dict(os.environ, PINVOU3_HOME=str(home), PINVOU3_AGENT_TASK_KEEP_SESSION="1")
                    started = time.monotonic()
                    try:
                        result = subprocess.run(sandbox.command(variants[variant], req, root, home,
                                                                proxy_socket=proxy_socket, xvfb=args.xvfb), capture_output=True,
                                                text=True, env=env, timeout=request["timeout_secs"] + 60)
                        legacy.write(root / f"stdout-{turn}.log", result.stdout)
                        legacy.write(root / f"stderr-{turn}.log", result.stderr)
                        try:
                            report = json.loads(result.stdout)
                        except ValueError:
                            report = {"status": "host_error", "tool_events": [], "error_kind": "invalid_report"}
                        return_code = result.returncode
                    except subprocess.TimeoutExpired as error:
                        for label, content in [("stdout", error.stdout), ("stderr", error.stderr)]:
                            if isinstance(content, bytes):
                                content = content.decode("utf-8", errors="replace")
                            legacy.write(root / f"{label}-{turn}.log", content or "")
                        report = {"status": "host_timeout", "timed_out": True, "tool_events": []}
                        return_code = None
                    if not report.get("session_id"):
                        # Preserve observable partial work when the host guard,
                        # rather than the product deadline, stopped a turn.
                        matching = []
                        for path in (home / "sessions").glob("*.json"):
                            try:
                                messages = json.loads(path.read_text()).get("messages", [])
                            except (OSError, ValueError):
                                continue
                            if any(message.get("role") == "user" and any(
                                block.get("type") == "text" and prompt in block.get("text", "")
                                for block in message.get("content", [])) for message in messages):
                                matching.append(path.stem)
                        if len(matching) == 1:
                            report["session_id"] = matching[0]
                    elapsed = time.monotonic() - started
                    legacy.write(root / f"response-{turn}.json", json.dumps(report, ensure_ascii=False))
                    artifacts = {}
                    for relative in ["primary/report.md", "secondary/report.md"]:
                        path = root / relative
                        if path.is_file():
                            artifacts[relative] = path.read_text()
                    legacy.write(root / f"artifacts-{turn}.json", json.dumps(artifacts, ensure_ascii=False))
                    events = sandbox.normalize_event_paths(audit.tool_events(audit.turn_blocks(root, report, turn)), root)
                    checks, dimensions = cases.grade(case, turn, root, home, report, before, snapshot, events, legacy, audit)
                    names = [e["name"] for e in events]
                    usage = report.get("usage")
                    requests = legacy.request_metrics(home, report.get("session_id"))
                    observed_usage = usage
                    if usage is None and requests:
                        observed_usage = {field: sum(request.get(field) or 0 for request in requests)
                                          for field in ["input_tokens", "output_tokens", "cache_hit_tokens", "cache_miss_tokens"]}
                    row = dict(variant=variant, model_id=args.model_alias, model=profile["model"],
                               grader_revision=manifest["grader_revision"],
                               repetition=repetition, scenario=case, turn=turn, status=report.get("status"),
                               passed=all(checks.values()), checks=checks, dimensions=dimensions,
                               elapsed_secs=round(elapsed, 3), return_code=return_code,
                               tool_calls=len(events), failed_tools=sum(e["failed"] is True for e in events),
                               unsettled_tools=sum(e["failed"] is None for e in events), tool_names=names,
                               tool_failure_kinds=dict(Counter(e.get("failure_kind", "tool_error") for e in events if e["failed"] is True)),
                               usage=usage, usage_partial=usage is None,
                               observed_usage=observed_usage,
                               usage_scope="completed_main_requests_only_partial" if usage is None else "whole_turn_including_children",
                               model_requests=requests, model_request_scope="main_session", tool_scope="main_session")
                    row["partition"] = ("holdout" if case in cases.EXTRA_CASES else
                                        "browser_boundary" if case == "browser_host_boundary" else "regression")
                    if case == "artifact" and turn:
                        target = root / "primary/report.md"
                        row["semantic_warnings"] = ([] if target.exists() and cases.acceptance_consistent(target.read_text())
                                                    else ["acceptance_wording_or_pending_step_requires_review"])
                    with receipt.open("a", encoding="utf-8") as sink:
                        sink.write(json.dumps(row, ensure_ascii=False) + "\n")
                    done.add(key)
                    print(json.dumps({k: row[k] for k in ["variant", "model_id", "repetition", "scenario", "turn", "passed", "elapsed_secs"]}), flush=True)
                    if report.get("session_id"):
                        request["session_id"] = report["session_id"]
    legacy.write(output / "wire-shapes.json", json.dumps(proxy.wire_shapes, indent=2))
    proxy.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--home-template", type=Path, required=True)
    parser.add_argument("--credential-cli", type=Path, required=True)
    parser.add_argument("--xvfb", type=Path, required=True)
    parser.add_argument("--model-id", required=True)
    parser.add_argument("--model-alias", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--cases")
    run(parser.parse_args())
