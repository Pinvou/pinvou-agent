"""Unit tests for the model-visible schema guard in mcp-server-contract-smoke.

The guard (check_schema_contract/_walk_tool_schema) runs inside the CI
fast-gate's mcp-server-contract-smoke.py, shape-checking every bundled
server's tools/list result against the MFJS shape set verified from the
validator source (CodeWhale schema_sanitize.rs: ALLOWED_KEYWORDS plus the
MissingType / UnsupportedKeyword / InvalidSchemaNode raise sites): every
schema position holds an object, "type" is a single concrete MFJS string
(or absent in favor of anyOf with union-only siblings), and no keyword
outside MFJS's allowed set survives — the validator drops the whole tool
for any of these, so a bundled server that ships one loses that tool
silently on the constrained-decoding route. All bundled servers are
currently compliant, so if the guard itself breaks the smoke stays green
(against compliant schemas a broken guard is exactly as silent as a good
one) — only the direct unit tests here turn red when the guard rots. Both
directions are pinned: the incident shape (itemless array), items:{},
untyped items, tuple items, boolean schemas, list-typed / non-string /
unknown "type" values, typeless annotation-only nodes, non-object schema
values, $ref/$dynamicRef/$recursiveRef, empty or sibling-polluted anyOf,
non-root $defs/$id, and every non-MFJS keyword (oneOf, allOf,
patternProperties, prefixItems, contains, propertyNames, not/if/then/else,
dependentSchemas, draft-07 dependencies, unevaluated*, contentSchema) must
be red; typed enums, typed unions, and the real pptx schema must be green.
Every recursion lane that survives (properties, additionalProperties,
items, anyOf branches, $defs) has a red case pinning it: the walker's
recursion is not decorative, every lane is consumed by an assertion. The
wiring (the call site itself) is pinned by check_schema_contract's
return-value contract: check_protocol asserts the returned count against
the boot contract and prints it into the success line, which raises the
moment the call site is deleted; the unit test pins the return value.

The real-pptx-schema green case needs python-pptx to import the server
module; in the fast-gate, unittest discover runs before python-pptx is
installed, so that case skips in dependency-free environments — CI's green
coverage of the real schema comes from the smoke step later in the same job
(a real stdio pptx boot run through the same guard); what this file adds is
local full-depth coverage.
"""
import importlib.util
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

try:
    pptx_server = _load_module("pptx_server_under_test", PPTX_SERVER)
except Exception:  # python-pptx not installed (fast-gate discover runs before deps)
    pptx_server = None


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
        # MFJS allows only title/description beside anyOf: a typed node with
        # a union is InvalidKeywordValue, not an intersection.
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

    def test_list_typed_inside_additional_properties_is_red(self):
        # The ban must hold at every recursion lane, not just properties.
        message = _contract_message(
            {"type": "object", "additionalProperties": {"type": ["array", "string"]}})
        self.assertIsNotNone(message)
        self.assertIn("additionalProperties", message)
        self.assertIn("list-typed", message)

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
        # tests cannot pin the call site.
        walked = smoke.check_schema_contract("t", [
            {"name": "a", "inputSchema": {"type": "string"}},
            {"name": "b", "inputSchema": {"type": "array",
                                          "items": {"type": "string"}}},
        ])
        self.assertEqual(walked, 2)

    def test_non_object_input_schema_is_red(self):
        message = _contract_message("not-a-schema")
        self.assertIsNotNone(message)
        self.assertIn("inputSchema", message)

    def test_typed_enum_and_leaf_nodes_are_green(self):
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"e": {"type": "string", "enum": ["a", "b"]},
                            "s": {"type": "string"}},
             "additionalProperties": False}))

    @unittest.skipIf(pptx_server is None, "python-pptx not installed (cannot import the pptx server module)")
    def test_real_pptx_schema_is_green(self):
        self.assertIsNone(_contract_message(pptx_server.TOOL_DEFS[0]["inputSchema"]))


if __name__ == "__main__":
    unittest.main()
