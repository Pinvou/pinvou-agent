#!/usr/bin/env python3
"""内置 MCP 契约矩阵：真实启动 stdio server，检查协议、工具调用与本地产物。"""
from __future__ import annotations

import copy
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
# Policy (ruling of 2026-11-05): the published Moonshot documents are the
# sole shipping basis for the constrained-decoding wire. A live probe may
# only ever tighten that contract, never widen it — the 2026-11-04 probe
# observations (a root anyOf union, pattern/format/$schema/prefixItems/
# exclusive bounds/nested allOf accepted with 200 on the coding endpoint)
# are recorded observations, never authority, and the per-route keyword
# tiers that briefly admitted them were removed on exactly that basis:
# every Moonshot route serves the same document and gets the same contract.
#
# The governing documents, fetched 2026-11-05:
#   - MFJS specification
#     (https://raw.githubusercontent.com/MoonshotAI/walle/main/docs/
#     mfjs-spec.zh.md): the 字段定义 sections enumerate exactly
#     description, default, anyOf, properties, additionalProperties, items,
#     type, enum, required — plus $defs/$ref under restricted support
#     ("$defs 必须定义在 schema 的根级别 (root level)；$ref 仅支持引用
#     本文档内部的 subschema"). Its closing clause is the authority for
#     every keyword it does not list: "若官方规范中的某些概念、关键字或
#     特性在本文档中未被提及，则表示当前 MFJS 尚未提供官方支持。虽然将这些
#     未列举的特性传入时可能不会被系统直接拦截，且可能被大模型观察到，但
#     我们不保证它们能在最终的结构化输出中产生稳定、符合预期的结果".
#   - Tool Use guide (https://platform.kimi.ai/docs/api/tool-use.md):
#     "The root of `parameters` must be an `object`", and `strict` defaults
#     to true ("true (default, omitting is equivalent to true)") — the
#     schema is enforced on the wire, not advisory.
#
# The guard mirrors the T11 engine pipeline (schema_sanitize.rs — the
# document-precise Moonshot face lands on the wire via CodeWhale #143, with
# the provider faces in #144/#145/#146/#147) against exactly that document
# set. Until the close-out re-pin lands that batch on this repository's
# gitlink, the guard pins the TARGET contract: the parent's pinned tree
# still runs the pre-T11 sanitizer, and the rules here converge with the
# wire at the re-pin, not independently:
#   - Advisory keywords are STRIPPED, not rejected. The specification's
#     unsupported list names the annotation group outright ("暂不支持
#     title、$comment 等描述性元数据，也不支持 format 格式注解字段");
#     title/$comment/$id and the bound keywords (minLength/maxLength/
#     minimum/maximum/minItems/maxItems) are unlisted, and the engine's
#     strip_mfjs_advisory_keywords removes them before validation because
#     removal is either semantics-lossless (annotations) or loses only soft
#     constraints (bounds) — the tool still ships and the Moonshot wire
#     carries only the documented set, while other providers keep receiving
#     the full schema. The mirror strips the same nine keywords first
#     (malformed values included: kimi_parameters_strips_bounds_instead_of_
#     rejecting_them), then walks what remains.
#   - Enums whose values are not homogeneous float/int/str scalars are
#     stripped the same way ("枚举值我们只支持基础类型中的 float, int 和
#     str，其它基础类型以及 object 和 array 暂不支持，并且不支持混合类型
#     (List 中的每个枚举值的类型必须相同，如不可以 str + int)"); a
#     surviving enum's values must still match its node's type. An EMPTY
#     enum (or a non-array one) is deliberately left in place by the strip
#     — the kind homogeneity check is vacuously true on an empty list —
#     and fails closed in the validator (InvalidKeywordValue): stripping it
#     would silently widen an unsatisfiable constraint into "anything
#     goes" (kimi_parameters_empty_enum_fails_closed_in_the_validator), so
#     the mirror keeps it too and the walk's own non-empty check flags it.
#   - The fail-closed set keeps the whole-tool drop: pattern, format,
#     $schema, prefixItems, exclusive bounds, and every other unlisted
#     keyword ("暂不支持使用 prefixItems 或 unevaluatedItems
#     等方式模拟实现元组（Tuple）", "暂不支持如 exclusiveMinimum，
#     exclusiveMaximum，minContains, maxContains 等复杂校验定义", "暂不支持
#     Meta-Schemas"), and a silent drop is the same failure for every
#     deviation (MissingType / UnsupportedKeyword / InvalidSchemaNode).
#     The MIRROR IS DELIBERATELY STRICTER THAN THE ENGINE on the union
#     keywords: the engine does have partial equivalents this guard refuses
#     to rely on — a single-branch nested allOf/oneOf collapses into its
#     parent (collapse_single_element_unions), a nested oneOf converts to
#     anyOf (normalize_kimi_compatibility), and root unions flatten
#     wholesale — so only a MULTI-branch nested allOf genuinely fails
#     closed engine-side (UnsupportedNestedAllOf). The mirror red-flags
#     every oneOf/allOf form because the engine's rewrites are invisible to
#     a guard that only sees the pre-sanitization schema; a red here is
#     conservative, not an engine-behavior claim. Scalar `const` is the
#     other deliberate-strictness case: the engine translates a scalar
#     const through the const→enum landing (type inference included) and
#     ships it, while an object/array const fails closed
#     (UnsupportedConstLiteral); the mirror reds every const because it
#     does not replicate that landing. #683's evaluate_script was lost to
#     the empty-`items` class (MissingType), which the T11 engine now
#     repairs (remove_empty_mfjs_items_schemas — flagged below, never
#     replicated). $ref/$dynamicRef/$recursiveRef stay banned
#     beyond the document's own $ref support because this guard cannot
#     follow the target and the shape underneath would go uninspected (the
#     engine inlines a root $ref before validation — same conservative
#     posture: the mirror flags what the engine rewrites).
#   - Every schema position holds an object; boolean and list values are
#     InvalidSchemaNode, empty nodes are MissingType. THE ENGINE REPAIRS
#     MORE THAN items:{} — this mirror deliberately flags the whole repair
#     class instead of replicating it: remove_empty_mfjs_items_schemas
#     removes an items:{} keyword entirely (JSON Schema's "items
#     unconstrained"), ensure_properties_object replaces a non-object
#     properties with {} at the root, prune_dangling_required strips
#     required entries that name no declared property (and drops an emptied
#     required), drop_dependent_keywords strips dependentSchemas/
#     dependentRequired with a prompt note, inline_internal_kimi_root_ref
#     inlines a root $ref, and an empty additionalProperties:{} is legal
#     engine-side (the validator's allow-empty arm). The guard keeps
#     flagging all of these: a red here is a false alarm in the safe
#     direction, pointing at shapes the engine silently rewrites.
#   - Bare type:object nodes are REPAIRED, not flagged: the engine injects
#     "properties": {} on every node whose type is exactly the string
#     "object" and that carries neither a properties nor an
#     additionalProperties key (inject_properties_on_bare_objects, early in
#     sanitize(); the injected key is never removed downstream). The mirror
#     replicates the injection BEFORE measuring the encoded-byte budget —
#     skipping it made the byte check a false green for exactly the shapes
#     the injection inflates (1000 bare objects fit 120000 bytes raw but
#     encode to ~136KB injected: ResourceLimitExceeded engine-side, green
#     mirror-side).
#   - "type", where present, must be a single concrete string: list-typed
#     ("type": ["array"]), non-string, and unknown values arrive typeless
#     through the engine's string-only sanitizer and die with MissingType.
#   - A node without "type" must carry anyOf; a surviving keyword must be
#     admissible for its node's type (mfjs_keyword_allowed_for_type):
#     `enum` is scalar-type-only (an `enum` on an object or boolean node is
#     InvalidKeywordValue) and its literals match the node's type;
#     required names unique declared properties. `default` is the one
#     keyword the specification deliberately keeps type-free — "这儿不做
#     类型检查，需要调用者自行保证其值类型内容和所在位置期望的内容相匹配"
#     (any position, any value, no type check) — and the doc-precise
#     engine matches the document exactly (T11:
#     kimi_parameters_ships_default_on_any_node_without_type_check), so
#     the mirror admits `default` on every node and never checks its
#     value.
#   - The parameters root must be a typed object without composition
#     (RootMustBeObject, the Tool Use root rule). A root union is flattened
#     engine-side — on the conflicting documents the strict face wins
#     (the specification lists anyOf as an applicator without authorizing
#     it at a parameters root) — so the guard's root ban stays red on a
#     root union: a false alarm in the safe direction that still matches
#     the T11 engine's flattened wire shape.
#   - Resource budgets are mirrored exactly (anyOf ≤ 500 branches, ≤ 3000
#     total properties, property depth ≤ 30, ≤ 1000 total enum values, ≤ 64
#     structural depth, ≤ 120000 encoded bytes, enums above 250 entries
#     capped at 75000 combined literal bytes) — ResourceLimitExceeded drops
#     the tool like every other deviation. Aligned to the walle conf.go
#     source defaults (source comparison, not a document clause,
#     2026-11-05); loosening one means changing the CodeWhale constants and
#     this mirror in lockstep, not waiting for Moonshot.
#   - anyOf keeps only the union's own siblings (description; root $defs)
#     and every branch is walked and concretely typed; the aligned branch
#     budget (500) and properties total (3000) leave the pptx per-layout
#     union (10 branches, 47 properties) far inside the budgets — the
#     union's real ceiling is the layout count, not the budget;
#     $defs interiors are walked so an unreferenced definition is no blind
#     spot ($defs is root-only).
#
# Background kept as observation record (the k3 slides:[] incident): with
# `type: "array"` and no items, constrained decoding completes the parameter
# to its minimal valid value [] — observed live 27 times in a row; the
# 2026-11-04 retest no longer reproduces the 27-in-a-row on a fresh single
# turn, but long-context/multi-turn field loss is still real (5 of 7 calls
# in a real user session), which is why the pptx schema moved to a
# per-layout anyOf whose branches carry the fields each layout's renderer
# cannot draw without (pinvou3-app/resources/mcp-servers/pptx/server.py).
# anyOf is a formal MFJS applicator (the specification's Applicator
# section), so the nested union itself is document-backed; on the pre-anyOf
# base a nested union flattens back to all-optional semantics, so the shape
# degrades safely backward.
_MFJS_CONCRETE_TYPES = ("string", "number", "integer", "boolean", "object", "array", "null")
# The MFJS specification's supported keyword set, exactly: the nine 字段定义
# entries plus $defs/$ref under restricted support. Everything unlisted is
# either advisory (stripped before the walk, see below) or fail-closed
# (UnsupportedKeyword drops the whole tool).
_MFJS_ALLOWED_KEYWORDS = frozenset((
    "$ref", "$defs", "anyOf", "properties", "additionalProperties",
    "items", "type", "enum", "required", "default", "description",
))
# The advisory keywords the specification does not list and the engine
# strips from the wire before validation (strip_mfjs_advisory_keywords,
# schema_sanitize.rs): annotations (semantics-lossless; "暂不支持 title、
# $comment 等描述性元数据"), the document identity keyword $id, and the
# min/max bound keywords (soft constraints the model may violate anyway).
# Removal keeps the tool shippable, so the mirror strips them too — a
# schema carrying them is compliant here because on the constrained route
# they never reach the validator, while other providers still receive the
# full schema. Everything unlisted and unstrippable (pattern, format,
# $schema, prefixItems, exclusive bounds, oneOf, allOf) fails closed here
# — see the fail-closed bullet above for the engine's partial equivalents
# the mirror deliberately does not rely on.
_MFJS_ADVISORY_KEYWORDS = (
    "title", "$comment", "$id", "minLength", "maxLength",
    "minimum", "maximum", "minItems", "maxItems",
)


def _strip_mfjs_advisory_keywords(schema):
    """Mirror of strip_mfjs_advisory_keywords on a deep copy: visit the
    schema-value positions (properties values, $defs entries, anyOf
    branches, object-form additionalProperties, items), then delete the
    advisory keys. oneOf/allOf need no visit: the containing node fails the
    keyword whitelist before any advisory keyword inside could matter."""
    node = copy.deepcopy(schema)

    def walk(value):
        if not isinstance(value, dict):
            return
        subs = value.get("properties")
        if isinstance(subs, dict):
            for sub in subs.values():
                walk(sub)
        definitions = value.get("$defs")
        if isinstance(definitions, dict):
            for sub in definitions.values():
                walk(sub)
        branches = value.get("anyOf")
        if isinstance(branches, list):
            for branch in branches:
                walk(branch)
        additional = value.get("additionalProperties")
        if isinstance(additional, dict):
            walk(additional)
        if isinstance(value.get("items"), dict):
            walk(value["items"])
        for keyword in _MFJS_ADVISORY_KEYWORDS:
            value.pop(keyword, None)

    walk(node)
    return node


def _mfjs_enum_literal_kind(value):
    """mfjs_literal_kind: str → String; JSON int → Integer (serde i64/u64);
    JSON float → Number; bool/null/object/array → None (unsupported)."""
    if isinstance(value, str):
        return "string"
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return "integer"
    if isinstance(value, float):
        return "number" if math.isfinite(value) else None
    return None


def _strip_mfjs_unsupported_enum_values(schema):
    """Mirror of strip_mfjs_unsupported_enum_values on the same deep copy:
    an enum whose values are not homogeneous float/int/str scalars is
    removed — an unconstrained node stays shippable ("……并且不支持混合类型
    (List 中的每个枚举值的类型必须相同，如不可以 str + int)"). An empty
    enum is vacuously homogeneous (all() over an empty kind list), so the
    engine's strip deliberately KEEPS it and the validator's non-empty
    check drops the whole tool instead — stripping it would silently widen
    an unsatisfiable constraint into "anything goes"
    (kimi_parameters_empty_enum_fails_closed_in_the_validator); the mirror
    keeps it too, and the walk's own non-empty check flags it. A non-array
    enum is left in place for the walk to fail closed (a malformed keyword
    is a schema bug, not a simplification opportunity). An enum-only node
    therefore keeps no type and stays red, exactly the engine's
    MissingType."""
    node = copy.deepcopy(schema)

    def walk(value):
        if not isinstance(value, dict):
            return
        subs = value.get("properties")
        if isinstance(subs, dict):
            for sub in subs.values():
                walk(sub)
        definitions = value.get("$defs")
        if isinstance(definitions, dict):
            for sub in definitions.values():
                walk(sub)
        branches = value.get("anyOf")
        if isinstance(branches, list):
            for branch in branches:
                walk(branch)
        additional = value.get("additionalProperties")
        if isinstance(additional, dict):
            walk(additional)
        if isinstance(value.get("items"), dict):
            walk(value["items"])
        values = value.get("enum")
        if isinstance(values, list):
            kinds = [_mfjs_enum_literal_kind(entry) for entry in values]
            # Strip only when the engine strips: every value must have a
            # scalar kind (the engine's Option collect yields None on the
            # first bool/null/object/array) AND the kinds must disagree.
            # An empty list is vacuously homogeneous and stays — the
            # walker's non-empty check then fails it closed, exactly the
            # engine's keep-then-reject split.
            if kinds and (any(kind is None for kind in kinds)
                          or len(set(kinds)) > 1):
                value.pop("enum", None)

    walk(node)
    return node


def _inject_properties_on_bare_objects(node):
    """Mirror of inject_properties_on_bare_objects (schema_sanitize.rs),
    applied in place BEFORE the encoded-bytes budget is measured: a schema
    node whose "type" is exactly the string "object" and that carries
    neither a "properties" nor an "additionalProperties" key (presence, any
    value) gains an empty "properties" map (+16 encoded bytes per bare
    object). The engine runs this pass early in its sanitize() recursion
    and the later strips and repairs never remove the injected key, so the
    wire bytes the validator counts include it — measuring the un-injected
    encoding let a schema the engine rejects pass as a false green (1000
    bare objects fit 120000 bytes raw but exceed it injected). The engine
    recurses over the WHOLE value tree (every dict value and array element,
    enum/default data positions included); so does this."""
    if isinstance(node, dict):
        if (node.get("type") == "object"
                and "properties" not in node
                and "additionalProperties" not in node):
            node["properties"] = {}
        for value in node.values():
            _inject_properties_on_bare_objects(value)
    elif isinstance(node, list):
        for value in node:
            _inject_properties_on_bare_objects(value)


# Per-type keyword admissibility, mirroring mfjs_keyword_allowed_for_type in
# schema_sanitize.rs exactly: a keyword surviving the whitelist is still
# InvalidKeywordValue when its node's type does not admit it — `enum` on an
# object or boolean node is the same silent tool drop. `default` is the one
# keyword the specification deliberately keeps type-free ("这儿不做类型检查，
# 需要调用者自行保证其值类型内容和所在位置期望的内容相匹配" — any value,
# any node), and the doc-precise engine matches the document
# (kimi_parameters_ships_default_on_any_node_without_type_check), so it is
# admissible everywhere and its value is never checked. type/description are
# admissible on every node and root $defs on the root, handled by the other
# rules.
_MFJS_KEYWORDS_FOR_TYPE = {
    "object": ("properties", "required", "additionalProperties", "default"),
    "array": ("items", "default"),
    "string": ("enum", "default"),
    "number": ("enum", "default"),
    "integer": ("enum", "default"),
    "boolean": ("default",),
    "null": ("default",),
}
# Resource budgets, mirroring the validator constants exactly
# (ResourceLimitExceeded raise sites in schema_sanitize.rs). Aligned to the
# walle conf.go source defaults (source comparison, not a document clause,
# 2026-11-05): MaxAnyOfItems=500, MaxTotalPropertiesKeysNum=3000,
# MaxSchemaDepth=30, 1000 total enum values, 75000 enum literal bytes; the
# encoded-bytes budget (120000) stays engine-local and the enum-length
# entry threshold (250) keeps the engine's value where walle's own default
# is 2500 — the stricter direction (more enums fall under the literal-byte
# cap here than upstream would check). Loosening one means changing the
# CodeWhale constants and this mirror in lockstep.
_MFJS_MAX_ANY_OF_ITEMS = 500
_MFJS_MAX_OBJECT_DEPTH = 30
_MFJS_MAX_TOTAL_PROPERTIES = 3000
_MFJS_MAX_TOTAL_ENUM_VALUES = 1000
# validate_mfjs_enum's conditional literal budget: an enum with more than 250
# entries may not carry more than 75000 combined literal bytes (str::len for
# strings, serde to_string() length for the numeric literals).
_MFJS_ENUM_LENGTH_CHECK_THRESHOLD = 250
_MFJS_MAX_ENUM_STRING_LENGTH = 75_000
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


_MFJS_ENUM_VALUE_MATCHERS = {
    "string": lambda values: all(isinstance(value, str) for value in values),
    "integer": lambda values: all(_mfjs_is_integer_value(value) for value in values),
    "number": lambda values: all(_mfjs_is_number(value) for value in values),
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
        # serde_json::to_vec(parameters).len() at the validator's entry —
        # measured on the POST-INJECTION tree: the engine's bare-object
        # injection runs before validation and nothing downstream removes
        # the injected key, so the wire bytes include it.
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
    for annotation in ("description",):
        # The only annotation the specification documents (title/$comment/
        # $id are advisory and stripped before this walk).
        if annotation in node and not isinstance(node[annotation], str):
            offenders.append(
                f"{path}: {annotation} must be a string (MFJS InvalidKeywordValue drops the tool)")
    if node_type is None and "anyOf" not in node:
        # Non-empty and typeless: annotation-only nodes ({"description":
        # ...}, {"enum": [...]}) and typeless containers are MissingType
        # for MFJS — the silent tool-drop class.
        offenders.append(
            f"{path}: no \"type\" and no anyOf — MFJS rejects typeless nodes with MissingType"
        )
    if "anyOf" in node:
        # MFJS allows only description beside anyOf ($defs at the root) and
        # a non-empty branch list within the union budget; everything else
        # is InvalidKeywordValue / ResourceLimitExceeded. (title is advisory
        # and stripped before this walk, so it can no longer appear here.)
        allowed_siblings = {"anyOf", "description"} | ({"$defs"} if is_root else set())
        bad_siblings = sorted(set(node) - allowed_siblings)
        if bad_siblings:
            offenders.append(
                f"{path}: anyOf sibling keyword(s) {bad_siblings} — MFJS allows only description beside anyOf"
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
        if not is_root and "$defs" in node:
            offenders.append(
                f"{path}: $defs outside the schema root — MFJS rejects them (InvalidKeywordValue tool drop)"
            )
        if node_type in _MFJS_CONCRETE_TYPES:
            # These per-node rules run in the validator only after the type
            # is established (everything past the MissingType raise), so
            # nodes already flagged typeless or non-concrete get no second
            # flag here.
            allowed = {"type", "description"} | set(_MFJS_KEYWORDS_FOR_TYPE[node_type])
            if is_root:
                allowed |= {"$defs"}
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
                    # length must stay within 75000 bytes (str::len for the
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
            # `default` ships verbatim on any node with no value check —
            # the specification's Any ("这儿不做类型检查……"), pinned
            # engine-side by
            # kimi_parameters_ships_default_on_any_node_without_type_check.
            # There is deliberately nothing to validate here.
            # (The min/max bound keywords are advisory: the strip above
            # removed them before this walk, valid or malformed alike —
            # losing only the soft constraint keeps the tool shippable.)
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


def _collect_non_finite_enum_offenders(node, path):
    """Fail closed on NaN/Infinity enum literals. This is a wire reality,
    not an MFJS rule: serde_json cannot parse a tools/list payload that
    carries them, so the request build fails before the validator ever
    runs — the tool is gone on the constrained route and broken on every
    other. The kind-based enum strip classifies a non-finite float as an
    unsupported kind, which stripped it and let the old red turn green; a
    green here would bless a schema no engine can ship. Runs on the RAW
    schema, before any strip, so the failure stays loud at every gitlink."""
    offenders: list[str] = []

    def walk(value, walk_path):
        if not isinstance(value, dict):
            return
        subs = value.get("properties")
        if isinstance(subs, dict):
            for key, sub in subs.items():
                walk(sub, f"{walk_path}.{key}")
        definitions = value.get("$defs")
        if isinstance(definitions, dict):
            for key, sub in definitions.items():
                walk(sub, f"{walk_path}.$defs.{key}")
        branches = value.get("anyOf")
        if isinstance(branches, list):
            for index, branch in enumerate(branches):
                walk(branch, f"{walk_path}.anyOf[{index}]")
        additional = value.get("additionalProperties")
        if isinstance(additional, dict):
            walk(additional, f"{walk_path}.additionalProperties")
        if isinstance(value.get("items"), dict):
            walk(value["items"], f"{walk_path}.items")
        values = value.get("enum")
        if isinstance(values, list):
            bad = [entry for entry in values
                   if isinstance(entry, float) and not math.isfinite(entry)]
            if bad:
                offenders.append(
                    f"{walk_path}: enum contains non-finite value(s) {bad!r} — NaN/Infinity cannot be carried by any JSON codec on the wire (the payload fails to parse before the validator runs)")

    walk(node, path)
    return offenders


def check_schema_contract(tool_id: str, tools: list) -> int:
    """Guard every tool's inputSchema; return the number of tools walked.

    The return value is printed into check_protocol's success line: delete
    the call site and that print immediately raises NameError — without it
    the guard's wiring could be silently dismantled while the whole CI
    matrix stays green (this file's unit tests can only pin the walker's
    internals, not the call site itself). The walk mirrors the engine's
    full pipeline: the advisory-keyword and unsupported-enum strips run
    first, the bare-object properties injection is replicated on the same
    tree BEFORE the byte budget is measured (exactly the engine's
    pre-validation transforms; the engine injects early and nothing
    downstream removes the injected key), then the walker's whole rule set
    applies to what the Moonshot wire would carry — shape, per-type keyword
    admissibility, enum/default value typing, required⊆properties, and the
    resource budgets — so a schema this guard accepts is not one the
    validator would drop.
    """
    offenders: list[str] = []
    for tool in tools:
        schema = tool.get("inputSchema")
        name = tool.get("name", "?")
        if not isinstance(schema, dict):
            offenders.append(f"{tool_id}.{name}: inputSchema missing or not an object")
            continue
        # Non-finite enum literals are checked on the RAW schema: the
        # kind-based strip would silently remove them (unsupported kind),
        # and no JSON codec can carry them anyway.
        offenders.extend(_collect_non_finite_enum_offenders(
            schema, f"{tool_id}.{name}"))
        wire_schema = _strip_mfjs_unsupported_enum_values(
            _strip_mfjs_advisory_keywords(schema))
        # The engine injects "properties": {} on bare type:object nodes
        # early and nothing downstream removes the key, so the byte budget
        # below must see the injected encoding (E19 parity).
        _inject_properties_on_bare_objects(wire_schema)
        budget = _MfjsBudget(wire_schema)
        if budget.encoded_bytes > _MFJS_MAX_SCHEMA_BYTES:
            offenders.append(
                f"{tool_id}.{name}: inputSchema encodes to {budget.encoded_bytes} bytes — over the MFJS budget of {_MFJS_MAX_SCHEMA_BYTES} (ResourceLimitExceeded drops the tool)")
        _walk_tool_schema(wire_schema, f"{tool_id}.{name}", offenders, is_root=True, budget=budget)
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
        "session-reader": {"read_session", "list_sessions", "send_message_to_session", "create_session"},
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
    # This PR's create_session addition rides the same mechanism one
    # version further: the merged manifest is 1.2.0.
    assert manifests["session-reader"]["version"] == "1.2.0", manifests["session-reader"]["version"]
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
            # create_session：必填发送者 + 隔离前缀请求者拒绝 + 无 watcher 短等待
            # 超时回 pending + spool 落盘（规范化后的 workspace 路径）+
            # 同幂等键重试 duplicate。
            no_sender = content_json(rpc.call("tools/call", {
                "name": "create_session",
                "arguments": {"title": "早报"},
            }))
            assert "required" in no_sender.get("error", "") and "from_session" in no_sender.get("error", ""), no_sender
            rejected = content_json(rpc.call("tools/call", {
                "name": "create_session",
                "arguments": {"title": "早报", "from_session": "sched-run1"},
            }))
            assert "cannot request session creation" in rejected.get("error", ""), rejected
            # Round-29 B2-3: the workspace lives OUTSIDE the relocated
            # PINVOU3_HOME — the fixture previously spooled the home itself,
            # enshrining the pre-PINVOU3_HOME-aware validator. Both
            # directions are pinned: the home refused, the outside dir
            # accepted and spooled (with the canonicalized path).
            inside = content_json(rpc.call("tools/call", {
                "name": "create_session",
                "arguments": {
                    "title": "早报", "first_message": "汇总",
                    "workspace_path": str(home),
                    "from_session": "src0001",
                },
            }))
            assert "Pinvou home" in inside.get("error", ""), inside
            with tempfile.TemporaryDirectory(prefix="pinvou-create-ws-") as ws_root:
                ws_dir = Path(ws_root, "ws")
                ws_dir.mkdir()
                pending = content_json(rpc.call("tools/call", {
                    "name": "create_session",
                    "arguments": {
                        "title": "重构会话", "first_message": "重构解析器",
                        "workspace_path": str(ws_dir) + "/..",
                        "from_session": "src0001", "idempotency_key": "k1",
                    },
                }))
                assert pending.get("ok") is True and pending.get("sessionId") is None, pending
                assert pending.get("delivery") == "pending", pending
                spooled = sorted(Path(home, "session-requests", "spool").glob("*.json"))
                assert len(spooled) == 1, spooled
                record = json.loads(spooled[0].read_text(encoding="utf-8"))
                assert record["title"] == "重构会话" and record["first_message"] == "重构解析器", record
                assert record["workspace_path"] == str(Path(ws_root).resolve()), record
                # Round-13 minor 9: the replay is IDENTICAL (same
                # workspace_path argument, so the payload digest matches) —
                # the old form resent a DIVERGENT payload and asserted only
                # duplicate:True computed from the pre-existing record, so it
                # could not distinguish identical-replay from ignore.
                # Re-assert the record and the mismatch flag as well.
                duplicate = content_json(rpc.call("tools/call", {
                    "name": "create_session",
                    "arguments": {
                        "title": "重构会话", "first_message": "重构解析器",
                        "workspace_path": str(ws_dir) + "/..",
                        "from_session": "src0001", "idempotency_key": "k1",
                    },
                }))
                assert duplicate.get("duplicate") is True, duplicate
                assert not duplicate.get("payload_mismatch"), duplicate
                spooled = sorted(Path(home, "session-requests", "spool").glob("*.json"))
                assert len(spooled) == 1, spooled
                record = json.loads(spooled[0].read_text(encoding="utf-8"))
                assert record["title"] == "重构会话" and record["first_message"] == "重构解析器", record
                assert record["workspace_path"] == str(Path(ws_root).resolve()), record
    print("✅ session-reader: 跨会话消息/会话创建校验/幂等/隔离前缀全旅程")

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
                                target_session=request.get("target_session"),
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
            target_file = Path(home, "sessions", "tgt0001.json")
            target_file.parent.mkdir(parents=True, exist_ok=True)
            target_file.write_text("{}", encoding="utf-8")
            target_args = {
                "name": "定时消息", "prompt": "确认", "rrule": "FREQ=HOURLY;INTERVAL=6",
                "target_session": "tgt0001", "from_session": "src0001",
                "idempotency_key": "target-smoke",
            }
            target_result = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task", "arguments": target_args,
            }))
            assert target_result.get("taskId", "").startswith("sim-"), target_result
            target_replay = content_json(rpc.call("tools/call", {
                "name": "create_scheduled_task", "arguments": target_args,
            }))
            assert target_replay.get("duplicate") is True, target_replay
            assert target_replay.get("payload_mismatch") is False, target_replay
            stop.set()
            # Round-15 minor 4: the watcher must be GONE before the later
            # journeys run — a thread descheduled between its stop-check and
            # the spool scan could otherwise drain timeout2's record after
            # stop.set() and flip the pending/target assertions below. The
            # bounded join makes its lifetime deterministic.
            watcher_thread.join(timeout=2)
            assert not watcher_thread.is_alive(), "fake watcher must finish before inspecting the spool"
            # Both pending and targeted writes drain; three receipts remain.
            assert not list(Path(home, "task-requests", "spool").glob("*.json"))
            assert len(list(Path(home, "task-requests", "spool", ".done").glob("*.json"))) == 3
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
