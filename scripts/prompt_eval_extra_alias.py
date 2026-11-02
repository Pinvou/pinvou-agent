#!/usr/bin/env python3
"""Supplemental Python-value alias diagnostic, separate from frozen JSON-shaped tests.

Added after inspecting a JSON-only copy implementation. Run uniformly on both
versions; do not relabel this as part of the original 104-case primary oracle.
Generated code always runs in the read-only, networkless verifier namespace.
"""

import argparse
from pathlib import Path
import sys

from prompt_eval_sandbox import VerifierSubprocess


PROGRAM = r'''
import copy
import runpy
import sys

fn = runpy.run_path(sys.argv[1])['merge_config']


def reference(defaults, overrides):
    result = copy.deepcopy(defaults)
    for key, value in overrides.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = reference(result[key], value)
        else:
            result[key] = copy.deepcopy(value)
    return result


def mutate(value):
    if isinstance(value, dict):
        for item in list(value.values()):
            mutate(item)
        value['__extra__'] = True
    elif isinstance(value, list):
        for item in list(value):
            mutate(item)
        value.append('__extra__')
    elif isinstance(value, tuple):
        for item in value:
            mutate(item)
    elif isinstance(value, set):
        value.add('__extra__')
    elif isinstance(value, bytearray):
        value.extend(b'changed')


pairs = [({'x': ([1], {'v': [2]})}, {}),
         ({}, {'x': ([3], {'v': [4]})}),
         ({'x': {1, 2}}, {}),
         ({}, {'x': bytearray(b'abc')})]
for defaults, overrides in pairs:
    before_defaults, before_overrides = copy.deepcopy(defaults), copy.deepcopy(overrides)
    result = fn(defaults, overrides)
    assert result == reference(defaults, overrides)
    assert defaults == before_defaults and overrides == before_overrides
    mutate(result)
    assert defaults == before_defaults and overrides == before_overrides
print('EXTRA_ORACLE_OK')
'''


def check(path):
    try:
        result = VerifierSubprocess.run([sys.executable, "-c", PROGRAM, str(path.resolve())],
                                       capture_output=True, text=True, timeout=10)
        return result.returncode == 0 and "EXTRA_ORACLE_OK" in result.stdout
    except VerifierSubprocess.TimeoutExpired:
        return False


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path)
    args = parser.parse_args()
    passed = check(args.path)
    print("passed" if passed else "failed")
    raise SystemExit(0 if passed else 1)
