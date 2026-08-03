import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPO_ROOT / "scripts" / "architecture-guard.py"


def load_guard_module():
    spec = importlib.util.spec_from_file_location("architecture_guard", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class ArchitectureGuardUnitTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.guard = load_guard_module()

    def test_count_baseline_detects_both_reduction_and_increase(self):
        failures, progress = self.guard.compare_counts(
            "sample", {"legacy": 2}, {"legacy": 3}
        )
        self.assertEqual([], failures)
        self.assertTrue(progress)

        failures, _ = self.guard.compare_counts(
            "sample", {"legacy": 4, "new": 1}, {"legacy": 3}
        )
        self.assertEqual(2, len(failures))
        self.assertTrue(self.guard.should_fail([], progress))
        self.assertTrue(self.guard.should_fail(failures, []))
        self.assertFalse(self.guard.should_fail([], []))

    def test_cycle_baseline_allows_shrinking_but_not_expansion(self):
        baseline = [["assistant", "files", "knowledge", "remote_control"]]
        self.assertTrue(self.guard.cycle_allowed(["assistant", "knowledge"], baseline))
        self.assertFalse(
            self.guard.cycle_allowed(["assistant", "knowledge", "scheduled"], baseline)
        )
        self.assertFalse(self.guard.cycle_allowed(["assistant"], baseline))

    def test_baseline_ratchet_allows_tightening_but_rejects_new_allowance(self):
        previous = {
            "schema_version": 1,
            "rules": {"rule": {"old": 3}},
            "rust_feature_cycles": [["a", "b", "c"]],
        }
        tighter = {
            "schema_version": 1,
            "rules": {"rule": {"old": 2}},
            "rust_feature_cycles": [["a", "b"]],
        }
        self.assertEqual([], self.guard.compare_baseline_ratchet(tighter, previous))

        wider = {
            "schema_version": 1,
            "rules": {"rule": {"old": 4, "new": 1}},
            "rust_feature_cycles": [["a", "b", "c", "d"]],
        }
        failures = self.guard.compare_baseline_ratchet(wider, previous)
        self.assertEqual(3, len(failures))

    def test_large_file_ratchet_can_be_initialized_once_then_cannot_grow(self):
        previous = {
            "schema_version": 1,
            "rules": {},
            "rust_feature_cycles": [],
        }
        initialized = {
            "schema_version": 1,
            "rules": {
                "frontend_large_file_lines": {"pinvou3-app/src/app/main.jsx": 1200},
                "rust_large_file_lines": {"pinvou3-app/src-tauri/src/lib.rs": 1600},
            },
            "rust_feature_cycles": [],
        }
        self.assertEqual(
            [], self.guard.compare_baseline_ratchet(initialized, previous)
        )

        grown = {
            "schema_version": 1,
            "rules": {
                "frontend_large_file_lines": {"pinvou3-app/src/app/main.jsx": 1201},
                "rust_large_file_lines": {"pinvou3-app/src-tauri/src/lib.rs": 1600},
            },
            "rust_feature_cycles": [],
        }
        failures = self.guard.compare_baseline_ratchet(grown, initialized)
        self.assertEqual(1, len(failures))

    def test_frontend_scanner_rejects_feature_to_app_and_native_tauri(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            feature = root / "pinvou3-app/src/features/chat/view.jsx"
            app = root / "pinvou3-app/src/app/helper.js"
            platform = root / "pinvou3-app/src/platform/tauri/client.js"
            feature.parent.mkdir(parents=True)
            app.parent.mkdir(parents=True)
            platform.parent.mkdir(parents=True)
            feature.write_text(
                'import {\n  helper\n} from "../../app/helper.js";\n'
                "window.__TAURI__.core.invoke('x');\n"
                "const os = navigator.platform;\n",
                encoding="utf-8",
            )
            app.write_text("export default {};\n", encoding="utf-8")
            platform.write_text("window.__TAURI__.core.invoke('allowed');\n", encoding="utf-8")

            rules, _ = self.guard.scan_frontend(root)
            self.assertEqual(1, sum(rules["frontend_feature_imports_app"].values()))
            self.assertEqual(1, sum(rules["frontend_tauri_global_outside_platform"].values()))
            self.assertEqual(1, sum(rules["frontend_user_agent_platform_detection"].values()))

            feature.write_text("\n".join(["const value = 1;"] * 1001), encoding="utf-8")
            rules, _ = self.guard.scan_frontend(root)
            self.assertEqual(
                1001,
                rules["frontend_large_file_lines"][
                    "pinvou3-app/src/features/chat/view.jsx"
                ],
            )

    def test_rust_scanner_finds_upward_dependency_cycle_cfg_and_legacy_modules(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            rust_root = root / "pinvou3-app/src-tauri/src"
            feature_a = rust_root / "features/a/mod.rs"
            feature_b = rust_root / "features/b/mod.rs"
            platform = rust_root / "features/a/platform/windows.rs"
            large = rust_root / "features/large.rs"
            platform_service = rust_root / "platform/service.rs"
            commands = rust_root / "app/commands.rs"
            for path in [feature_a, feature_b, large, platform, platform_service, commands]:
                path.parent.mkdir(parents=True, exist_ok=True)
            (rust_root / "lib.rs").write_text(
                '#[path = "app/bridge/mod.rs"]\nmod bridge;\n'
                '#[path = "features/a/mod.rs"]\nmod feature_a;\n'
                '#[path = "features/b/mod.rs"]\nmod feature_b;\n'
                'fn handler() { tauri::generate_handler![commands::ok, features::a::leak,]; }\n',
                encoding="utf-8",
            )
            feature_a.write_text(
                '#[cfg(target_os = "windows")]\n'
                '#[tauri::command]\n'
                '#[cfg(any(windows, target_arch = "aarch64"))]\n'
                '#[cfg_attr(all(unix, target_family = "unix"), allow(dead_code))]\n'
                'fn cfg_macro() { if cfg!(target_env = "msvc") {} }\n'
                "use crate::{bridge, feature_b};\n"
                'fn leaked_platform_detail() { Command::new("powershell.exe"); }\n'
                "fn upward() { bridge::notify(); feature_b::run(); }\n",
                encoding="utf-8",
            )
            feature_b.write_text(
                "fn backward() { crate::feature_a::run(); }\n", encoding="utf-8"
            )
            large.write_text("\n".join(["fn value() {}"] * 1501), encoding="utf-8")
            platform.write_text(
                '#[cfg(target_os = "windows")]\n'
                'fn allowed() { Command::new("xdg-open"); }\n',
                encoding="utf-8",
            )
            platform_service.write_text(
                "fn reverse() { crate::feature_a::run(); }\n", encoding="utf-8"
            )
            commands.write_text('include!("commands/chat.rs");\n', encoding="utf-8")

            rules, cycles, _ = self.guard.scan_rust(root)
            self.assertEqual(
                1501,
                rules["rust_large_file_lines"][
                    "pinvou3-app/src-tauri/src/features/large.rs"
                ],
            )
            self.assertEqual(1, rules["rust_feature_depends_on_app"]["a->bridge"])
            self.assertEqual(
                1,
                rules["rust_platform_depends_on_upper_layer"][
                    "pinvou3-app/src-tauri/src/platform/service.rs->features::a"
                ],
            )
            self.assertEqual(
                1,
                rules["rust_tauri_handler_outside_app"][
                    "pinvou3-app/src-tauri/src/lib.rs:features::a::leak"
                ],
            )
            self.assertEqual([["a", "b"]], cycles)
            self.assertEqual(1, rules["rust_cyclic_feature_dependencies"]["a->b"])
            self.assertEqual(1, rules["rust_cyclic_feature_dependencies"]["b->a"])
            self.assertEqual(
                6,
                rules["rust_target_cfg_outside_adapter"][
                    "pinvou3-app/src-tauri/src/features/a/mod.rs"
                ],
            )
            self.assertNotIn(
                "pinvou3-app/src-tauri/src/features/a/platform/windows.rs",
                rules["rust_target_cfg_outside_adapter"],
            )
            self.assertEqual(
                1,
                rules["rust_platform_details_outside_adapter"][
                    "pinvou3-app/src-tauri/src/features/a/mod.rs"
                ],
            )
            self.assertNotIn(
                "pinvou3-app/src-tauri/src/features/a/platform/windows.rs",
                rules["rust_platform_details_outside_adapter"],
            )
            self.assertEqual(
                1,
                rules["rust_legacy_module_indirection"][
                    "pinvou3-app/src-tauri/src/app/commands.rs:include!"
                ],
            )
            self.assertEqual(
                3,
                rules["rust_legacy_module_indirection"][
                    "pinvou3-app/src-tauri/src/lib.rs:#[path]"
                ],
            )
            self.assertEqual(
                1,
                rules["rust_tauri_commands_outside_app"][
                    "pinvou3-app/src-tauri/src/features/a/mod.rs"
                ],
            )

    def test_resource_scanner_rejects_platform_binaries_in_common(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            common = root / "pinvou3-app/src-tauri/resources/common/bundle/bin"
            common.mkdir(parents=True)
            (common / "tool.exe").write_bytes(b"MZ\x00\x00")
            (common / "tool").write_bytes(b"\x7fELF")
            generated = (
                root
                / "pinvou3-app/src-tauri/resources/common/web-template/node_modules/tool"
            )
            generated.parent.mkdir(parents=True)
            generated.write_bytes(b"\x7fELF")
            platform = root / "pinvou3-app/src-tauri/resources/platforms/linux/x86_64"
            platform.mkdir(parents=True)
            (platform / "allowed").write_bytes(b"\x7fELF")

            rules = self.guard.scan_resources(root)

            self.assertEqual(2, sum(rules["common_platform_binaries"].values()))


class ArchitectureGuardRepositoryTests(unittest.TestCase):
    def test_checked_in_baseline_passes(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT_PATH)],
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
        )
        self.assertEqual(0, result.returncode, result.stdout)
        self.assertIn("architecture guard passed", result.stdout)

    def test_baseline_is_valid_json_with_expected_schema(self):
        baseline = json.loads(
            (REPO_ROOT / "scripts/architecture-baseline.json").read_text(encoding="utf-8")
        )
        self.assertEqual(1, baseline["schema_version"])
        self.assertIn("rules", baseline)
        self.assertIn("rust_feature_cycles", baseline)

if __name__ == "__main__":
    unittest.main()
