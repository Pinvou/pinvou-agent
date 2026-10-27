"""Trusted fixtures exercise post-freeze signature/keyword diagnostics."""

import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location("extra_api", SCRIPTS / "prompt_eval_extra_api.py")
api = importlib.util.module_from_spec(spec)
spec.loader.exec_module(api)


class ApiDiagnosticTests(unittest.TestCase):
    def run_fixture(self, source, function="shipping_fee"):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.py"
            path.write_text(source)
            # These are trusted, fixed unit fixtures only. Actual model output
            # always uses the helper's read-only, networkless verifier.
            with patch.object(api.VerifierSubprocess, "run", side_effect=subprocess.run):
                return api.check(path, function)

    def test_original_keyword_api_allows_annotations_and_docstrings(self):
        self.assertEqual(self.run_fixture(
            'def shipping_fee(items: int) -> int:\n    """Fee."""\n    return items * 100\n'),
            {"signature_preserved": True, "keyword_calls_correct": True})

    def test_renamed_parameter_breaks_keyword_call_despite_positional_correctness(self):
        self.assertEqual(self.run_fixture('def shipping_fee(quantity_items):\n    return quantity_items * 100\n'),
                         {"signature_preserved": False, "keyword_calls_correct": False})

    def test_positional_only_change_is_not_a_preserved_api(self):
        self.assertEqual(self.run_fixture('def shipping_fee(items, /):\n    return items * 100\n'),
                         {"signature_preserved": False, "keyword_calls_correct": False})

    def test_added_default_is_distinct_from_keyword_compatibility(self):
        self.assertEqual(self.run_fixture('def shipping_fee(items=0):\n    return items * 100\n'),
                         {"signature_preserved": False, "keyword_calls_correct": True})

    def test_correct_signature_does_not_mask_bad_calculation(self):
        self.assertEqual(self.run_fixture('def shipping_fee(items):\n    return items + 100\n'),
                         {"signature_preserved": True, "keyword_calls_correct": False})

    def test_other_original_keyword_contracts(self):
        fixtures = {
            "discount": "def discount(price, percent):\n    return price * (100 - percent) / 100\n",
            "total_fee": "def total_fee(count):\n    return count * 7\n",
            "merge_config": "import copy\ndef merge_config(defaults, overrides):\n    result=copy.deepcopy(defaults)\n    for key, value in overrides.items():\n        result[key]=merge_config(result[key], value) if isinstance(result.get(key), dict) and isinstance(value, dict) else copy.deepcopy(value)\n    return result\n",
        }
        for function, source in fixtures.items():
            with self.subTest(function=function):
                self.assertEqual(self.run_fixture(source, function),
                                 {"signature_preserved": True, "keyword_calls_correct": True})

    def test_early_exit_or_import_error_cannot_report_success(self):
        for source in ("raise SystemExit(0)\n", "raise RuntimeError('fixture import failure')\n"):
            with self.subTest(source=source):
                self.assertEqual(self.run_fixture(source),
                                 {"signature_preserved": False, "keyword_calls_correct": False})

    def test_timeout_and_unknown_contract_fail_closed(self):
        with patch.object(api.VerifierSubprocess, "run", side_effect=subprocess.TimeoutExpired("fixture", 15)):
            self.assertEqual(api.check("fixture.py", "shipping_fee"),
                             {"signature_preserved": False, "keyword_calls_correct": False})
        with self.assertRaises(ValueError):
            api.check("fixture.py", "unregistered_function")


if __name__ == "__main__":
    unittest.main()
