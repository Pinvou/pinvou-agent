import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check_stage1_zero_diff.py"
SPEC = importlib.util.spec_from_file_location("stage1_zero_diff_guard", SCRIPT)
guard = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = guard
SPEC.loader.exec_module(guard)


class Stage1ZeroDiffGuardTests(unittest.TestCase):
    def test_allows_only_stage1_owned_paths(self):
        changed = [
            guard.ChangedPath("M", ("pinvou-cli/Cargo.toml",)),
            guard.ChangedPath("M", (".github/workflows/pr-check.yml",)),
            guard.ChangedPath("M", ("AGENTS.md",)),
        ]
        self.assertEqual([], guard.find_violations(changed))

    def test_rejects_desktop_code_and_codewhale_gitlink(self):
        changed = [
            guard.ChangedPath("M", ("pinvou3-app/src/main.jsx",)),
            guard.ChangedPath("M", ("CodeWhale",)),
        ]
        violations = "\n".join(guard.find_violations(changed))
        self.assertIn("pinvou3-app/src/main.jsx", violations)
        self.assertIn("CodeWhale", violations)

    def test_rejects_main_release_and_packaging_inputs(self):
        changed = [
            guard.ChangedPath("M", ("VERSION",)),
            guard.ChangedPath("M", (".github/workflows/release-packages.yml",)),
        ]
        violations = "\n".join(guard.find_violations(changed))
        self.assertIn("VERSION", violations)
        self.assertIn("release-packages.yml", violations)

    def test_rejects_rename_crossing_the_boundary(self):
        changed = guard.parse_name_status_z(
            "R100\0pinvou-cli/old.rs\0pinvou3-app/src-tauri/new.rs\0"
        )
        violations = "\n".join(guard.find_violations(changed))
        self.assertIn("pinvou3-app/src-tauri/new.rs", violations)

    def test_parses_copy_and_regular_records_deterministically(self):
        changed = guard.parse_name_status_z(
            "M\0AGENTS.md\0C075\0pinvou-cli/a.rs\0pinvou-cli/b.rs\0"
        )
        self.assertEqual(
            [
                guard.ChangedPath("M", ("AGENTS.md",)),
                guard.ChangedPath("C075", ("pinvou-cli/a.rs", "pinvou-cli/b.rs")),
            ],
            changed,
        )


if __name__ == "__main__":
    unittest.main()
