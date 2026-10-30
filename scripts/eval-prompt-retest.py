#!/usr/bin/env python3
"""Opt-in V2.2 retest using the unchanged isolated community runner."""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

import prompt_eval_retest_cases as cases


spec = importlib.util.spec_from_file_location("community", Path(__file__).with_name("eval-prompt-community.py"))
community = importlib.util.module_from_spec(spec)
spec.loader.exec_module(community)
community.cases = cases
community.extra_source_files = (Path(__file__),)
cases.subprocess = community.sandbox.VerifierSubprocess
prepare = cases.prepare


def prepare_with_snapshot(case, root, home, legacy):
    request, prompts = prepare(case, root, home, legacy)
    if case in ("nested_root_rule", "plan_secondary_readonly"):
        legacy.write(root / "retest-secondary-snapshot.json", json.dumps(legacy.project_snapshot(root / "secondary")))
    return request, prompts


cases.prepare = prepare_with_snapshot


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("baseline", "candidate", "home-template", "credential-cli", "xvfb", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--model-id", required=True)
    parser.add_argument("--model-alias", required=True)
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--cases")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True, mode=0o700)
    extension = {"suite": "pinvou-v2.2-retest", "fresh_holdouts": list(cases.FRESH_CASES),
                 "known_extra_regressions": ["unbound_artifact"],
                 "source_hashes": {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                                   for p in (Path(__file__), Path(cases.__file__))}}
    manifest = args.output / "extension-manifest.json"
    if manifest.exists() and json.loads(manifest.read_text()) != extension:
        raise ValueError("resume requires identical retest extension")
    manifest.write_text(json.dumps(extension, indent=2))
    community.run(args)
