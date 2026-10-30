#!/usr/bin/env python3
"""Opt-in V2.4 repair comparison, reusing the isolated runner and known cases."""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

import prompt_eval_repair_cases as cases


def attach_cases(community, suite=cases):
    community.cases = suite
    community.extra_source_files = (Path(__file__),)
    # New, delegated and public-API generated-code graders all stay isolated.
    suite.subprocess = community.sandbox.VerifierSubprocess
    suite.previous.subprocess = community.sandbox.VerifierSubprocess
    suite.api.VerifierSubprocess = community.sandbox.VerifierSubprocess
    prepare = suite.prepare

    def prepare_with_snapshot(case, root, home, legacy):
        request, prompts = prepare(case, root, home, legacy)
        if case in ("nested_root_rule", "plan_secondary_readonly"):
            legacy.write(root / "retest-secondary-snapshot.json", json.dumps(legacy.project_snapshot(root / "secondary")))
        elif case == "third_root":
            snapshots = {name: legacy.project_snapshot(root / folder)
                         for name, folder in (("secondary", "secondary"), ("third", "third root"))}
            legacy.write(root / "repair-root-snapshots.json", json.dumps(snapshots))
        return request, prompts

    suite.prepare = prepare_with_snapshot


if __name__ == "__main__":
    spec = importlib.util.spec_from_file_location("repair_community", Path(__file__).with_name("eval-prompt-community.py"))
    community = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(community)
    attach_cases(community)
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("baseline", "candidate", "home-template", "credential-cli", "xvfb", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--model-id", required=True)
    parser.add_argument("--model-alias", required=True)
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--cases")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True, mode=0o700)
    sources = (Path(__file__), Path(cases.__file__), Path(cases.previous.__file__), Path(cases.api.__file__))
    extension = {"suite": "pinvou-v2.4-repair", "fresh_holdouts": [],
                 "known_regression_cases": [*community.legacy.CASES, *cases.EXTRA_CASES],
                 "api_contract": "original names/kinds/defaults and correct keyword calls for six implemented-code families",
                 "fidelity_format": "one nonempty progress line with at most one of 。！？!?",
                 "source_hashes": {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in sources}}
    manifest = args.output / "extension-manifest.json"
    if manifest.exists() and json.loads(manifest.read_text()) != extension:
        raise ValueError("resume requires identical repair extension")
    manifest.write_text(json.dumps(extension, indent=2))
    community.run(args)
