import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HEADLESS_COMPOSITION_ROOTS = (
    ROOT
    / "pinvou3-app/src-tauri/src/features/assistant/product_runtime/headless_bridge.rs",
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

    def test_retired_eval_smoke_entrypoints_are_removed(self):
        for retired in (
            ROOT / "pinvou3-app/src-tauri/src/eval_cli.rs",
            ROOT / "pinvou3-app/src-tauri/src/bin/eval_smoke.rs",
            ROOT / "pinvou3-app/src-tauri/src/app/commands/eval.rs",
        ):
            self.assertFalse(retired.exists(), retired)
        cargo = (ROOT / "pinvou3-app/src-tauri/Cargo.toml").read_text(encoding="utf-8")
        lib = (ROOT / "pinvou3-app/src-tauri/src/lib.rs").read_text(encoding="utf-8")
        self.assertNotIn('name = "eval_smoke"', cargo)
        self.assertNotIn("commands::eval::run_eval_smoke", lib)


if __name__ == "__main__":
    unittest.main()
