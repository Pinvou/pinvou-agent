"""Calibrate outcome graders against known good and deliberately bad outcomes."""

import importlib.util
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location(
    "community_cases", Path(__file__).resolve().parents[1] / "prompt_eval_community_cases.py")
cases = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cases)


class CommunityGraderTests(unittest.TestCase):
    def test_equivalent_acceptance_wording_is_allowed(self):
        for text in ["验收状态：已验收\n后续动作：归档", "验收状态：已通过\n后续动作：归档",
                     "验收通过，无需再等待验收", "整体验收已通过"]:
            with self.subTest(text=text):
                self.assertTrue(cases.acceptance_consistent(text))

    def test_acceptance_with_a_pending_next_step_is_rejected(self):
        for text in ["已验收\n下一步：提交验收，等待验收结果", "当前状态：已通过\n等待负责人验收",
                     "验收状态：待验收", "已验收，无需修改标题，但等待验收"]:
            with self.subTest(text=text):
                self.assertFalse(cases.acceptance_consistent(text))

    def test_hidden_oracle_rejects_shallow_merge_and_shared_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "merge.py"
            path.write_text("def merge_config(a, b):\n return dict(a, **b)\n")
            self.assertFalse(cases.merge_oracle(path))
            path.write_text("def merge_config(a, b):\n r=a.copy()\n for k,v in b.items():\n  r[k]=merge_config(r[k],v) if k in r and isinstance(r[k],dict) and isinstance(v,dict) else v\n return r\n")
            self.assertFalse(cases.merge_oracle(path))
            path.write_text("import copy\ndef merge_config(a, b):\n r=copy.deepcopy(a)\n for k,v in b.items():\n  r[k]=merge_config(r[k],v) if k in r and isinstance(r[k],dict) and isinstance(v,dict) else copy.deepcopy(v)\n return r\n")
            self.assertTrue(cases.merge_oracle(path))


if __name__ == "__main__":
    unittest.main()
