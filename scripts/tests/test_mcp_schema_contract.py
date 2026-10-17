"""Unit tests for the model-visible schema guard in mcp-server-contract-smoke.

The guard (check_schema_contract/_walk_tool_schema) runs inside the CI
fast-gate's mcp-server-contract-smoke.py, shape-checking every bundled
server's tools/list result. All bundled servers are currently compliant, so
if the guard itself breaks the smoke stays green (against compliant schemas
a broken guard is exactly as silent as a good one) — only the direct unit
tests here turn red when the guard rots. Both directions are pinned: the
incident shape (itemless array), items:{}, untyped items, tuple items,
prefixItems, boolean schemas, anyOf with an empty branch, list-typed "type"
keywords, and $ref must be red; fully typed oneOf/allOf and the real pptx
schema must be green. Every recursion lane — properties,
additionalProperties, anyOf/oneOf branches, $defs/definitions, contains,
propertyNames, not, if/then/else, dependentSchemas, draft-07 dependencies,
unevaluatedProperties/unevaluatedItems — has a red case pinning it: the
walker's recursion is not decorative, every lane is consumed by an
assertion. The wiring (the call site itself) is pinned by
check_schema_contract's return-value contract: check_protocol prints it
into the success line, which raises NameError the moment the call site is
deleted; the unit test pins the return value.

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

    def test_fully_typed_one_of_items_is_green(self):
        self.assertIsNone(_contract_message(
            {"type": "array",
             "items": {"oneOf": [{"type": "string"}, {"type": "null"}]}}))

    def test_fully_typed_all_of_items_is_green(self):
        self.assertIsNone(_contract_message(
            {"type": "array",
             "items": {"allOf": [{"type": "string"}, {"maxLength": 5}]}}))

    def test_dollar_ref_is_red_as_property_and_as_items(self):
        message = _contract_message(
            {"type": "object",
             "definitions": {"w": {"type": "string"}},
             "properties": {"a": {"$ref": "#/definitions/w"},
                            "b": {"type": "array", "items": {"$ref": "#/definitions/w"}}}})
        self.assertIsNotNone(message)
        self.assertIn("$ref", message)

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

    def test_prefix_items_is_red(self):
        # Tuple semantics, same family as tuple items: outside the verified
        # MFJS shape set.
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
        # Pin the anyOf/oneOf branch recursion lane: any branch of a union
        # may be taken.
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

    def test_itemless_array_inside_defs_is_red(self):
        # $defs and draft-07 definitions both recurse; an unreferenced
        # definition must not be a blind spot.
        message = _contract_message(
            {"type": "object",
             "$defs": {"a": {"type": "array"}},
             "definitions": {"b": {"type": "array"}}})
        self.assertIsNotNone(message)
        self.assertIn("$defs.a", message)
        self.assertIn("definitions.b", message)

    def test_itemless_array_inside_contains_is_red(self):
        message = _contract_message({"type": "array", "items": {"type": "string"}, "contains": {"type": "array"}})
        self.assertIsNotNone(message)
        self.assertIn("contains", message)

    def test_itemless_array_inside_property_names_is_red(self):
        message = _contract_message({"type": "object", "propertyNames": {"type": "array"}})
        self.assertIsNotNone(message)
        self.assertIn("propertyNames", message)

    def test_itemless_array_inside_not_and_conditionals_is_red(self):
        message = _contract_message(
            {"type": "object",
             "not": {"type": "array"},
             "if": {"type": "array"},
             "then": {"type": "array"},
             "else": {"type": "array"}})
        self.assertIsNotNone(message)
        for lane in (".not", ".if", ".then", ".else"):
            self.assertIn(lane, message)

    def test_itemless_array_inside_dependent_schemas_is_red(self):
        message = _contract_message(
            {"type": "object",
             "dependentSchemas": {"a": {"type": "array"}},
             "dependencies": {"b": {"type": "array"}, "c": ["x"]}})
        self.assertIsNotNone(message)
        self.assertIn("dependentSchemas.a", message)
        self.assertIn("dependencies.b", message)
        # Property-name-list form of draft-07 dependencies is data: absent
        # from the offenders on purpose.
        self.assertNotIn("dependencies.c", message)

    def test_itemless_array_inside_unevaluated_keywords_is_red(self):
        message = _contract_message(
            {"type": "object",
             "unevaluatedProperties": {"type": "array"},
             "unevaluatedItems": {"type": "array"}})
        self.assertIsNotNone(message)
        self.assertIn("unevaluatedProperties", message)
        self.assertIn("unevaluatedItems", message)

    def test_guard_wiring_returns_walked_tool_count(self):
        # Wiring pin: check_protocol consumes this return value to print the
        # success line, which raises NameError the moment the call site is
        # deleted — the walker's own unit tests cannot pin the call site.
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

    def test_enum_only_and_leaf_nodes_are_green(self):
        self.assertIsNone(_contract_message(
            {"type": "object",
             "properties": {"e": {"enum": ["a", "b"]}, "s": {"type": "string"}},
             "additionalProperties": False}))

    @unittest.skipIf(pptx_server is None, "python-pptx not installed (cannot import the pptx server module)")
    def test_real_pptx_schema_is_green(self):
        self.assertIsNone(_contract_message(pptx_server.TOOL_DEFS[0]["inputSchema"]))


if __name__ == "__main__":
    unittest.main()
