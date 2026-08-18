import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HEADLESS_COMPOSITION_ROOTS = (
    ROOT / "pinvou3-app/src-tauri/src/eval_cli.rs",
    ROOT / "pinvou3-app/src-tauri/src/headless_bridge.rs",
)
EVAL_TOOL_POLICY = (
    ROOT
    / "pinvou3-app/src-tauri/src/features/assistant/product_runtime/eval_tool_policy.rs"
)


class HeadlessBootContractTests(unittest.TestCase):
    def test_headless_composition_roots_do_not_restore_retired_skill_bindings(self):
        for source_path in HEADLESS_COMPOSITION_ROOTS:
            with self.subTest(source=source_path.name):
                source = source_path.read_text(encoding="utf-8")
                self.assertNotIn("load_skill_bindings", source)

    def test_gaia_policy_uses_codewhale_canonical_tool_families(self):
        production = EVAL_TOOL_POLICY.read_text(encoding="utf-8").split("#[cfg(test)]", 1)[0]

        for canonical in ['"File"', '"Web"', '"image_analyze"']:
            self.assertIn(canonical, production)
        for retired in [
            '"read_file"',
            '"list_dir"',
            '"grep_files"',
            '"file_search"',
            '"web_search"',
            '"fetch_url"',
        ]:
            self.assertNotIn(retired, production)


if __name__ == "__main__":
    unittest.main()
