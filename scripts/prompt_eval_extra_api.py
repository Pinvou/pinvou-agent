"""Post-freeze API diagnostics; never replace the frozen primary score."""

import json
from pathlib import Path
import subprocess
import sys

from prompt_eval_sandbox import VerifierSubprocess


CONTRACTS = {
    "discount": {"parameters": ["price", "percent"], "calls": [
        [{"price": 100, "percent": 20}, 80], [{"price": 50, "percent": 0}, 50]]},
    "shipping_fee": {"parameters": ["items"], "calls": [
        [{"items": 0}, 0], [{"items": 3}, 300], [{"items": 17}, 1700]]},
    "total_fee": {"parameters": ["count"], "calls": [
        [{"count": 0}, 0], [{"count": 2}, 14], [{"count": 17}, 119]]},
    "merge_config": {"parameters": ["defaults", "overrides"], "calls": [
        [{"defaults": {"a": {"x": [1]}, "keep": True},
          "overrides": {"a": {"y": None}}},
         {"a": {"x": [1], "y": None}, "keep": True}]]},
}

MARKER = "PINVOU_API_DIAGNOSTIC:"
PROGRAM = r'''
import inspect, json, runpy, sys
source, function, contract = sys.argv[1:]
contract = json.loads(contract)
fn = runpy.run_path(source)[function]
parameters = list(inspect.signature(fn).parameters.values())
signature_ok = (
    [p.name for p in parameters] == contract['parameters']
    and all(p.kind is inspect.Parameter.POSITIONAL_OR_KEYWORD
            and p.default is inspect.Parameter.empty for p in parameters)
)
try:
    keyword_ok = all(fn(**arguments) == expected for arguments, expected in contract['calls'])
except Exception:
    keyword_ok = False
print('PINVOU_API_DIAGNOSTIC:' + json.dumps({
    'signature_preserved': signature_ok, 'keyword_calls_correct': keyword_ok
}))
'''


def check(source, function):
    """Check original parameter kinds/names/defaults and keyword-call behavior."""
    if function not in CONTRACTS:
        raise ValueError("unknown evaluation API contract")
    source = Path(source).resolve()
    failed = {"signature_preserved": False, "keyword_calls_correct": False}
    try:
        result = VerifierSubprocess.run(
            [sys.executable, "-c", PROGRAM, str(source), function, json.dumps(CONTRACTS[function])],
            cwd=source.parent, capture_output=True, text=True, timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired):
        return failed
    if result.returncode:
        return failed
    lines = [line[len(MARKER):] for line in result.stdout.splitlines() if line.startswith(MARKER)]
    try:
        checks = json.loads(lines[-1])
    except (IndexError, ValueError):
        return failed
    if not isinstance(checks, dict) or set(checks) != set(failed) or any(type(value) is not bool for value in checks.values()):
        return failed
    return checks
