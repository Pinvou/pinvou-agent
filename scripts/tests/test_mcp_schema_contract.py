"""mcp-server-contract-smoke 模型可见 schema 守卫自身的单测。

守卫(check_schema_contract/_walk_tool_schema)跑在 CI fast-gate 的
mcp-server-contract-smoke.py 里,对全部 bundled server 的 tools/list 结果做
形状检查。所有 bundled server 目前都合规——守卫自身若被改坏,smoke 依旧
全绿(对合规 schema,坏守卫与好守卫同样沉默),只有这里的直接单测能在
守卫腐坏时爆红。红/绿两方向都钉:事故形状(数组无 items)、items:{}、
无类型 items、tuple items、prefixItems、boolean schema、anyOf 空分支、
$ref 必须红;全类型 oneOf/allOf、真实 pptx schema 必须绿。递归通道
(additionalProperties、anyOf 分支内部)各自有一个红例钉住——walker 的
递归不是摆设,而是每条通道都被断言消费。接线(调用点本身)由
check_schema_contract 的返回值契约钉住:check_protocol 把它打进成功行,
调用点被删时该行立即 NameError,单测钉返回值。

真实 pptx schema 的绿例需要 python-pptx 才能导入 server 模块;fast-gate 里
unittest discover 先于 python-pptx 安装执行,故该例在有依赖的环境跳过——
CI 对真实 schema 的绿覆盖由同一 job 里后面的 smoke 步骤(真实 stdio 启动
pptx 并跑同一守卫)提供,这里补的是本地全量深度。
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
except Exception:  # python-pptx 未装(fast-gate discover 步骤先于依赖安装)
    pptx_server = None


def _contract_message(schema):
    """跑守卫,返回违例清单文本;合规返回 None。"""
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
        self.assertIn("缺少带具体类型的 items", message)
        self.assertIn("slides", message)

    def test_empty_items_node_is_red(self):
        message = _contract_message({"type": "array", "items": {}})
        self.assertIsNotNone(message)
        self.assertIn("空 schema 节点", message)

    def test_untyped_items_node_is_red(self):
        message = _contract_message({"type": "array", "items": {"description": "x"}})
        self.assertIsNotNone(message)
        self.assertIn("缺少带具体类型的 items", message)

    def test_tuple_items_is_red(self):
        message = _contract_message({"type": "array", "items": [{"type": "string"}]})
        self.assertIsNotNone(message)
        self.assertIn("tuple", message)

    def test_union_with_untyped_branch_is_red(self):
        message = _contract_message(
            {"type": "array",
             "items": {"anyOf": [{"type": "string"}, {"description": "x"}]}})
        self.assertIsNotNone(message)
        self.assertIn("缺少带具体类型的 items", message)

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
        self.assertIn("缺少带具体类型的 items", message)

    def test_empty_property_node_is_red(self):
        message = _contract_message({"type": "object", "properties": {"x": {}}})
        self.assertIsNotNone(message)
        self.assertIn("空 schema 节点", message)

    def test_boolean_schema_node_is_red(self):
        # true 是空 schema 的语义等价物:与 {} 同类,必须红。
        message = _contract_message({"type": "object", "properties": {"x": True}})
        self.assertIsNotNone(message)
        self.assertIn("boolean schema", message)

    def test_prefix_items_is_red(self):
        # 元组语义与 tuple items 同族,超出已验证的 MFJS 形状集。
        message = _contract_message(
            {"type": "array", "items": {"type": "string"},
             "prefixItems": [{"type": "string"}]})
        self.assertIsNotNone(message)
        self.assertIn("prefixItems", message)

    def test_itemless_array_inside_additional_properties_is_red(self):
        # 钉 additionalProperties 递归通道:删掉这条递归,此例必须翻红不了。
        message = _contract_message(
            {"type": "object", "additionalProperties": {"type": "array"}})
        self.assertIsNotNone(message)
        self.assertIn("additionalProperties", message)
        self.assertIn("缺少带具体类型的 items", message)

    def test_itemless_array_inside_union_branch_is_red(self):
        # 钉 anyOf/oneOf 分支递归通道:并集任一分支都可能被取到。
        message = _contract_message(
            {"type": "object",
             "properties": {"x": {"anyOf": [{"type": "array"}, {"type": "string"}]}}})
        self.assertIsNotNone(message)
        self.assertIn("anyOf[0]", message)
        self.assertIn("缺少带具体类型的 items", message)

    def test_guard_wiring_returns_walked_tool_count(self):
        # 接线钉:check_protocol 消费这个返回值打印成功行,调用点被删时
        # 那行 print 立即 NameError——walker 内部单测钉不住调用点本身。
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

    @unittest.skipIf(pptx_server is None, "python-pptx 未安装(无法导入 pptx server 模块)")
    def test_real_pptx_schema_is_green(self):
        self.assertIsNone(_contract_message(pptx_server.TOOL_DEFS[0]["inputSchema"]))


if __name__ == "__main__":
    unittest.main()
