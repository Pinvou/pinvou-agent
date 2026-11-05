"""Unit tests for the model-visible schema guard in mcp-server-contract-smoke.

Policy (ruling of 2026-11-05): the published Moonshot documents are the
sole shipping basis for the constrained-decoding wire — a live probe may
only ever tighten it, never widen it. The guard (check_schema_contract/
_walk_tool_schema) runs inside the CI fast-gate's
mcp-server-contract-smoke.py, shape-checking every bundled server's
tools/list result against exactly the keyword set those documents list,
mirroring the engine pipeline at the pinned CodeWhale gitlink
(schema_sanitize.rs):

- the whitelist is the MFJS specification's supported set, exactly — its
  字段定义 sections enumerate description, default, anyOf, properties,
  additionalProperties, items, type, enum, required, plus $defs/$ref under
  restricted support ("$defs 必须定义在 schema 的根级别 (root level)；$ref
  仅支持引用本文档内部的 subschema") — and its closing clause is the
  authority for everything else: "若官方规范中的某些概念、关键字或特性在
  本文档中未被提及，则表示当前 MFJS 尚未提供官方支持……我们不保证它们能在
  最终的结构化输出中产生稳定、符合预期的结果" (mfjs-spec.zh.md, fetched
  2026-11-05);
- the advisory keywords the document cannot express (title, $comment, $id,
  minLength, maxLength, minimum, maximum, minItems, maxItems — the
  annotation group is named outright: "暂不支持 title、$comment 等描述性
  元数据") are STRIPPED before validation, engine-side
  (strip_mfjs_advisory_keywords) and in this mirror: removal is either
  semantics-lossless (annotations) or loses only soft constraints (bounds),
  so the tool still ships and the Moonshot wire carries only the documented
  set — a schema carrying them is green here, not red, while other
  providers keep receiving the full schema (malformed advisory values are
  stripped too: kimi_parameters_strips_bounds_instead_of_rejecting_them);
- the fail-closed set keeps the whole-tool drop: pattern, format, $schema,
  prefixItems, exclusive bounds, nested allOf, oneOf, and every other
  unlisted keyword has no removal equivalent ("暂不支持使用 prefixItems 或
  unevaluatedItems 等方式模拟实现元组", "暂不支持如 exclusiveMinimum，
  exclusiveMaximum，minContains, maxContains 等复杂校验定义") and the
  validator silently drops the tool (UnsupportedKeyword / MissingType /
  InvalidSchemaNode) — how #683 lost evaluate_script;
- an enum whose values are not homogeneous float/int/str scalars is
  stripped ("枚举值我们只支持基础类型中的 float, int 和 str……并且不支持
  混合类型 (List 中的每个枚举值的类型必须相同，如不可以 str + int)");
  a surviving enum's values must still match its node's type;
- "type" is a single concrete string (or absent in favor of anyOf with
  union-only siblings); a surviving keyword must be admissible for its
  node's type (default on an array or enum/default on an object is
  InvalidKeywordValue); required names unique declared properties; a
  keyword explicitly present with a null value counts as present-and-invalid
  (never "absent"); the root is a typed object without composition (the
  Tool Use guide: "The root of `parameters` must be an `object`", and
  strict defaults to true — "true (default, omitting is equivalent to
  true)" — so the schema is enforced on the wire; a root union is flattened
  engine-side on the strict face of the conflicting documents and this
  guard stays red on it, a false alarm in the safe direction)
  (platform.kimi.ai/docs/api/tool-use.md, fetched 2026-11-05);
- the resource budgets hold (anyOf ≤ 10 branches, ≤ 100 total properties,
  property depth ≤ 5, ≤ 500 total enum values, ≤ 64 structural depth,
  ≤ 120000 encoded bytes, enums above 250 entries capped at 7500 combined
  literal bytes). These budgets are the CodeWhale engine's own stricter
  MFJS mirror policy — the walle source defaults are MaxAnyOfItems=500,
  MaxTotalPropertiesKeysNum=3000, MaxSchemaDepth=30 (verified against
  MoonshotAI/walle upstream main on 2026-11-05, cited for contrast only) —
  so loosening one means changing the CodeWhale constants, not waiting for
  Moonshot. The 2026-11-04 probe observations (root unions, pattern/format/
  $schema/prefixItems/exclusive bounds/nested allOf accepted with 200 on
  the coding endpoint) are recorded observations and never widen this
  contract.

All bundled servers are currently compliant, so if the guard itself breaks
the smoke stays green (against compliant schemas a broken guard is exactly
as silent as a good one) — only the direct unit tests here turn red when
the guard rots. Both directions are pinned: the incident shape (itemless
array), items:{}, untyped items, tuple items, boolean schemas, list-typed /
non-string / unknown "type" values, typeless annotation-only nodes,
non-object schema values, $ref/$dynamicRef/$recursiveRef, empty or
sibling-polluted anyOf, non-root $defs, every fail-closed keyword (oneOf,
allOf, patternProperties, prefixItems, pattern, format, $schema,
exclusiveMinimum/exclusiveMaximum, contains, propertyNames,
not/if/then/else, dependentSchemas, draft-07 dependencies, unevaluated*,
contentSchema), the per-type/budget deviations, the explicit-null keyword
values, the homogeneous-enum type mismatches, and the reviewer
counterexamples (array default, object enum/default, eleven-branch anyOf,
null keyword values, oversized enum literals) must be red; advisory
keywords (stripped then passed, even malformed), stripped unsupported
enums, typed enums, typed unions within every budget, and the real pptx
schema must be green. Every recursion lane that survives (properties,
additionalProperties, items, anyOf branches, $defs) has a red case pinning
it: the walker's recursion is not decorative, every lane is consumed by an
assertion. The wiring (the call site itself) is pinned by
check_schema_contract's return-value contract: check_protocol asserts the
returned count against the boot contract and prints it into the success
line, which raises the moment the call site is deleted; the unit test pins
the return value.

The real-pptx-schema green case loads the schema in a short-lived
subprocess, never by importing server.py into this process: on Windows the
server's import-time UTF-8 setup replaces sys.stdout with a fresh
TextIOWrapper over the shared console buffer, so an in-process import
leaves a wrapper whose release closes the buffer under the process's real
stdout — a later module in the same discovery run (the Windows diagnostics
tests) then dies with "ValueError: I/O operation on closed file" on its
first print (review #713 round 7, P2-2). When python-pptx is not installed
(the fast-gate discover runs before deps) the probe fails to import and
the case skips, exactly like the direct import it replaced; CI's green
coverage of the real schema comes from the smoke step later in the same
job (a real stdio pptx boot run through the same guard).

Since 2026-11-04 the real pptx schema is a per-layout anyOf: constrained
decoding channels complete a deep optional-field payload to its minimal
legal shape (layout+title-only pages), so slides.items is an anyOf union
of exactly one type:object branch per layout whose required lists the
fields that layout's renderer cannot draw without (the only 10/10-
compliant probe shape at +6% bytes; the all-fields-required control
falsified at +76% tokens). The union-level tests pin that shape on the
walker side — typed object branches beside anyOf stay green, the branch
budget boundary holds for object branches too, and duplicated property
names across branches accumulate into the total-properties budget — and
the real-schema cases pin the union itself (exactly one branch per
layout, single-value layout enums, required ⊆ declared properties,
properties total within budget). On the pre-anyOf base the engine
flattens the union back to all-optional semantics — the old behavior —
so the shape degrades safely backward.
"""
import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SMOKE = ROOT / "scripts" / "mcp-server-contract-smoke.py"
PPTX_SERVER = ROOT / "pinvou3-app" / "resources" / "mcp-servers" / "pptx" / "server.py"


def _load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


smoke = _load_module("mcp_contract_smoke_under_test", SMOKE)


def _load_pptx_schema():
    """Return the real pptx inputSchema, extracted in a subprocess.

    The import itself must not happen in this process (module docstring:
    the server's Windows import-time stream setup poisons later tests);
    the child only reports the schema as JSON. Any failure — python-pptx
    missing is the expected one — keeps the skip semantics of the direct
    import this replaced.
    """
    probe = (
        "import importlib.util, json, sys\n"
        "spec = importlib.util.spec_from_file_location('pptx_server_probe', sys.argv[1])\n"
        "module = importlib.util.module_from_spec(spec)\n"
        "spec.loader.exec_module(module)\n"
        "sys.stdout.write(json.dumps(module.TOOL_DEFS[0]['inputSchema'], ensure_ascii=True))\n"
    )
    try:
        proc = subprocess.run(
            [sys.executable, "-c", probe, str(PPTX_SERVER)],
            capture_output=True, text=True, encoding="utf-8", timeout=60,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"pptx schema probe failed to run ({error}); the real-schema case skips",
              file=sys.stderr)
        return None
    if proc.returncode != 0:
        if "No module named" not in proc.stderr:
            print(f"pptx schema probe exited {proc.returncode}: {proc.stderr[-400:]}",
                  file=sys.stderr)
        return None
    return json.loads(proc.stdout)


pptx_schema = _load_pptx_schema()


def _contract_message(schema):
    """Run the guard; return the offender-list text, or None when compliant."""
    try:
        smoke.check_schema_contract("t", [{"name": "tool", "inputSchema": schema}])
    except AssertionError as error:
        return str(error)
    return None


class SchemaContractWalkerTest(unittest.TestCase):
    def test_incident_shape_itemless_array_is_red(self):
        message = _contract_message(
            {"type": "object", "properties": {"slides": {"type": "array"}}})
        self.assertIsNotNone(message)
        self.assertIn("without concretely typed items", message)
        self.assertIn("slides", message)

    def test_empty_items_node_is_red(self):
        message = _contract_message({"type": "array", "items": {}})
        self.assertIsNotNone(message)
        self.assertIn("empty schema node", message)

    def test_untyped_items_node_is_red(self):
        message = _contract_message({"type": "array", "items": {"description": "x"}})
        self.assertIsNotNone(message)
        self.assertIn("without concretely typed items", message)

    def test_tuple_items_is_red(self):
        message = _contract_message({"type": "array", "items": [{"type": "string"}]})
        self.assertIsNotNone(message)
        self.assertIn("tuple", message)

    def test_union_with_untyped_branch_is_red(self):
        message = _contract_message(
            {"type": "array",
             "items": {"anyOf": [{"type": "string"}, {"description": "x"}]}})
        self.assertIsNotNone(message)
        self.assertIn("without concretely typed items", message)

    def test_one_of_keyword_is_red(self):
        # MFJS has no oneOf: ALLOWED_KEYWORDS rejects it outright
        # (UnsupportedKeyword), so a union must be spelled anyOf.
        message = _contract_message(
            {"type": "array",
             "items": {"oneOf": [{"type": "string"}, {"type": "null"}]}})
        self.assertIsNotNone(message)
        self.assertIn("unsupported keyword", message)
        self.assertIn("oneOf", message)

    def test_all_of_keyword_is_red(self):
        # Same as oneOf: allOf is outside the verified MFJS shape set even
        # when every branch is fully typed.
        message = _contract_message(
            {"type": "array",
             "items": {"allOf": [{"type": "string"}, {"maxLength": 5}]}})
        self.assertIsNotNone(message)
        self.assertIn("unsupported keyword", message)
        self.assertIn("allOf", message)

    def test_pattern_properties_keyword_is_red(self):
        # Pin the keyword ban on the last JSON-Schema name→schema map the
        # old walker recursed into: MFJS has no patternProperties.
        message = _contract_message(
            {"type": "object", "patternProperties": {".*": {"type": "string"}}})
        self.assertIsNotNone(message)
        self.assertIn("unsupported keyword", message)
        self.assertIn("patternProperties", message)

    def test_dollar_ref_is_red_as_property_and_as_items(self):
        message = _contract_message(
            {"type": "object",
             "definitions": {"w": {"type": "string"}},
             "properties": {"a": {"$ref": "#/definitions/w"},
                            "b": {"type": "array", "items": {"$ref": "#/definitions/w"}}}})
        self.assertIsNotNone(message)
        self.assertIn("$ref", message)

    def test_dynamic_and_recursive_ref_are_red(self):
        # Same indirection class as $ref: the guard cannot inspect the
        # target, so the shape underneath would go uninspected.
        for keyword in ("$dynamicRef", "$recursiveRef"):
            message = _contract_message({keyword: "#a"})
            self.assertIsNotNone(message, keyword)
            self.assertIn("not followed by this guard", message, keyword)

    def test_nested_array_without_items_is_red_at_inner_level(self):
        message = _contract_message({"type": "array", "items": {"type": "array"}})
        self.assertIsNotNone(message)
        self.assertIn("without concretely typed items", message)

    def test_empty_property_node_is_red(self):
        message = _contract_message({"type": "object", "properties": {"x": {}}})
        self.assertIsNotNone(message)
        self.assertIn("empty schema node", message)

    def test_boolean_schema_node_is_red(self):
        # true is semantically the empty schema: same class as {}, must be red.
        message = _contract_message({"type": "object", "properties": {"x": True}})
        self.assertIsNotNone(message)
        self.assertIn("boolean schema", message)

    def test_prefix_items_keyword_is_red(self):
        # Tuple semantics: legal JSON Schema, outside the verified MFJS
        # shape set like tuple items.
        message = _contract_message(
            {"type": "array", "items": {"type": "string"},
             "prefixItems": [{"type": "string"}]})
        self.assertIsNotNone(message)
        self.assertIn("prefixItems", message)

    def test_itemless_array_inside_additional_properties_is_red(self):
        # Pin the additionalProperties recursion lane: remove that recursion
        # and this case must stop turning red.
        message = _contract_message(
            {"type": "object", "additionalProperties": {"type": "array"}})
        self.assertIsNotNone(message)
        self.assertIn("additionalProperties", message)
        self.assertIn("without concretely typed items", message)

    def test_itemless_array_inside_union_branch_is_red(self):
        # Pin the anyOf branch recursion lane: any branch of a union may be
        # taken, so an itemless array in one branch is incident-shaped.
        message = _contract_message(
            {"type": "object",
             "properties": {"x": {"anyOf": [{"type": "array"}, {"type": "string"}]}}})
        self.assertIsNotNone(message)
        self.assertIn("anyOf[0]", message)
        self.assertIn("without concretely typed items", message)

    def test_list_typed_type_keyword_is_red(self):
        # Legal JSON Schema, but the engine sanitizer reads string types
        # only: the node arrives typeless downstream and MFJS drops the tool
        # (#683 class). Banned outright, not interpreted.
        message = _contract_message(
            {"type": "object", "properties": {"x": {"type": ["string"]}}})
        self.assertIsNotNone(message)
        self.assertIn("list-typed", message)

    def test_list_typed_incident_shape_is_red(self):
        # The incident shape encoded as a list type must not slip past the
        # array trigger (a plain string compare would miss it).
        message = _contract_message({"type": ["array"]})
        self.assertIsNotNone(message)
        self.assertIn("list-typed", message)

    def test_list_typed_inside_additional_properties_is_red(self):
        # The ban must hold at every recursion lane, not just properties.
        message = _contract_message(
            {"type": "object", "additionalProperties": {"type": ["array", "string"]}})
        self.assertIsNotNone(message)
        self.assertIn("additionalProperties", message)
        self.assertIn("list-typed", message)

    def test_non_string_type_value_is_red(self):
        # {"type": 1234}: the sanitizer reads string types only, so this
        # node arrives typeless and dies with MissingType.
        message = _contract_message({"type": "object", "properties": {"x": {"type": 1234}}})
        self.assertIsNotNone(message)
        self.assertIn("not a concrete MFJS string type", message)

    def test_non_concrete_type_string_is_red(self):
        # {"type": "Array"} is not a concrete MFJS type (case-sensitive
        # string compare upstream): same MissingType drop.
        message = _contract_message({"type": "object", "properties": {"x": {"type": "Array"}}})
        self.assertIsNotNone(message)
        self.assertIn("not a concrete MFJS string type", message)

    def test_typeless_annotation_node_is_red(self):
        # A non-empty node with no "type" and no anyOf is MissingType for
        # MFJS: {"description": ...} is the same silent tool-drop class as
        # the empty node, not an "always accept" wildcard.
        message = _contract_message(
            {"type": "object", "properties": {"x": {"description": "y"}}})
        self.assertIsNotNone(message)
        self.assertIn("no \"type\" and no anyOf", message)

    def test_typeless_enum_property_is_red(self):
        # {"enum": [...]} without a type reads as "any of these" in JSON
        # Schema but is MissingType for MFJS: the enum leaf must carry its
        # concrete type.
        message = _contract_message(
            {"type": "object", "properties": {"e": {"enum": ["a", "b"]}}})
        self.assertIsNotNone(message)
        self.assertIn("no \"type\" and no anyOf", message)

    def test_schema_list_value_is_red(self):
        # A list at a schema position ({"items": [[]]}, a list-valued
        # property) is InvalidSchemaNode upstream, not a silently-skipped
        # value.
        message = _contract_message({"type": "object", "properties": {"x": ["a"]}})
        self.assertIsNotNone(message)
        self.assertIn("MFJS requires an object", message)

    def test_empty_anyof_branch_list_is_red(self):
        message = _contract_message(
            {"type": "object", "properties": {"x": {"anyOf": []}}})
        self.assertIsNotNone(message)
        self.assertIn("non-empty branch list", message)

    def test_anyof_type_sibling_is_red(self):
        # MFJS allows only description beside anyOf (title is advisory and
        # stripped before the sibling check): a typed node with a union is
        # InvalidKeywordValue, not an intersection.
        message = _contract_message(
            {"type": "object",
             "properties": {"x": {"type": "object", "anyOf": [{"type": "string"}]}}})
        self.assertIsNotNone(message)
        self.assertIn("anyOf sibling", message)

    def test_content_schema_keyword_is_red(self):
        # contentSchema would otherwise be a hiding spot for an itemless
        # array; it is simply outside the MFJS keyword set.
        message = _contract_message(
            {"type": "object",
             "properties": {"r": {"type": "string", "contentSchema": {"type": "array"}}}})
        self.assertIsNotNone(message)
        self.assertIn("unsupported keyword", message)
        self.assertIn("contentSchema", message)

    def test_defs_outside_root_is_red(self):
        # $defs (and $id) are root-only for MFJS; elsewhere they are
        # InvalidKeywordValue, so a nested definition cannot act as a data
        # side channel either.
        message = _contract_message(
            {"type": "object",
             "properties": {"x": {"$defs": {}, "type": "string"}}})
        self.assertIsNotNone(message)
        self.assertIn("outside the schema root", message)

    def test_itemless_array_inside_defs_is_red(self):
        # $defs interiors recurse (an unreferenced definition must not be a
        # blind spot); draft-07 `definitions` is not an MFJS keyword and is
        # named by the keyword ban itself.
        message = _contract_message(
            {"type": "object",
             "$defs": {"a": {"type": "array"}},
             "definitions": {"b": {"type": "array"}}})
        self.assertIsNotNone(message)
        self.assertIn("$defs.a", message)
        self.assertIn("unsupported keyword", message)
        self.assertIn("definitions", message)

    def test_contains_keyword_is_red(self):
        message = _contract_message({"type": "array", "items": {"type": "string"}, "contains": {"type": "array"}})
        self.assertIsNotNone(message)
        self.assertIn("unsupported keyword", message)
        self.assertIn("contains", message)

    def test_property_names_keyword_is_red(self):
        message = _contract_message({"type": "object", "propertyNames": {"type": "array"}})
        self.assertIsNotNone(message)
        self.assertIn("unsupported keyword", message)
        self.assertIn("propertyNames", message)

    def test_not_and_conditional_keywords_are_red(self):
        message = _contract_message(
            {"type": "object",
             "not": {"type": "array"},
             "if": {"type": "array"},
             "then": {"type": "array"},
             "else": {"type": "array"}})
        self.assertIsNotNone(message)
        for keyword in ("not", "if", "then", "else"):
            self.assertIn(f"'{keyword}'", message)

    def test_dependent_schemas_and_dependencies_keywords_are_red(self):
        message = _contract_message(
            {"type": "object",
             "dependentSchemas": {"a": {"type": "array"}},
             "dependencies": {"b": {"type": "array"}, "c": ["x"]}})
        self.assertIsNotNone(message)
        self.assertIn("dependentSchemas", message)
        self.assertIn("dependencies", message)

    def test_unevaluated_keywords_are_red(self):
        message = _contract_message(
            {"type": "object",
             "unevaluatedProperties": {"type": "array"},
             "unevaluatedItems": True})
        self.assertIsNotNone(message)
        self.assertIn("unevaluatedProperties", message)
        self.assertIn("unevaluatedItems", message)

    def test_guard_wiring_returns_walked_tool_count(self):
        # Wiring pin: check_protocol asserts this return value against the
        # boot contract and prints it into the success line, both of which
        # raise the moment the call site is deleted — the walker's own unit
        # tests cannot pin the call site. Roots stay object-typed: a
        # parameters root must be a typed object (RootMustBeObject), so a
        # scalar root is not a compliant tool schema to count.
        walked = smoke.check_schema_contract("t", [
            {"name": "a", "inputSchema": {"type": "object",
                                          "properties": {"x": {"type": "string"}}}},
            {"name": "b", "inputSchema": {"type": "object",
                                          "properties": {"y": {"type": "array",
                                                               "items": {"type": "string"}}}}},
        ])
        self.assertEqual(walked, 2)

    def test_non_object_input_schema_is_red(self):
        message = _contract_message("not-a-schema")
        self.assertIsNotNone(message)
        self.assertIn("inputSchema", message)

    # ── Per-type keyword admissibility and value typing (round 7, P2-3) ──
    # A keyword surviving the whitelist can still be InvalidKeywordValue on
    # its node's type; these pin the reviewer counterexamples that the
    # whitelist-only guard wrongly passed.

    def test_array_default_is_red(self):
        # The reviewer counterexample, verbatim: MFJS forbids `default` on
        # an array (InvalidKeywordValue) — the whitelist alone accepted it.
        message = _contract_message(
            {"type": "object",
             "properties": {"options": {"type": "array", "items": {"type": "string"},
                                        "default": []}}})
        self.assertIsNotNone(message)
        self.assertIn("not permitted on type \"array\"", message)
        self.assertIn("default", message)

    def test_object_enum_and_default_are_red(self):
        # The same mismatch on object nodes: enum/default are scalar-type
        # keywords only.
        message = _contract_message(
            {"type": "object",
             "properties": {"o": {"type": "object",
                                  "properties": {"a": {"type": "string"}},
                                  "enum": ["x"], "default": {}}}})
        self.assertIsNotNone(message)
        self.assertIn("not permitted on type \"object\"", message)
        self.assertIn("'default'", message)
        self.assertIn("'enum'", message)

    def test_enum_value_type_mismatch_is_red(self):
        # A homogeneous scalar enum survives the strip, so the validator's
        # enum/type match still drops the tool on a mismatch (the mixed
        # ["a", 1] shape is stripped away before this check runs — see the
        # strip pins below).
        message = _contract_message(
            {"type": "object",
             "properties": {"e": {"type": "integer", "enum": ["a", "b"]}}})
        self.assertIsNotNone(message)
        self.assertIn('do not all match type "integer"', message)

    def test_default_value_type_mismatch_is_red(self):
        message = _contract_message(
            {"type": "object",
             "properties": {"d": {"type": "string", "default": 5}}})
        self.assertIsNotNone(message)
        self.assertIn("default does not match type \"string\"", message)

    # ── Advisory-keyword strip pins (2026-11-05 document-precise rewrite):
    # the specification's field set has no title/$comment/$id and no bound
    # keywords, and the engine strips them from the wire before validation
    # instead of dropping the tool — so the mirror strips too (strip-then-
    # pass), and the remaining documented keywords are still validated. Each
    # of these was red under the pre-strip mirror that validated the
    # advisory keywords as if the document listed them.

    def test_mixed_kind_enum_is_stripped_and_green(self):
        # "并且不支持混合类型 (List 中的每个枚举值的类型必须相同，如不可以
        # str + int)": the enum is removed, the tool ships.
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"e": {"type": "string", "enum": ["a", 1]}}}))

    def test_non_scalar_and_empty_enums_are_stripped_and_green(self):
        # bool/null/object/array values sit outside "float, int 和 str":
        # the keyword is removed (an unconstrained node stays shippable).
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {
                 "flag": {"type": "boolean", "enum": [False]},
                 "nullish": {"type": "null", "enum": [None]},
                 "obj": {"type": "object",
                         "properties": {"a": {"type": "string"}},
                         "enum": [{"a": 1}]},
                 "arr": {"type": "array", "items": {"type": "string"},
                         "enum": [["a"]]},
                 "empty": {"type": "string", "enum": []},
             }}))
        # An enum-only node keeps no type after the strip: MissingType,
        # exactly the baseline behavior — the strip never invents a type.
        message = _contract_message(
            {"type": "object", "properties": {"leaf": {"enum": ["a", 1]}}})
        self.assertIsNotNone(message)
        self.assertIn("empty schema node", message)

    def test_serde_kind_mix_enum_is_stripped_homogeneous_stays_green(self):
        # The homogeneity split follows serde kinds, not JSON values:
        # 1 (Integer) and 2.5 (Number) are different kinds and the enum is
        # stripped; [1, 2] and [0.5, 1.5] are homogeneous and survive into
        # the validator's own enum/type match.
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {
                 "mixed": {"type": "number", "enum": [1, 2.5]},
                 "ints": {"type": "integer", "enum": [1, 2]},
                 "floats": {"type": "number", "enum": [0.5, 1.5]},
             }}))

    def test_advisory_keywords_are_stripped_not_shipped(self):
        # The advisory set is exactly the nine keywords the engine removes
        # (title, $comment, $id, minLength, maxLength, minimum, maximum,
        # minItems, maxItems); everything else on these nodes is documented
        # (enum/default/description) and still validated after the strip.
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"s": {"type": "string", "default": "x", "enum": ["x"],
                                  "minLength": 1, "maxLength": 5, "title": "S"},
                            "i": {"type": "integer", "default": 3, "enum": [3],
                                  "minimum": -3, "maximum": 9},
                            "a": {"type": "array", "items": {"type": "string"},
                                  "minItems": 1, "maxItems": 5},
                            "b": {"type": "boolean", "default": False},
                            "c": {"type": "string", "$comment": "hint",
                                  "$id": "https://example.invalid/s"}}}))

    def test_malformed_advisory_bounds_are_stripped_not_rejected(self):
        # The engine strips the bound keywords before validating any value
        # (kimi_parameters_strips_bounds_instead_of_rejecting_them), so a
        # malformed bound — foreign type placement, out-of-range literal,
        # inverted range, explicit null — loses only the soft constraint
        # and the tool ships. Red under the pre-strip mirror that validated
        # bound values as if the document listed them.
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {
                 "a": {"type": "array", "items": {"type": "string"},
                       "minLength": 1},
                 "s": {"type": "string", "minLength": -1},
                 "n": {"type": "number", "minimum": 5, "maximum": 1},
                 "z": {"type": "number", "minimum": None},
                 "t": {"type": "string", "minLength": None, "maxLength": 5},
             }}))

    def test_malformed_annotation_is_stripped_and_description_still_checked(self):
        # Annotations are stripped wholesale (values included), so a
        # non-string title ships; description is a documented keyword and
        # its string typing still drops the tool.
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"x": {"type": "string", "title": 5}}}))
        message = _contract_message(
            {"type": "object",
             "properties": {"x": {"type": "string", "description": 5}}})
        self.assertIsNotNone(message)
        self.assertIn("description must be a string", message)

    def test_non_root_dollar_id_is_stripped_and_green(self):
        # The advisory strip removes $id at every position, so the old
        # "$defs/$id outside the root" red no longer fires for $id; only
        # $defs keeps a root-only rule.
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"x": {"$id": "https://example.invalid/x",
                                  "type": "string"}}}))

    def test_document_named_fail_closed_keywords_are_red(self):
        # The specification names these groups outright: format under the
        # annotation ban ("也不支持 format 格式注解字段"), exclusive bounds
        # under the complex-validation ban ("暂不支持如 exclusiveMinimum，
        # exclusiveMaximum，minContains, maxContains 等复杂校验定义");
        # pattern has no removal equivalent and fails closed with them.
        message = _contract_message(
            {"type": "object",
             "properties": {
                 "p": {"type": "string", "pattern": "^a"},
                 "f": {"type": "string", "format": "date-time"},
                 "e": {"type": "integer", "exclusiveMinimum": 0,
                       "exclusiveMaximum": 10},
             }})
        self.assertIsNotNone(message)
        for keyword in ("pattern", "format", "exclusiveMinimum",
                        "exclusiveMaximum"):
            self.assertIn(f"'{keyword}'", message)

    def test_dollar_schema_keyword_is_red(self):
        # Meta-Schemas are unsupported outright ("暂不支持 Meta-Schemas");
        # the closing clause fail-closes the keyword.
        message = _contract_message(
            {"type": "object",
             "properties": {
                 "x": {"type": "string",
                       "$schema": "https://json-schema.org/draft/2020-12/schema"}}})
        self.assertIsNotNone(message)
        self.assertIn("unsupported keyword", message)
        self.assertIn("$schema", message)

    def test_required_must_name_unique_declared_properties(self):
        message = _contract_message(
            {"type": "object",
             "properties": {"a": {"type": "string"}},
             "required": ["a", "ghost", "a"]})
        self.assertIsNotNone(message)
        self.assertIn("unique declared properties", message)

    def test_required_without_properties_is_red(self):
        message = _contract_message({"type": "object", "required": ["a"]})
        self.assertIsNotNone(message)
        self.assertIn("requires a properties object", message)

    def test_properties_must_be_object_is_red(self):
        # A list at the properties keyword itself previously fell out of the
        # walker's isinstance-dict gate silently and passed — the validator
        # rejects the shape outright.
        message = _contract_message({"type": "object", "properties": []})
        self.assertIsNotNone(message)
        self.assertIn("properties must be an object", message)

    def test_additional_properties_must_be_bool_or_object_is_red(self):
        message = _contract_message({"type": "object", "additionalProperties": []})
        self.assertIsNotNone(message)
        self.assertIn("additionalProperties must be a boolean or a schema object", message)

    def test_root_defs_must_be_object_is_red(self):
        message = _contract_message({"type": "object", "$defs": []})
        self.assertIsNotNone(message)
        self.assertIn("$defs must be an object", message)

    def test_reserved_property_name_is_red(self):
        message = _contract_message(
            {"type": "object", "properties": {"anyOf": {"type": "string"}}})
        self.assertIsNotNone(message)
        self.assertIn("empty or reserved", message)

    def test_root_must_be_object_is_red(self):
        # validate_mfjs_parameters (RootMustBeObject): function parameters
        # are always a typed object on the constrained route.
        message = _contract_message({"type": "string"})
        self.assertIsNotNone(message)
        self.assertIn("root must be type \"object\"", message)

    # ── Explicit-null keyword values (round 8): the validator's keyword arms
    # are presence-aware (obj.get(k) + is_some), so a keyword explicitly
    # present with a null value is a present-and-invalid value — it never
    # reads as "keyword absent". A .get(...) is-not-None probe cannot tell
    # the two apart because JSON null is Python None.

    def test_null_properties_is_red(self):
        message = _contract_message(
            {"type": "object",
             "properties": {"inner": {"type": "object", "properties": None}}})
        self.assertIsNotNone(message)
        self.assertIn("properties must be an object", message)

    def test_null_additional_properties_is_red(self):
        message = _contract_message(
            {"type": "object",
             "properties": {"o": {"type": "object", "additionalProperties": None}}})
        self.assertIsNotNone(message)
        self.assertIn("additionalProperties must be a boolean or a schema object", message)

    def test_null_required_is_red(self):
        message = _contract_message(
            {"type": "object",
             "properties": {"o": {"type": "object",
                                  "properties": {"a": {"type": "string"}},
                                  "required": None}}})
        self.assertIsNotNone(message)
        self.assertIn("required must be a list", message)

    # (The round-8 null-bound pins moved into the strip block above: a
    # bound keyword is stripped before any value is validated, so an
    # explicit null inside an advisory keyword can no longer be red.)

    # ── Resource budgets (round 7, P2-3): ResourceLimitExceeded drops the
    # tool exactly like every other deviation, so each budget needs a red
    # pin and its boundary stays green.

    def test_anyof_branch_budget_is_enforced(self):
        branches = [{"type": "string"}] * 11
        message = _contract_message({"type": "object", "properties": {"x": {"anyOf": branches}}})
        self.assertIsNotNone(message)
        self.assertIn("at most 10", message)
        self.assertIsNone(_contract_message(
            {"type": "object", "properties": {"x": {"anyOf": branches[:10]}}}))

    def test_anyof_items_of_typed_object_branches_with_required_is_green(self):
        # The per-layout mitigation shape (pptx slides.items, 2026-11-04):
        # an items anyOf whose branches are explicit type:object nodes with
        # their own properties and required. MFJS legalizes this union
        # (ValidateAnyOf; the rules the engine enforces here — explicit
        # branch types, no type sibling at the anyOf level, the 10/100/5
        # budgets — are the CodeWhale mirror's own policy, deliberately
        # stricter than the walle upstream defaults of 500 branches / 3000
        # properties / 30 depth, verified against walle main on
        # 2026-11-05), so the guard must stay green — the constrained
        # decoder reads each branch's required as the fields it must fill.
        branches = [
            {"type": "object",
             "properties": {"layout": {"type": "string", "enum": ["cover"]},
                            "title": {"type": "string"},
                            "subtitle": {"type": "string"}},
             "required": ["layout", "title"]},
            {"type": "object",
             "properties": {"layout": {"type": "string", "enum": ["bullets"]},
                            "title": {"type": "string"},
                            "bullets": {"type": "array", "items": {"type": "string"}}},
             "required": ["layout", "title", "bullets"]},
        ]
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"slides": {"type": "array", "items": {"anyOf": branches}}}}))

    def test_property_budget_accumulates_across_anyof_branches(self):
        # Branch-duplicated property names count toward the whole-document
        # properties budget (the validator's MfjsValidationState accumulates
        # across every node): ten branches of eleven properties each exceed
        # the 100 budget even though no single branch does.
        def branch(prefix):
            return {"type": "object",
                    "properties": {f"{prefix}{index}": {"type": "string"}
                                   for index in range(11)},
                    "required": [f"{prefix}0"]}
        message = _contract_message(
            {"type": "object",
             "properties": {"x": {"anyOf": [branch(f"b{i}_") for i in range(10)]}}})
        self.assertIsNotNone(message)
        self.assertIn("exceed the MFJS budget of 100", message)
        # Ten properties per branch (100 total, top-level property included)
        # stays inside the budget.
        def slim_branch(prefix):
            props = {f"{prefix}{index}": {"type": "string"} for index in range(9)}
            return {"type": "object", "properties": props,
                    "required": [f"{prefix}0"]}
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"x": {"anyOf": [slim_branch(f"b{i}_") for i in range(10)]}}}))

    def test_total_properties_budget_is_enforced(self):
        def schema_with(count):
            return {"type": "object",
                    "properties": {f"p{index}": {"type": "string"}
                                   for index in range(count)}}
        message = _contract_message(schema_with(101))
        self.assertIsNotNone(message)
        self.assertIn("exceed the MFJS budget of 100", message)
        self.assertIsNone(_contract_message(schema_with(100)))

    def test_property_depth_budget_is_enforced(self):
        def nested(depth):
            node = {"type": "string"}
            for _ in range(depth):
                node = {"type": "object", "properties": {"inner": node}}
            return node
        message = _contract_message(nested(6))
        self.assertIsNotNone(message)
        self.assertIn("nesting deeper than 5", message)
        self.assertIsNone(_contract_message(nested(5)))

    def test_total_enum_budget_is_enforced(self):
        enum = [f"v{index}" for index in range(501)]
        message = _contract_message(
            {"type": "object", "properties": {"e": {"type": "string", "enum": enum}}})
        self.assertIsNotNone(message)
        self.assertIn("exceed the MFJS budget of 500", message)

    def test_enum_literal_length_budget_is_enforced(self):
        # validate_mfjs_enum's conditional budget: an enum with more than 250
        # entries may not carry more than 7500 combined literal bytes. 251
        # distinct 100-character values sit below the 500-value and
        # 120000-byte caps, so only this budget rejects them.
        enum = [f"{index:03d}" + "x" * 97 for index in range(251)]
        message = _contract_message(
            {"type": "object", "properties": {"e": {"type": "string", "enum": enum}}})
        self.assertIsNotNone(message)
        self.assertIn("literal bytes", message)
        self.assertIn("above 250 entries at 7500", message)
        # Both sides of the length boundary at 251 entries: exactly 7500
        # (250 * 29 + 250) stays green, one more byte goes red.
        def enum_at(last_len):
            values = ["x" * 29] * 250 + ["y" * last_len]
            return {"type": "object",
                    "properties": {"e": {"type": "string", "enum": values}}}
        self.assertIsNone(_contract_message(enum_at(250)))
        message = _contract_message(enum_at(251))
        self.assertIsNotNone(message)
        self.assertIn("literal bytes", message)

    def test_enum_length_budget_starts_above_the_entry_threshold(self):
        # 250 entries carry no aggregate literal cap; only the whole-schema
        # byte budget applies below the 251st entry.
        enum = ["x" * 100] * 250
        self.assertIsNone(_contract_message(
            {"type": "object", "properties": {"e": {"type": "string", "enum": enum}}}))

    def test_encoded_bytes_budget_is_enforced(self):
        schema = {"type": "object",
                  "properties": {"d": {"type": "string",
                                       "description": "x" * (smoke._MFJS_MAX_SCHEMA_BYTES + 100)}}}
        message = _contract_message(schema)
        self.assertIsNotNone(message)
        self.assertIn("over the MFJS budget of", message)

    def test_typed_enum_and_leaf_nodes_are_green(self):
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"e": {"type": "string", "enum": ["a", "b"]},
                            "s": {"type": "string"}},
             "additionalProperties": False}))

    @unittest.skipIf(pptx_schema is None, "python-pptx not installed (cannot load the pptx server schema)")
    def test_real_pptx_schema_is_green(self):
        self.assertIsNone(_contract_message(pptx_schema))

    @unittest.skipIf(pptx_schema is None, "python-pptx not installed (cannot load the pptx server schema)")
    def test_real_pptx_schema_items_is_per_layout_anyof(self):
        # The 2026-11-04 mitigation shape: slides.items is an anyOf union of
        # exactly one explicit type:object branch per layout. A regression
        # to the flat single-object items (whose required was only layout,
        # so constrained decoding dropped every content field) must turn
        # red here.
        slides = pptx_schema["properties"]["slides"]
        self.assertEqual(slides.get("type"), "array")
        items = slides.get("items")
        self.assertIsInstance(items, dict)
        self.assertNotIn("type", items,
                         "anyOf must not share its level with type (MFJS InvalidKeywordValue)")
        branches = items.get("anyOf")
        self.assertIsInstance(branches, list)
        self.assertEqual(len(branches), 10,
                         "one branch per layout; an 11th also exceeds the anyOf budget")
        layouts = []
        for index, branch in enumerate(branches):
            self.assertEqual(branch.get("type"), "object",
                             f"branch {index} must be an explicit type:object")
            props = branch.get("properties")
            required = branch.get("required")
            self.assertIsInstance(props, dict)
            self.assertIsInstance(required, list)
            self.assertTrue(required, f"branch {index} must declare required")
            self.assertTrue(set(required) <= set(props),
                            f"branch {index} required must name declared properties")
            enum = props.get("layout", {}).get("enum")
            self.assertEqual(len(enum), 1,
                             f"branch {index} layout enum must be a single value")
            layouts.append(enum[0])
        self.assertEqual(sorted(layouts),
                         sorted(["cover", "agenda", "section", "bullets", "two_col",
                                 "kpi", "chart", "table", "image", "end"]))

    @unittest.skipIf(pptx_schema is None, "python-pptx not installed (cannot load the pptx server schema)")
    def test_real_pptx_schema_anyof_budgets_measured(self):
        # The union duplicates layout/title across its 10 branches, so the
        # whole-document properties total is what the budget sees. Measure
        # it the way the guard's _MfjsBudget does (every properties dict,
        # recursed through anyOf/items/additionalProperties) and pin it
        # inside the budget: at the 2026-11-04 shape it is 47 of 100 —
        # headroom for ~5 more layouts at most, after which a branch merge
        # or a budget widening is required, not silent growth.
        def count(node):
            total = 0
            if not isinstance(node, dict):
                return total
            props = node.get("properties")
            if isinstance(props, dict):
                total += len(props)
                for sub in props.values():
                    total += count(sub)
            for key in ("items", "additionalProperties"):
                sub = node.get(key)
                if isinstance(sub, dict):
                    total += count(sub)
                elif isinstance(sub, list):
                    total += sum(count(entry) for entry in sub)
            branches = node.get("anyOf")
            if isinstance(branches, list):
                total += sum(count(branch) for branch in branches)
            return total
        total = count(pptx_schema)
        self.assertLessEqual(total, smoke._MFJS_MAX_TOTAL_PROPERTIES,
                             f"pptx schema holds {total} properties — over budget; "
                             "merge branches or widen deliberately")


if __name__ == "__main__":
    unittest.main()
