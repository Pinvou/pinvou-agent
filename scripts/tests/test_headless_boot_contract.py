import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HEADLESS_COMPOSITION_ROOTS = (
    ROOT / "pinvou3-app/src-tauri/src/eval_cli.rs",
    ROOT / "pinvou3-app/src-tauri/src/headless_bridge.rs",
)


class HeadlessBootContractTests(unittest.TestCase):
    def test_headless_composition_roots_do_not_restore_retired_skill_bindings(self):
        for source_path in HEADLESS_COMPOSITION_ROOTS:
            with self.subTest(source=source_path.name):
                source = source_path.read_text(encoding="utf-8")
                self.assertNotIn("load_skill_bindings", source)


if __name__ == "__main__":
    unittest.main()
