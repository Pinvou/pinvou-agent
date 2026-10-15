#!/usr/bin/env python3
"""内置 MCP 契约矩阵：真实启动 stdio server，检查协议、工具调用与本地产物。"""
from __future__ import annotations

import json
import math
import os
import queue
import re
import subprocess
import sys
import tempfile
import threading
import time
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MCP_ROOT = ROOT / "pinvou3-app" / "resources" / "mcp-servers"

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")


class RpcServer:
    def __init__(
        self,
        server_dir: Path,
        env: dict[str, str] | None = None,
        timeout_s: float | None = None,
        args: list[str] | None = None,
    ):
        child_env = os.environ.copy()
        child_env.update(env or {})
        self.server_dir = server_dir
        self.timeout_s = timeout_s or float(os.environ.get("PINVOU3_MCP_RPC_TIMEOUT_SECS", "10"))
        self.proc = subprocess.Popen(
            [sys.executable, "server.py", *(args or [])],
            cwd=server_dir,
            env=child_env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        self.seq = 0

    def call(self, method: str, params=None) -> dict:
        self.seq += 1
        request = {"jsonrpc": "2.0", "id": self.seq, "method": method}
        if params is not None:
            request["params"] = params
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        self.proc.stdin.flush()
        result: queue.Queue[tuple[str | None, BaseException | None]] = queue.Queue(maxsize=1)

        def read_response() -> None:
            try:
                result.put((self.proc.stdout.readline(), None))
            except BaseException as exc:  # surface pipe/decoder failures in the caller
                result.put((None, exc))

        threading.Thread(target=read_response, daemon=True).start()
        try:
            line, read_error = result.get(timeout=self.timeout_s)
        except queue.Empty as exc:
            self._terminate()
            stderr = self._stderr_tail()
            raise AssertionError(
                f"MCP {self.server_dir.name} {method} timed out after "
                f"{self.timeout_s:g}s; stderr tail: {stderr or '(empty)'}"
            ) from exc
        if read_error is not None:
            self._terminate()
            raise AssertionError(
                f"MCP {self.server_dir.name} {method} response read failed: {read_error}"
            ) from read_error
        assert line is not None
        if not line:
            self._terminate()
            stderr = self._stderr_tail()
            raise AssertionError(f"MCP server exited without response: {stderr[-1000:]}")
        response = json.loads(line.lstrip("\ufeff"))
        assert response.get("id") == self.seq, response
        return response

    def _terminate(self) -> None:
        if self.proc.poll() is not None:
            return
        self.proc.terminate()
        try:
            self.proc.wait(timeout=1)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=1)

    def _stderr_tail(self) -> str:
        if not self.proc.stderr:
            return ""
        return self.proc.stderr.read()[-1000:].strip()

    def close(self):
        if self.proc.stdin:
            try:
                self.proc.stdin.close()
            except OSError:
                pass
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self._terminate()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def content_json(response: dict) -> dict:
    assert "error" not in response, response
    content = response["result"]["content"]
    assert content and content[0]["type"] == "text", response
    return json.loads(content[0]["text"])


# ── Model-visible schema contract ──────────────────────────────────────
# Background (the k3 slides:[] incident): with `type: "array"` and no items,
# constrained tool-call decoding channels (Kimi and friends) complete the
# parameter to its minimal valid value [] (observed live: slides:[] emitted
# 27 times in a row). The validator on that route (MFJS, CodeWhale
# schema_sanitize.rs) is much stricter than JSON Schema, and every deviation
# is the same silent failure — the whole tool is dropped (MissingType /
# UnsupportedKeyword / InvalidSchemaNode), which is how #683 lost
# evaluate_script. The shape set below is verified against the validator
# source at the pinned gitlink, not assumed, and this guard enforces it:
#   - every schema position holds an object; boolean and list values are
#     InvalidSchemaNode, empty nodes are MissingType (the engine repairs
#     exactly the items:{} shape, nothing else);
#   - "type", where present, must be a single concrete MFJS string:
#     list-typed ("type": ["array"]), non-string, and unknown values arrive
#     typeless through the engine's string-only sanitizer and die with
#     MissingType;
#   - a node without "type" must carry anyOf — typeless annotation-only
#     nodes ({"description": ...}, {"enum": ...}) are MissingType;
#   - any keyword outside MFJS's ALLOWED_KEYWORDS is rejected outright
#     (UnsupportedKeyword), so oneOf/allOf/patternProperties/prefixItems/
#     contains/unevaluated*/... are banned here: a bundled server must never
#     ship a schema the validator would silently discard. $ref/$dynamicRef/
#     $recursiveRef are banned even though MFJS allows $ref, because this
#     guard cannot follow the target and the shape underneath would go
#     uninspected;
#   - a keyword that survives the whitelist must still be admissible for its
#     node's type (mfjs_keyword_allowed_for_type): `default` on an array or
#     `enum`/`default` on an object is InvalidKeywordValue — the same silent
#     drop (review #713 round 7, P2-3). Enum and default literals must match
#     their node's type; required must name unique declared properties;
#     minLength/maxLength (string), minItems/maxItems (array), and
#     minimum/maximum (number/integer) reject foreign types, out-of-range
#     literals, and inverted ranges (InvalidRange);
#   - the parameters root must be a typed object without composition
#     (RootMustBeObject);
#   - the validator's resource budgets are mirrored exactly (anyOf ≤ 10
#     branches, ≤ 100 total properties, property depth ≤ 5, ≤ 500 total enum
#     values, ≤ 64 structural depth, ≤ 120000 encoded bytes) —
#     ResourceLimitExceeded drops the tool like every other deviation.
#   - anyOf keeps only the union's own siblings (MFJS InvalidKeywordValue
#     otherwise) and every branch is walked and concretely typed; $defs
#     interiors are walked so an unreferenced definition is no blind spot
#     ($defs/$id are root-only for MFJS).
_MFJS_CONCRETE_TYPES = ("string", "number", "integer", "boolean", "object", "array", "null")
_MFJS_ALLOWED_KEYWORDS = frozenset((
    "$id", "$ref", "$defs", "anyOf", "properties", "additionalProperties",
    "items", "type", "enum", "required", "maxLength", "minLength",
    "maximum", "minimum", "maxItems", "minItems", "title", "description",
    "default",
))
# Per-type keyword admissibility, mirroring mfjs_keyword_allowed_for_type in
# schema_sanitize.rs: a keyword surviving the whitelist is still
# InvalidKeywordValue when its node's type does not admit it — `default` on
# an array and `enum`/`default` on an object are the same silent tool drop
# (review #713 round 7, P2-3). type/title/description are admissible on
# every node and root $defs/$id on the root, handled by the other rules.
_MFJS_KEYWORDS_FOR_TYPE = {
    "object": ("properties", "required", "additionalProperties"),
    "array": ("items", "minItems", "maxItems"),
    "string": ("enum", "default", "minLength", "maxLength"),
    "number": ("enum", "default", "minimum", "maximum"),
    "integer": ("enum", "default", "minimum", "maximum"),
    "boolean": ("default",),
    "null": ("default",),
}
# Resource budgets, mirroring the validator constants exactly
# (ResourceLimitExceeded raise sites in schema_sanitize.rs).
_MFJS_MAX_ANY_OF_ITEMS = 10
_MFJS_MAX_OBJECT_DEPTH = 5
_MFJS_MAX_TOTAL_PROPERTIES = 100
_MFJS_MAX_TOTAL_ENUM_VALUES = 500
# validate_mfjs_enum's conditional literal budget: an enum with more than 250
# entries may not carry more than 7500 combined literal bytes (str::len for
# strings, serde to_string() length for the numeric literals).
_MFJS_ENUM_LENGTH_CHECK_THRESHOLD = 250
_MFJS_MAX_ENUM_STRING_LENGTH = 7_500
_MFJS_MAX_SCHEMA_BYTES = 120_000
_MFJS_MAX_STRUCTURAL_DEPTH = 64
_MFJS_MAX_SAFE_INTEGER = 9_007_199_254_740_991
# Property names the validator refuses outright (InvalidKeywordValue).
_MFJS_RESERVED_PROPERTY_NAMES = frozenset(
    ("$defs", "$ref", "anyOf", "required", "additionalProperties"))


def _mfjs_is_number(value) -> bool:
    """A JSON number; bool is an int in Python but not a number for serde."""
    return (isinstance(value, (int, float)) and not isinstance(value, bool)
            and (not isinstance(value, float) or math.isfinite(value)))


def _mfjs_is_integer_value(value) -> bool:
    """mfjs_integer_value: finite, integral, |v| ≤ 2^53-1."""
    return (_mfjs_is_number(value) and float(value).is_integer()
            and abs(float(value)) <= _MFJS_MAX_SAFE_INTEGER)


def _mfjs_is_u64(value) -> bool:
    return (isinstance(value, int) and not isinstance(value, bool)
            and 0 <= value <= 2 ** 64 - 1)


_MFJS_ENUM_VALUE_MATCHERS = {
    "string": lambda values: all(isinstance(value, str) for value in values),
    "integer": lambda values: all(_mfjs_is_integer_value(value) for value in values),
    "number": lambda values: all(_mfjs_is_number(value) for value in values),
}

_MFJS_DEFAULT_VALUE_MATCHERS = {
    "boolean": lambda value: isinstance(value, bool),
    "null": lambda value: value is None,
    "string": lambda value: isinstance(value, str),
    "number": _mfjs_is_number,
    "integer": _mfjs_is_integer_value,
}


class _MfjsBudget:
    """Whole-document counters for one inputSchema walk — the validator's
    MfjsValidationState. Property/enum totals accumulate across every node,
    so they cannot be carried by the recursion itself; each overflow is
    reported once."""

    def __init__(self, root):
        self.total_properties = 0
        self.total_enum_values = 0
        self.properties_budget_reported = False
        self.enum_budget_reported = False
        # serde_json::to_vec(parameters).len() at the validator's entry.
        self.encoded_bytes = len(json.dumps(
            root, separators=(",", ":"), ensure_ascii=False).encode("utf-8"))


def _schema_has_concrete_type(node) -> bool:
    if not isinstance(node, dict) or not node:
        return False
    if node.get("type") in _MFJS_CONCRETE_TYPES:
        return True
    # anyOf is MFJS's only union: any branch may be taken, so every branch
    # must be concrete (oneOf/allOf are not MFJS keywords at all — the
    # keyword ban flags them before this helper is even asked).
    branches = node.get("anyOf")
    return isinstance(branches, list) and bool(branches) and all(
        _schema_has_concrete_type(branch) for branch in branches)


def _walk_tool_schema(node, path, offenders, is_root=False, property_depth=0,
                      structural_depth=0, budget=None):
    """Walk schema-value positions only; {} inside enum/default/const/examples is data, never walked."""
    if isinstance(node, bool):
        # true is semantically the empty schema, and false is a schema
        # position holding a non-object: MFJS rejects both
        # (MissingType / InvalidSchemaNode), fail closed.
        offenders.append(f"{path}: boolean schema (true/false) is semantically the empty schema; MFJS has no representation")
        return
    if not isinstance(node, dict):
        # A list (or scalar) at a schema position is InvalidSchemaNode
        # upstream — e.g. {"items": [[]]} or a list-valued property — the
        # same whole-tool drop as every other deviation.
        offenders.append(f"{path}: schema position holds {type(node).__name__}; MFJS requires an object")
        return
    if "$ref" in node or "$dynamicRef" in node or "$recursiveRef" in node:
        # This guard never inspects a ref target (bundled servers are small
        # hand-written schemas — inline it); not following refs keeps the
        # incident class from hiding behind an indirection, fail closed.
        # $dynamicRef/$recursiveRef are the same indirection class.
        offenders.append(f"{path}: $ref/$dynamicRef/$recursiveRef is not followed by this guard (target uninspected); inline the schema")
        return
    if not node:
        offenders.append(f"{path}: empty schema node (MFJS rejects it and silently drops the tool)")
        return
    if structural_depth > _MFJS_MAX_STRUCTURAL_DEPTH:
        offenders.append(
            f"{path}: structural depth {structural_depth} exceeds the MFJS budget of {_MFJS_MAX_STRUCTURAL_DEPTH} (ResourceLimitExceeded drops the tool)")
    if is_root and (node.get("type") != "object" or "anyOf" in node):
        # validate_mfjs_parameters (RootMustBeObject): a function parameters
        # document must be a typed object without composition; the
        # constrained route drops the whole tool otherwise.
        offenders.append(
            f"{path}: root must be type \"object\" without anyOf (MFJS RootMustBeObject drops the tool)")
    node_type = node.get("type")
    if isinstance(node_type, list):
        # Legal JSON Schema, but the engine sanitizer reads string types
        # only, so the node arrives typeless downstream and MFJS drops the
        # tool (#683 class). Fail closed like the other unverified shapes
        # instead of interpreting the list.
        offenders.append(
            f"{path}: list-typed \"type\" ({node_type!r}) is outside the verified MFJS shape set (type must be a single string)"
        )
    elif node_type is not None and node_type not in _MFJS_CONCRETE_TYPES:
        # {"type": 1234} and {"type": "Array"} are the same MissingType
        # drop as an absent type: the sanitizer reads string types only.
        offenders.append(
            f"{path}: \"type\" ({node_type!r}) is not a concrete MFJS string type"
        )
    unsupported = sorted(set(node) - _MFJS_ALLOWED_KEYWORDS)
    if unsupported:
        # Verified against MFJS ALLOWED_KEYWORDS: anything outside the set
        # is UnsupportedKeyword — the validator discards the whole schema,
        # so there is nothing to recurse into that could still matter.
        offenders.append(
            f"{path}: unsupported keyword(s) {unsupported} — outside the verified MFJS shape set, the validator drops the whole tool"
        )
    for annotation in ("title", "description"):
        if annotation in node and not isinstance(node[annotation], str):
            offenders.append(
                f"{path}: {annotation} must be a string (MFJS InvalidKeywordValue drops the tool)")
    if "$id" in node and not isinstance(node["$id"], str):
        offenders.append(f"{path}: $id must be a string (MFJS InvalidKeywordValue drops the tool)")
    if node_type is None and "anyOf" not in node:
        # Non-empty and typeless: annotation-only nodes ({"description":
        # ...}, {"enum": [...]}) and typeless containers are MissingType
        # for MFJS — the silent tool-drop class.
        offenders.append(
            f"{path}: no \"type\" and no anyOf — MFJS rejects typeless nodes with MissingType"
        )
    if "anyOf" in node:
        # MFJS allows only title/description beside anyOf ($defs/$id at the
        # root) and a non-empty branch list within the union budget;
        # everything else is InvalidKeywordValue / ResourceLimitExceeded.
        allowed_siblings = {"anyOf", "title", "description"} | ({"$defs", "$id"} if is_root else set())
        bad_siblings = sorted(set(node) - allowed_siblings)
        if bad_siblings:
            offenders.append(
                f"{path}: anyOf sibling keyword(s) {bad_siblings} — MFJS allows only title/description beside anyOf"
            )
        branches = node.get("anyOf")
        if not isinstance(branches, list) or not branches:
            offenders.append(f"{path}: anyOf must be a non-empty branch list (MFJS rejects other shapes)")
        elif len(branches) > _MFJS_MAX_ANY_OF_ITEMS:
            offenders.append(
                f"{path}: anyOf has {len(branches)} branches — MFJS allows at most {_MFJS_MAX_ANY_OF_ITEMS} (ResourceLimitExceeded drops the tool)")
        for index, sub in enumerate(branches if isinstance(branches, list) else []):
            _walk_tool_schema(sub, f"{path}.anyOf[{index}]", offenders,
                              property_depth=property_depth,
                              structural_depth=structural_depth + 1, budget=budget)
    else:
        if not is_root and ("$defs" in node or "$id" in node):
            offenders.append(
                f"{path}: $defs/$id outside the schema root — MFJS rejects them (InvalidKeywordValue tool drop)"
            )
        if node_type in _MFJS_CONCRETE_TYPES:
            # These per-node rules run in the validator only after the type
            # is established (everything past the MissingType raise), so
            # nodes already flagged typeless or non-concrete get no second
            # flag here.
            allowed = {"type", "title", "description"} | set(_MFJS_KEYWORDS_FOR_TYPE[node_type])
            if is_root:
                allowed |= {"$defs", "$id"}
            stray = sorted(set(node) - allowed)
            if stray:
                offenders.append(
                    f"{path}: keyword(s) {stray} not permitted on type \"{node_type}\" — MFJS InvalidKeywordValue drops the tool")
            if "enum" in node and "enum" in _MFJS_KEYWORDS_FOR_TYPE[node_type]:
                values = node["enum"]
                if not isinstance(values, list) or not values:
                    offenders.append(
                        f"{path}: enum must be a non-empty array (MFJS InvalidKeywordValue drops the tool)")
                else:
                    if not _MFJS_ENUM_VALUE_MATCHERS[node_type](values):
                        offenders.append(
                            f"{path}: enum values do not all match type \"{node_type}\" (MFJS InvalidKeywordValue drops the tool)")
                    if budget is not None:
                        budget.total_enum_values += len(values)
                        if (budget.total_enum_values > _MFJS_MAX_TOTAL_ENUM_VALUES
                                and not budget.enum_budget_reported):
                            budget.enum_budget_reported = True
                            offenders.append(
                                f"{path}: {budget.total_enum_values} total enum values exceed the MFJS budget of {_MFJS_MAX_TOTAL_ENUM_VALUES} (ResourceLimitExceeded drops the tool)")
                    # validate_mfjs_enum's conditional literal budget: once an
                    # enum holds more than 250 entries its combined literal
                    # length must stay within 7500 bytes (str::len for the
                    # string case, serde to_string() for numeric literals) or
                    # the validator raises ResourceLimitExceeded.
                    if len(values) > _MFJS_ENUM_LENGTH_CHECK_THRESHOLD:
                        encoded_length = sum(
                            len(value.encode("utf-8")) if isinstance(value, str)
                            else len(json.dumps(
                                value, separators=(",", ":")).encode("utf-8"))
                            for value in values)
                        if encoded_length > _MFJS_MAX_ENUM_STRING_LENGTH:
                            offenders.append(
                                f"{path}: enum of {len(values)} values carries {encoded_length} literal bytes — MFJS caps enums above {_MFJS_ENUM_LENGTH_CHECK_THRESHOLD} entries at {_MFJS_MAX_ENUM_STRING_LENGTH} (ResourceLimitExceeded drops the tool)")
            if ("default" in node and node_type in _MFJS_DEFAULT_VALUE_MATCHERS
                    and not _MFJS_DEFAULT_VALUE_MATCHERS[node_type](node["default"])):
                # validate_mfjs_default: object/array nodes cannot carry a
                # default at all (flagged by the per-type rule above).
                offenders.append(
                    f"{path}: default does not match type \"{node_type}\" (MFJS InvalidDefault drops the tool)")
            for lo_kw, hi_kw in (("minLength", "maxLength"), ("minItems", "maxItems"),
                                 ("minimum", "maximum")):
                # Presence checks, mirroring the validator's obj.get(k).is_some()
                # arms: an explicitly present null keyword value is
                # present-and-invalid (validate_mfjs_bounds parses it and
                # raises InvalidRange), never "keyword absent" — .get() alone
                # cannot tell them apart because JSON null is Python None.
                lo_present, hi_present = lo_kw in node, hi_kw in node
                if not lo_present and not hi_present:
                    continue
                lo, hi = node.get(lo_kw), node.get(hi_kw)
                if lo_kw == "minLength":
                    expected_type, literal_ok = "string", _mfjs_is_u64
                elif lo_kw == "minItems":
                    expected_type, literal_ok = "array", _mfjs_is_u64
                else:
                    expected_type = "number/integer"
                    literal_ok = (_mfjs_is_integer_value if node_type == "integer"
                                  else _mfjs_is_number)
                type_ok = (node_type in ("number", "integer")
                           if expected_type == "number/integer"
                           else node_type == expected_type)
                if not type_ok:
                    offenders.append(
                        f"{path}: {lo_kw}/{hi_kw} require type \"{expected_type}\" (MFJS InvalidRange drops the tool)")
                elif not ((not lo_present or literal_ok(lo))
                          and (not hi_present or literal_ok(hi))):
                    offenders.append(
                        f"{path}: {lo_kw}/{hi_kw} hold an out-of-range literal (MFJS InvalidRange drops the tool)")
                elif lo_present and hi_present and lo > hi:
                    offenders.append(
                        f"{path}: {lo_kw} must not exceed {hi_kw} (MFJS InvalidRange drops the tool)")
            if node_type == "object":
                properties = node.get("properties")
                if "properties" in node and not isinstance(properties, dict):
                    offenders.append(
                        f"{path}: properties must be an object (MFJS InvalidKeywordValue drops the tool)")
                additional = node.get("additionalProperties")
                if "additionalProperties" in node and not isinstance(additional, (bool, dict)):
                    offenders.append(
                        f"{path}: additionalProperties must be a boolean or a schema object (MFJS InvalidKeywordValue drops the tool)")
                required = node.get("required")
                if "required" in node:
                    # validate_mfjs_required order: the required value must be
                    # a list before the properties object it names is consulted.
                    if not isinstance(required, list):
                        offenders.append(
                            f"{path}: required must be a list of non-empty property names (MFJS InvalidKeywordValue drops the tool)")
                    elif not isinstance(properties, dict):
                        offenders.append(
                            f"{path}: required requires a properties object (MFJS InvalidKeywordValue drops the tool)")
                    elif (not all(isinstance(entry, str) and entry for entry in required)
                            or len(required) != len(set(required))
                            or any(entry not in properties for entry in required)):
                        offenders.append(
                            f"{path}: required must name unique declared properties (MFJS InvalidKeywordValue drops the tool)")
    if node_type == "array":
        items = node.get("items")
        if isinstance(items, list):
            # Tuple form is legal JSON Schema but outside the MFJS shape set
            # verified in this repo (items must be a single schema); banned
            # uniformly, fail closed.
            offenders.append(f"{path}: tuple-form items is outside the verified MFJS shape set (items must be a single schema)")
        elif not _schema_has_concrete_type(items):
            offenders.append(
                f"{path}: array without concretely typed items (constrained decoding channels complete it to [])"
            )
    subs = node.get("properties")
    if isinstance(subs, dict):
        if budget is not None:
            budget.total_properties += len(subs)
            if (budget.total_properties > _MFJS_MAX_TOTAL_PROPERTIES
                    and not budget.properties_budget_reported):
                budget.properties_budget_reported = True
                offenders.append(
                    f"{path}: {budget.total_properties} total properties exceed the MFJS budget of {_MFJS_MAX_TOTAL_PROPERTIES} (ResourceLimitExceeded drops the tool)")
        for name, sub in subs.items():
            if not name or name in _MFJS_RESERVED_PROPERTY_NAMES:
                offenders.append(
                    f"{path}.properties.{name!r}: property name is empty or reserved (MFJS InvalidKeywordValue drops the tool)")
            if property_depth + 1 > _MFJS_MAX_OBJECT_DEPTH:
                offenders.append(
                    f"{path}.properties.{name}: property nesting deeper than {_MFJS_MAX_OBJECT_DEPTH} (MFJS ResourceLimitExceeded drops the tool)")
            _walk_tool_schema(sub, f"{path}.properties.{name}", offenders,
                              property_depth=property_depth + 1,
                              structural_depth=structural_depth + 1, budget=budget)
    if isinstance(node.get("additionalProperties"), dict):
        _walk_tool_schema(node["additionalProperties"], f"{path}.additionalProperties", offenders,
                          property_depth=property_depth,
                          structural_depth=structural_depth + 1, budget=budget)
    items = node.get("items")
    if isinstance(items, list):
        for index, sub in enumerate(items):
            _walk_tool_schema(sub, f"{path}.items[{index}]", offenders,
                              property_depth=property_depth,
                              structural_depth=structural_depth + 1, budget=budget)
    elif isinstance(items, dict):
        _walk_tool_schema(items, f"{path}.items", offenders,
                          property_depth=property_depth,
                          structural_depth=structural_depth + 1, budget=budget)
    if isinstance(node.get("$defs"), dict):
        for name, sub in node["$defs"].items():
            if not name or "/" in name:
                offenders.append(
                    f"{path}.$defs.{name!r}: definition name must be non-empty and \"/\"-free (MFJS InvalidKeywordValue drops the tool)")
            # Definitions validate with the property depth reset to 0, like
            # the validator's $defs arm.
            _walk_tool_schema(sub, f"{path}.$defs.{name}", offenders,
                              property_depth=0,
                              structural_depth=structural_depth + 1, budget=budget)
    elif is_root and "$defs" in node:
        offenders.append(f"{path}: $defs must be an object (MFJS InvalidKeywordValue drops the tool)")


def check_schema_contract(tool_id: str, tools: list) -> int:
    """Guard every tool's inputSchema; return the number of tools walked.

    The return value is printed into check_protocol's success line: delete
    the call site and that print immediately raises NameError — without it
    the guard's wiring could be silently dismantled while the whole CI
    matrix stays green (this file's unit tests can only pin the walker's
    internals, not the call site itself). The walk mirrors the validator's
    whole rule set — shape, per-type keyword admissibility, enum/default
    value typing, bounds, required⊆properties, and the resource budgets —
    so a schema this guard accepts is not one the validator would drop.
    """
    offenders: list[str] = []
    for tool in tools:
        schema = tool.get("inputSchema")
        name = tool.get("name", "?")
        if not isinstance(schema, dict):
            offenders.append(f"{tool_id}.{name}: inputSchema missing or not an object")
            continue
        budget = _MfjsBudget(schema)
        if budget.encoded_bytes > _MFJS_MAX_SCHEMA_BYTES:
            offenders.append(
                f"{tool_id}.{name}: inputSchema encodes to {budget.encoded_bytes} bytes — over the MFJS budget of {_MFJS_MAX_SCHEMA_BYTES} (ResourceLimitExceeded drops the tool)")
        _walk_tool_schema(schema, f"{tool_id}.{name}", offenders, is_root=True, budget=budget)
    assert not offenders, (
        "Model-visible schema violates the contract:\n  " + "\n  ".join(offenders)
    )
    return len(tools)


def check_protocol(tool_id: str, expected_tools: set[str], env=None):
    with RpcServer(MCP_ROOT / tool_id, env) as rpc:
        initialized = rpc.call("initialize")
        assert initialized["result"]["protocolVersion"] == "2024-11-05"
        listed = rpc.call("tools/list")
        actual = {tool["name"] for tool in listed["result"]["tools"]}
        assert actual == expected_tools, (tool_id, actual)
        walked = check_schema_contract(tool_id, listed["result"]["tools"])
        # Self-pinning success line: the printed count must equal the boot
        # contract's tool set, so a guard that silently walks nothing (or a
        # rewired print) cannot report a compliance it never checked.
        assert walked == len(expected_tools), (tool_id, walked, len(expected_tools))
    print(f"✅ {tool_id}: initialize + tools/list ({len(expected_tools)}) + schema guard ({walked} inputSchemas walked)")


def check_session_reader_length_cap_parity() -> None:
    """Round-9 minor 1: the host demotes at parent.len() > 128 citing the
    server's MAX_SESSION_ID_LEN — pin the constant itself so a silent
    server-side lowering re-opens the dead-tool-declaration scenario the
    host demotion exists to prevent."""
    source = (MCP_ROOT / "session-reader" / "server.py").read_text(
        encoding="utf-8")
    match = re.search(r"MAX_SESSION_ID_LEN\s*=\s*(\d+)", source)
    assert match, "MAX_SESSION_ID_LEN must stay declared in server.py"
    assert int(match.group(1)) == 128, (
        "server MAX_SESSION_ID_LEN moved: update engine_pool.rs's host-side "
        "length demote (parent.len() > 128) in lockstep")


def check_session_reader_scoped_journey() -> None:
    """ADR-0024 scoped instance (--only-session): the aux engine's shape."""
    with tempfile.TemporaryDirectory(prefix="pinvou3-scoped-smoke-") as sessions:
        def write_session(sid: str, user: str, answer: str) -> None:
            (Path(sessions) / f"{sid}.json").write_text(
                json.dumps({
                    "schema_version": 1,
                    "metadata": {
                        "id": sid, "title": f"session {sid}",
                        "created_at": "2026-10-08T00:00:00Z",
                        "updated_at": "2026-10-08T00:00:00Z",
                        "message_count": 2,
                    },
                    "messages": [
                        {"role": "user", "content": [{"type": "text", "text": user}]},
                        {"role": "assistant", "content": [{"type": "text", "text": answer}]},
                    ],
                }, ensure_ascii=False),
                encoding="utf-8",
            )

        write_session("parent01", "父任务的问题", "父任务的回答")
        write_session("sibling02", "别的会话", "不该被读到")

        with RpcServer(
            MCP_ROOT / "session-reader",
            # The shipped per-aux config emits the = form (dash-immune);
            # the wire journey must drive the shipped form, not the
            # two-token spelling the code abandoned.
            args=["--sessions-dir", sessions, "--only-session=parent01"],
        ) as rpc:
            # Scoped mode lists the read-only surface: #627's cross-session
            # send cannot be data-scoped the way reads are, so the scoped
            # listing and the executable surface stay in lockstep.
            listed = rpc.call("tools/list")
            assert {tool["name"] for tool in listed["result"]["tools"]} == {
                "read_session", "list_sessions"}

            ok = content_json(rpc.call("tools/call", {
                "name": "read_session",
                "arguments": {"session_id": "parent01"},
            }))
            assert ok["ok"] and ok["turns"][0]["userText"] == "父任务的问题"

            denied = content_json(rpc.call("tools/call", {
                "name": "read_session",
                "arguments": {"session_id": "sibling02"},
            }))
            assert not denied["ok"] and "not readable" in denied["error"]

            scoped_list = content_json(rpc.call("tools/call", {
                "name": "list_sessions", "arguments": {},
            }))
            assert [entry["sessionId"] for entry in scoped_list["sessions"]] == ["parent01"]

            # The #627 send tool rides the unscoped app instance only: a
            # scoped instance refuses it structurally (never a spool write).
            refused = content_json(rpc.call("tools/call", {
                "name": "send_message_to_session",
                "arguments": {
                    "session_id": "parent01",
                    "title": "must never deliver",
                    "message": "scoped instances are read-only",
                },
            }))
            assert not refused["ok"] and "parent-context reads" in refused["error"]

    # Startup validation: an isolated-prefix pin refuses to boot (the aux
    # spawn then fails its MCP boot and carries no session-reader tools —
    # fail-closed).
    proc = subprocess.Popen(
        [sys.executable, "server.py", "--only-session", "aux-parent01"],
        cwd=MCP_ROOT / "session-reader",
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, encoding="utf-8",
    )
    _, stderr = proc.communicate(timeout=10)
    assert proc.returncode == 2, "an isolated-prefix pin must refuse to start with exit 2"
    assert "--only-session" in stderr and "not readable" in stderr, stderr
    print("✅ session-reader: scoped instance (--only-session) 全旅程 + 隔离前缀拒启")


def check_rpc_timeout() -> None:
    with tempfile.TemporaryDirectory(prefix="pinvou-mcp-hung-") as tmp:
        server_dir = Path(tmp)
        (server_dir / "server.py").write_text(
            "import sys, time\n"
            "for _line in sys.stdin:\n"
            "    print('hung server marker', file=sys.stderr, flush=True)\n"
            "    time.sleep(60)\n",
            encoding="utf-8",
        )
        try:
            with RpcServer(server_dir, timeout_s=0.2) as rpc:
                rpc.call("initialize")
        except AssertionError as exc:
            message = str(exc)
            assert "timed out" in message and "hung server marker" in message, message
        else:
            raise AssertionError("hung MCP server should time out")
    print("✅ stdio RPC: 卡住时按单次超时终止，并带 stderr 尾部诊断")


def main():
    check_rpc_timeout()
    manifests = {}
    for manifest_path in sorted(MCP_ROOT.glob("*/manifest.json")):
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        assert manifest["id"] == manifest_path.parent.name
        manifests[manifest["id"]] = manifest
    assert set(manifests) == {
        "weather",
        "iwencai",
        "qcc",
        "obsidian",
        "pptx",
        "gongwen",
        "yuandian-mcp",
        "canva-mcp",
        "patsnap-search",
        "wecom-bot",
        "tencent-docs",
        "session-reader",
        "app-automations",
    }
    print("✅ manifest: 13 个可安装 MCP 清单完整且目录 ID 一致")

    expected = {
        "weather": {"get_weather"},
        "iwencai": {
            "hithink_market_query", "hithink_finance_query", "hithink_event_query",
            "hithink_management_query", "hithink_business_query", "hithink_industry_query",
            "hithink_zhishu_query", "hithink_sector_selector", "hithink_astock_selector",
            "hithink_macro_query", "news_search", "announcement_search",
        },
        "obsidian": {"search", "read_note", "list", "create_note", "edit_note", "rename_note", "delete_note"},
        "pptx": {"make_pptx"},
        "gongwen": {"make_gongwen"},
        "wecom-bot": {"send_text", "send_markdown", "send_news", "send_image", "send_file"},
        "session-reader": {"read_session", "list_sessions", "send_message_to_session"},
        "app-automations": {
            "create_scheduled_task", "read_scheduled_task", "list_scheduled_tasks",
            "update_scheduled_task", "delete_scheduled_task",
        },
    }
    for tool_id, names in expected.items():
        check_protocol(tool_id, names)

    # E6 (ADR-0024): the scoped-mode behavior addition ships as manifest
    # 1.1.0 — the builtin package re-releases by byte comparison
    # (ensure_package_released upgrades an existing install on mismatch).
    assert manifests["session-reader"]["version"] == "1.1.0", manifests["session-reader"]["version"]
    check_session_reader_length_cap_parity()
    check_session_reader_scoped_journey()

    with RpcServer(MCP_ROOT / "weather", {"AMAP_KEY": ""}) as rpc:
        result = content_json(rpc.call("tools/call", {"name": "get_weather", "arguments": {"city": "杭州"}}))
        assert result == {"error": "AMAP_KEY 未配置"}
    print("✅ weather: 未配置凭据返回稳定错误，不发起外网请求")

    with RpcServer(MCP_ROOT / "wecom-bot", {"WECOM_BOT_KEY": ""}) as rpc:
        result = content_json(rpc.call("tools/call", {"name": "send_text", "arguments": {"content": "测试"}}))
        assert result == {"error": "WECOM_BOT_KEY 未配置，请在工具详情里填写群机器人 webhook key"}
    print("✅ wecom-bot: 未配置凭据返回稳定错误，不发起外网请求")

    with RpcServer(MCP_ROOT / "iwencai", {"IWENCAI_API_KEY": ""}) as rpc:
        response = rpc.call("tools/call", {"name": "news_search", "arguments": {"query": "黄金价格"}})
        assert response["error"]["code"] == -32000
        assert "IWENCAI_API_KEY" in response["error"]["message"]
    print("✅ iwencai: 真实 tools/call 缺凭据错误契约")

    with tempfile.TemporaryDirectory(prefix="pinvou-obsidian-") as vault:
        env = {"OBSIDIAN_VAULT_PATH": vault}
        with RpcServer(MCP_ROOT / "obsidian", env) as rpc:
            made = content_json(rpc.call("tools/call", {"name": "create_note", "arguments": {"path": "测试/自动化", "content": "# 自动化\n工具商店契约"}}))
            assert made.get("type") == "obsidian_created" and Path(vault, "测试", "自动化.md").is_file()
            read = content_json(rpc.call("tools/call", {"name": "read_note", "arguments": {"path": "测试/自动化"}}))
            assert "工具商店契约" in read.get("content", "")
            found = content_json(rpc.call("tools/call", {"name": "search", "arguments": {"query": "工具商店"}}))
            assert found.get("count") == 1
            preview = content_json(rpc.call("tools/call", {"name": "delete_note", "arguments": {"path": "测试/自动化"}}))
            assert preview.get("type") == "confirm_required" and Path(vault, "测试", "自动化.md").exists()
            deleted = content_json(rpc.call("tools/call", {"name": "delete_note", "arguments": {"path": "测试/自动化", "confirm": True}}))
            assert deleted.get("type") == "obsidian_deleted" and not Path(vault, "测试", "自动化.md").exists()
    print("✅ obsidian: 创建/读取/搜索/人在环删除全旅程")

    with tempfile.TemporaryDirectory(prefix="pinvou-session-reader-send-") as home:
        sessions = Path(home, "sessions")
        sessions.mkdir(parents=True)
        for sid, title in (("src0001", "源会话"), ("tgt0001", "目标会话")):
            (sessions / f"{sid}.json").write_text(json.dumps({
                "metadata": {"id": sid, "title": title, "updated_at": "2026-09-28T00:00:00Z", "message_count": 1},
                "messages": [],
            }, ensure_ascii=False), encoding="utf-8")
        with RpcServer(MCP_ROOT / "session-reader", {"PINVOU3_HOME": home}) as rpc:
            sent = content_json(rpc.call("tools/call", {
                "name": "send_message_to_session",
                "arguments": {"to_session": "tgt0001", "text": "跨会话交接", "from_session": "src0001", "idempotency_key": "k1"},
            }))
            assert sent.get("delivery") == "pending" and sent.get("toSession") == "tgt0001", sent
            duplicate = content_json(rpc.call("tools/call", {
                "name": "send_message_to_session",
                "arguments": {"to_session": "tgt0001", "text": "跨会话交接", "from_session": "src0001", "idempotency_key": "k1"},
            }))
            assert duplicate.get("duplicate") is True, duplicate
            spooled = sorted(Path(home, "messaging", "spool").glob("*.json"))
            assert len(spooled) == 1, spooled
            record = json.loads(spooled[0].read_text(encoding="utf-8"))
            assert record["to_session"] == "tgt0001" and record["from_session"] == "src0001", record
            assert record["from_title"] == "源会话" and record["text"] == "跨会话交接", record
            isolated = content_json(rpc.call("tools/call", {
                "name": "send_message_to_session",
                "arguments": {"to_session": "sched-run1", "text": "hi"},
            }))
            assert "not readable" in isolated.get("error", ""), isolated
    print("✅ session-reader: 跨会话消息校验/幂等/隔离前缀全旅程")

    with tempfile.TemporaryDirectory(prefix="pinvou-app-automations-") as home:
        # A plain session file for the scheduled-message target check.
        Path(home, "sessions").mkdir(parents=True)
        Path(home, "sessions", "tgt0001.json").write_text(
            json.dumps({"metadata": {"id": "tgt0001", "title": "目标"}, "messages": []}),
            encoding="utf-8",
        )
        with RpcServer(MCP_ROOT / "app-automations", {"PINVOU3_HOME": home}) as rpc:
            # CRON/分钟级是产品子集外的硬拒（B2/B3）。
            cron = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task",
                "arguments": {"name": "x", "prompt": "y", "rrule": "FREQ=CRON;EXPR=*/5 * * * *"},
            }))
            assert "CRON" in cron.get("error", ""), cron
            # 无 watcher：短等待超时回 pending（A3），spool 留存（C3 重启恢复前置）。
            pending = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task",
                "arguments": {
                    "name": "早报", "prompt": "汇总新闻",
                    "rrule": "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
                    "from_session": "src0001",
                    "idempotency_key": "k1",
                },
            }))
            assert pending.get("ok") is True and pending.get("taskId") is None, pending
            assert pending.get("delivery") == "pending", pending
            spooled = sorted(Path(home, "task-requests", "spool").glob("*.json"))
            assert len(spooled) == 1, spooled
            record = json.loads(spooled[0].read_text(encoding="utf-8"))
            assert record["rrule"].startswith("FREQ=WEEKLY;") and record["schema_version"] == 1, record
            # 同幂等键重试：同一 spool 文件、duplicate 标记（C1）。
            duplicate = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task",
                "arguments": {
                    "name": "早报", "prompt": "汇总新闻",
                    "rrule": "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
                    "from_session": "src0001",
                    "idempotency_key": "k1",
                },
            }))
            assert duplicate.get("duplicate") is True, duplicate
            spooled = sorted(Path(home, "task-requests", "spool").glob("*.json"))
            assert len(spooled) == 1, spooled
            # 模拟应用侧 watcher（features/scheduled/creation_requests）：持续排水，
            # 逐条写结果标记并删 spool——积压的 k1 请求与在途请求都拿到同步回执
            # （A2 短等待同步返回 + C3 积压恢复）。
            import threading as _threading
            import importlib.util as _importlib_util

            # Round-24 minor 8: production markers carry the canonical
            # request digest — a digest-less marker is a shape production
            # never produces. The simulated watcher computes it with the
            # server module's own function so the smoke cannot drift from it.
            _server_spec = _importlib_util.spec_from_file_location(
                "pinvou_app_automations_server_for_smoke",
                MCP_ROOT / "app-automations" / "server.py",
            )
            _server_module = _importlib_util.module_from_spec(_server_spec)
            _server_spec.loader.exec_module(_server_module)

            stop = _threading.Event()

            def watcher():
                while not stop.is_set():
                    for path in sorted(Path(home, "task-requests", "spool").glob("*.json")):
                        try:
                            request = json.loads(path.read_text(encoding="utf-8"))
                        except (OSError, ValueError):
                            continue
                        spool_id = path.stem
                        done_dir = Path(home, "task-requests", "spool", ".done")
                        done_dir.mkdir(parents=True, exist_ok=True)
                        (done_dir / f"{spool_id}.json").write_text(json.dumps({
                            "ok": True,
                            "task_id": "sim-%s" % spool_id[:8],
                            "task_name": request.get("name"),
                            # Round-26 M6(c): the Rust watcher coerces a
                            # create's paused null to false BEFORE digesting
                            # (unwrap_or(false)) — digesting the raw spool
                            # value here made the smoke marker's digest
                            # diverge from production for every default
                            # create, so an identical keyed replay
                            # false-flagged payload_mismatch below.
                            "request_digest": _server_module.spool_request_digest(
                                request.get("kind"),
                                task_id=request.get("task_id"),
                                name=request.get("name"),
                                prompt=request.get("prompt"),
                                rrule=request.get("rrule"),
                                model_id=request.get("model_id"),
                                paused=(
                                    False
                                    if request.get("kind") == "create"
                                    and request.get("paused") is None
                                    else request.get("paused")
                                ),
                            ),
                        }, ensure_ascii=False), encoding="utf-8")
                        path.unlink()
                    time.sleep(0.05)

            watcher_thread = _threading.Thread(target=watcher, daemon=True)
            watcher_thread.start()
            confirmed = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task",
                "arguments": {
                    "name": "晚报", "prompt": "汇总新闻",
                    "rrule": "FREQ=HOURLY;INTERVAL=6;BYHOUR=20;BYMINUTE=30",
                },
            }))
            assert confirmed.get("ok") is True, confirmed
            assert confirmed.get("taskId", "").startswith("sim-"), confirmed
            assert confirmed.get("taskName") == "晚报", confirmed
            assert confirmed.get("duplicate") is False, confirmed
            stop.set()
            # Round-15 minor 4: the watcher must be GONE before the later
            # journeys run — a thread descheduled between its stop-check and
            # the spool scan could otherwise drain timeout2's record after
            # stop.set() and flip the pending/target assertions below. The
            # bounded join makes its lifetime deterministic.
            watcher_thread.join(timeout=2)
            assert not watcher_thread.is_alive(), "fake watcher must finish before inspecting the spool"
            # 积压与在途都被排水，spool 清空、.done 留有两条标记。
            assert not list(Path(home, "task-requests", "spool").glob("*.json"))
            assert len(list(Path(home, "task-requests", "spool", ".done").glob("*.json"))) == 2
            # Round-26 M6(c): a keyed replay against a WATCHER-DRAINED key
            # must answer the recorded duplicate with NO payload_mismatch —
            # the marker's digest is the one production's watcher would
            # write, so the server's coerced replay digest matches it.
            replay = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task",
                "arguments": {
                    "name": "早报", "prompt": "汇总新闻",
                    "rrule": "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
                    "from_session": "src0001",
                    "idempotency_key": "k1",
                },
            }))
            assert replay.get("ok") is True and replay.get("taskId", "").startswith("sim-"), replay
            assert replay.get("duplicate") is True, replay
            assert replay.get("payload_mismatch") is False, replay
            # Scheduled-message mode: isolated targets are hard-rejected
            # (self-wake = the recursion direction); a valid target carries
            # target_session into the spool record.
            isolated = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task",
                "arguments": {
                    "name": "自唤醒", "prompt": "x", "rrule": "FREQ=ONCE;AT=2099-06-01T09:30",
                    "target_session": "sched-run1",
                },
            }))
            assert "isolated" in isolated.get("error", ""), isolated
            timeout2 = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task",
                "arguments": {
                    "name": "定时询问", "prompt": "问一下进展",
                    "rrule": "FREQ=HOURLY;INTERVAL=6",
                    "target_session": "tgt0001",
                },
            }))
            assert timeout2.get("delivery") == "pending", timeout2
            spooled = sorted(Path(home, "task-requests", "spool").glob("*.json"))
            targets = []
            for path in spooled:
                record = json.loads(path.read_text(encoding="utf-8"))
                if record.get("target_session"):
                    targets.append(record["target_session"])
            assert targets == ["tgt0001"], (spooled, targets)
            # list：只投影安全字段，绝不带 prompt（§3.1）；watcher 未真建任务，store 为空。
            listed = content_json(rpc.call("tools/call", {"name": "list_scheduled_tasks"}))
            assert listed.get("ok") is True and listed.get("total") == 0, listed
            # 读详情：未知 id 显式 not found（I3）；update/delete 校验路径（I4/I5）。
            missing = content_json(rpc.call("tools/call", {
                "name": "read_scheduled_task", "arguments": {"task_id": "nosuch"},
            }))
            assert "not found" in missing.get("error", ""), missing
            no_fields = content_json(rpc.call("tools/call", {
                "name": "update_scheduled_task", "arguments": {"task_id": "nosuch"},
            }))
            assert "at least one field" in no_fields.get("error", ""), no_fields
            bad_target = content_json(rpc.call("tools/call", {
                "name": "update_scheduled_task",
                "arguments": {"task_id": "nosuch", "name": "x"},
            }))
            assert "not found" in bad_target.get("error", ""), bad_target
            no_target = content_json(rpc.call("tools/call", {
                "name": "delete_scheduled_task", "arguments": {},
            }))
            assert "invalid task_id" in no_target.get("error", ""), no_target
            extra = content_json(rpc.call("tools/call", {
                "name": "delete_scheduled_task",
                "arguments": {"task_id": "nosuch", "name": "x"},
            }))
            assert "no extra fields" in extra.get("error", ""), extra
    print("✅ app-automations: rrule 子集硬拒/幂等/短等待同步返回/CRUD 校验全旅程")

    with tempfile.TemporaryDirectory(prefix="pinvou-artifacts-") as artifacts:
        env = {"PINVOU3_SESSION_ARTIFACTS": artifacts}
        with RpcServer(MCP_ROOT / "pptx", env) as rpc:
            result = content_json(rpc.call("tools/call", {"name": "make_pptx", "arguments": {
                "filename": "自动化测试", "theme": "business-blue", "slides": [
                    {"layout": "cover", "title": "自动化测试", "subtitle": "工具商店"},
                    {"layout": "bullets", "title": "结论", "bullets": ["安装可用", "产物可编辑"]},
                ]
            }}))
            pptx_path = Path(result["path"])
            assert result.get("ok") is True and result.get("slides") == 2
            assert pptx_path.parent == Path(artifacts) and zipfile.is_zipfile(pptx_path)

        with RpcServer(MCP_ROOT / "gongwen", env) as rpc:
            result = content_json(rpc.call("tools/call", {"name": "make_gongwen", "arguments": {
                "filename": "自动化测试通知", "doc_type": "通知", "issuer": "测试中心",
                "title": "测试中心关于开展自动化测试的通知", "recipient": "各部门",
                "body": "为提升产品质量，现将有关事项通知如下。\n一、开展工具安装测试。\n二、验证产物可以正常打开。",
                "signer": "测试中心", "date": "2026年7月14日",
            }}))
            docx_path = Path(result["path"])
            assert result.get("ok") is True and result["validate"]["ok"] is True
            assert docx_path.parent == Path(artifacts) and zipfile.is_zipfile(docx_path)
    print("✅ pptx/gongwen: 真实 tools/call 生成可打开的 OOXML 产物")

    weather = manifests["weather"]
    assert weather["secret_env"] == [{"key": "AMAP_KEY", "provider": "amap", "required": True}]
    assert weather["config_fields"] == [{
        "key": "AMAP_KEY",
        "label": "高德 Web 服务 API Key",
        "required": True,
        "target": "env",
        "secret": True,
    }]
    print("✅ weather: 用户自填高德 Web 服务 Key 清单契约")

    iwencai = manifests["iwencai"]
    assert iwencai["secret_env"] == [{"key": "IWENCAI_API_KEY", "provider": "iwencai", "required": True}]
    assert iwencai["config_fields"] == [{
        "key": "IWENCAI_API_KEY",
        "label": "问财 API Key",
        "required": True,
        "target": "env",
        "secret": True,
    }]
    print("✅ iwencai: 用户自填问财 API Key 清单契约")

    wecom_bot = manifests["wecom-bot"]
    assert wecom_bot["secret_env"] == [{"key": "WECOM_BOT_KEY", "provider": "wecom-bot", "required": True}]
    assert wecom_bot["config_fields"] == [{
        "key": "WECOM_BOT_KEY",
        "label": "群机器人 Webhook Key",
        "required": True,
        "target": "env",
        "secret": True,
    }]
    print("✅ wecom-bot: 用户自填群机器人 Webhook Key 清单契约")

    qcc = manifests["qcc"]
    assert qcc.get("secret_headers") in (None, [])
    # Remote-only manifest: a command-bearing qcc manifest would silently
    # escape both the boot contract below and the schema guard, which only
    # sees booted stdio servers.
    assert not qcc.get("command")
    # 可选密钥契约：qcc 明文迁移把历史凭证存放在 Bearer target 下并接线
    # bearer_token_env_var，manifest 必须声明同一 key 才能被
    # sync_secret_values 的重启重水化枚举覆盖（缺声明 = 首次重启即 401）。
    # required=False 保证 OAuth-only 新装不弹必填、空输入照常安装。
    assert qcc["config_fields"] == [{
        "key": "QCC_API_KEY",
        "label": "企查查 API Key",
        "required": False,
        "target": "bearer",
        "secret": True,
    }]
    assert qcc["servers"] == [{
        "name": "qcc-company",
        "url": "https://agent.qcc.com/mcp/company/stream",
        "scopes": ["mcp:tools"],
        "oauth_resource": "https://agent.qcc.com/mcp/company/stream",
    }]
    print("✅ qcc: 唯一 qcc-company 远程端点 + OAuth scope/resource + 可选 API Key 清单契约")

    yuandian = manifests["yuandian-mcp"]
    assert yuandian["mcp_tools"] == [] and not yuandian["command"]
    assert yuandian["servers"] == [{
        "name": "yuandian_mcp",
        "url": "https://open.chineselaw.com/mcp",
        "scopes": ["mcp"],
        "oauth_resource": "https://open.chineselaw.com/mcp",
    }]
    print("✅ yuandian-mcp: 唯一远程端点 + OAuth scope/resource 清单契约")

    canva = manifests["canva-mcp"]
    assert canva["mcp_tools"] == [] and not canva["command"]
    assert canva["servers"] == [{
        "name": "canva_mcp",
        "url": "https://mcp.canva.cn/mcp",
        "scopes": [
            "profile:read",
            "design:meta:read",
            "design:content:write",
            "design:content:read",
            "folder:read",
            "folder:write",
            "brandtemplate:content:read",
            "brandtemplate:meta:read",
            "brandtemplate:content:write",
            "comment:write",
            "comment:read",
            "asset:read",
            "asset:write",
            "brandkit:read",
            "help:answers:read",
            "help:answers:write",
        ],
        "oauth_resource": "https://mcp.canva.cn/mcp",
    }]
    assert "validate_on_install" not in canva
    assert "secret_headers" not in canva
    assert "config_fields" not in canva
    print("✅ canva-mcp: 唯一远程端点 + OAuth scope/resource 清单契约")

    patsnap = manifests["patsnap-search"]
    assert patsnap["mcp_tools"] == ["patsnap_search", "patsnap_fetch"] and not patsnap["command"]
    assert patsnap["servers"] == [{
        "name": "patsnap-search",
        "url": "https://connect.zhihuiya.com/2b0355/logic-mcp",
    }]
    assert patsnap["validate_on_install"] is True
    assert patsnap["secret_headers"] == [{
        "header": "Authorization",
        "scheme": "Bearer",
        "source_key": "PATSNAP_API_KEY",
        "provider": "patsnap",
        "required": True,
    }]
    assert patsnap["config_fields"] == [{
        "key": "PATSNAP_API_KEY",
        "label": "智慧芽 API Key",
        "required": True,
        "target": "bearer",
        "secret": True,
    }]
    print("✅ patsnap-search: 唯一远程端点 + bearer secret + 安装校验契约")

    tdoc = manifests["tencent-docs"]
    assert tdoc["mcp_tools"] == [] and not tdoc["command"]
    assert tdoc["servers"] == [
        {"name": "tencent-docs", "url": "https://docs.qq.com/openapi/mcp"},
        {"name": "tdoc-slide", "url": "https://docs.qq.com/api/v6/slide/mcp"},
        {"name": "tdoc-doc", "url": "https://docs.qq.com/api/v6/doc/mcp"},
        {"name": "tdoc-sheet", "url": "https://docs.qq.com/api/v6/sheet/mcp"},
    ]
    assert tdoc["validate_on_install"] is True
    assert tdoc["secret_headers"] == [{
        "header": "Authorization",
        "scheme": "",
        "source_key": "TENCENT_DOCS_TOKEN",
        "provider": "tencent-docs",
        "required": True,
    }]
    # 不变量:Token 通道唯一。若再引入 config_fields target="bearer",安装时两通道会
    # 同写 Authorization(scheme 分歧→bearer_token_env_var 与 env_headers 并存),
    # 最终请求头是否带 Bearer 前缀将取决于底座合并顺序——禁止这种自相矛盾。
    assert tdoc.get("config_fields", []) == [], (
        "tencent-docs 的 Token 只经 secret_headers 声明,不得再加 config_fields"
    )
    print("✅ tencent-docs: 四官方远程端点 + 无 scheme 原始 Token 注入 + 安装校验契约")

    print("\n✅ ALL MCP SERVER CONTRACTS PASS")


if __name__ == "__main__":
    main()
