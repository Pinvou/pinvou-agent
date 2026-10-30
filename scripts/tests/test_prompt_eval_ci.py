"""CI must prepare grader prerequisites and execute the example harness."""

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


class EvaluationCiContracts(unittest.TestCase):
    def test_secret_scan_sets_up_supported_python_before_importing_tomllib(self):
        workflow = (ROOT / ".github/workflows/secret-scan.yml").read_text()
        steps = re.split(r"(?m)^      - ", workflow)
        control = next(i for i, step in enumerate(steps) if "python3 -m unittest scripts.tests.test_eval_digest_allowlist" in step)
        setup = next((step for step in steps[:control] if "uses: actions/setup-python@" in step), "")
        self.assertTrue(setup, "digest controls import tomllib; hosted system Python is insufficient")
        version = re.search(r'python-version: [\'"]?(3\.(?:x|\d+))', setup)
        self.assertIsNotNone(version)
        self.assertTrue(version[1] == "3.x" or int(version[1].split(".")[1]) >= 11)
        self.assertIn("if: github.event_name != 'merge_group'", setup)

    def test_cli_regression_executes_prompt_example_unit_tests(self):
        workflow = (ROOT / ".github/workflows/pr-check.yml").read_text()
        cli = workflow.split("\n  cli-test:", 1)[1].split("\n  cli-lint:", 1)[0]
        steps = re.split(r"(?m)^      - ", cli)
        execution = next((step for step in steps if "--example prompt_eval" in step), "")
        self.assertIn("cargo test --manifest-path pinvou-cli/Cargo.toml -p pinvou-cli --example prompt_eval --locked", execution)
        self.assertIn("if: github.event_name != 'push'", execution)
        self.assertNotIn("--no-run", execution)

    def test_namespace_dependency_is_ready_before_test_discovery_and_retries_mirrors(self):
        workflow = (ROOT / ".github/workflows/pr-check.yml").read_text()
        prepare = workflow.index("- name: Prepare evaluation namespace controls")
        discover = workflow.index("- name: Gate policy test suite (scripts/tests)")
        self.assertLess(prepare, discover)
        self.assertIn("apt-get -o Acquire::Retries=3 update", workflow[prepare:discover])
        self.assertIn("apt-get -o Acquire::Retries=3 install", workflow[prepare:discover])
        self.assertNotIn("|| true", workflow[prepare:discover])


if __name__ == "__main__":
    unittest.main()
