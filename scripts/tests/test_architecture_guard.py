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

            rules = self.guard.scan_frontend(root)
            self.assertEqual(1, sum(rules["frontend_feature_imports_app"].values()))
            self.assertEqual(1, sum(rules["frontend_tauri_global_outside_platform"].values()))
            self.assertEqual(1, sum(rules["frontend_user_agent_platform_detection"].values()))

    def test_rust_scanner_finds_upward_dependency_cycle_and_platform_leaks(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            rust_root = root / "pinvou3-app/src-tauri/src"
            feature_a = rust_root / "features/a/mod.rs"
            feature_b = rust_root / "features/b/mod.rs"
            platform = rust_root / "features/a/platform/windows.rs"
            platform_service = rust_root / "platform/service.rs"
            for path in [feature_a, feature_b, platform, platform_service]:
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
            platform.write_text(
                '#[cfg(target_os = "windows")]\n'
                'fn allowed() { Command::new("xdg-open"); }\n',
                encoding="utf-8",
            )
            platform_service.write_text(
                "fn reverse() { crate::feature_a::run(); }\n", encoding="utf-8"
            )

            rules, cycles = self.guard.scan_rust(root)
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
                rules["rust_tauri_commands_outside_app"][
                    "pinvou3-app/src-tauri/src/features/a/mod.rs"
                ],
            )

    def test_platform_rule_exceptions_require_header_marker_and_reason(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            rust_root = root / "pinvou3-app/src-tauri/src"
            allowed = rust_root / "features/allowed/mod.rs"
            rejected = rust_root / "features/rejected/mod.rs"
            late = rust_root / "features/late/mod.rs"
            allowed.parent.mkdir(parents=True)
            rejected.parent.mkdir(parents=True)
            late.parent.mkdir(parents=True)
            allowed.write_text(
                "// architecture-guard: allow-target-cfg -- dependency is Windows-only\n"
                "// architecture-guard: allow-platform-detail -- parses external command\n"
                '#[cfg(target_os = "windows")]\n'
                'fn run() { Command::new("powershell.exe"); }\n',
                encoding="utf-8",
            )
            late.write_text(
                "// header filler\n" * 20
                + "// architecture-guard: allow-target-cfg -- too late\n"
                + "// architecture-guard: allow-platform-detail -- too late\n"
                + '#[cfg(target_os = "windows")]\n'
                + 'fn run() { Command::new("powershell.exe"); }\n',
                encoding="utf-8",
            )
            rejected.write_text(
                "// architecture-guard: allow-target-cfg\n"
                "// architecture-guard: allow-platform-detail --\n"
                '#[cfg(target_os = "windows")]\n'
                'fn run() { Command::new("powershell.exe"); }\n',
                encoding="utf-8",
            )

            rules, _ = self.guard.scan_rust(root)

            self.assertNotIn(
                "pinvou3-app/src-tauri/src/features/allowed/mod.rs",
                rules["rust_target_cfg_outside_adapter"],
            )
            self.assertNotIn(
                "pinvou3-app/src-tauri/src/features/allowed/mod.rs",
                rules["rust_platform_details_outside_adapter"],
            )
            self.assertEqual(
                1,
                rules["rust_target_cfg_outside_adapter"][
                    "pinvou3-app/src-tauri/src/features/rejected/mod.rs"
                ],
            )
            self.assertEqual(
                1,
                rules["rust_platform_details_outside_adapter"][
                    "pinvou3-app/src-tauri/src/features/rejected/mod.rs"
                ],
            )
            self.assertEqual(
                1,
                rules["rust_target_cfg_outside_adapter"][
                    "pinvou3-app/src-tauri/src/features/late/mod.rs"
                ],
            )
            self.assertEqual(
                1,
                rules["rust_platform_details_outside_adapter"][
                    "pinvou3-app/src-tauri/src/features/late/mod.rs"
                ],
            )

    def test_rust_scanner_rejects_external_group_kill_spawn(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            rust_root = root / "pinvou3-app/src-tauri/src"
            platform = rust_root / "platform/process.rs"
            platform.parent.mkdir(parents=True)
            platform.write_text(
                'fn run() {\n'
                '    Command::new("kill");\n'
                '    Command::new("/usr/bin/kill");\n'
                '    Path::new("kill");\n'
                '    connector_cli_command(cmd, "kill");\n'
                '    Command::new("taskkill");\n'
                '}\n',
                encoding="utf-8",
            )

            rules, _ = self.guard.scan_rust(root)

            # The generic `::new("kill")` pattern overlaps the Command/Path
            # specific ones, so those three shapes count twice each.
            self.assertEqual(
                7,
                rules["rust_external_group_kill_spawn"][
                    "pinvou3-app/src-tauri/src/platform/process.rs"
                ],
            )

    def test_external_group_kill_stub_exception_requires_header_marker_and_reason(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            rust_root = root / "pinvou3-app/src-tauri/src"
            allowed = rust_root / "platform/allowed.rs"
            rejected = rust_root / "platform/rejected.rs"
            late = rust_root / "platform/late.rs"
            for path in [allowed, rejected, late]:
                path.parent.mkdir(parents=True, exist_ok=True)
            probe = 'fn run() { Command::new("kill"); }\n'
            allowed.write_text(
                "// architecture-guard: allow-external-group-kill-stub"
                " -- compile-only contract stub, no real platform binds it\n"
                + probe,
                encoding="utf-8",
            )
            late.write_text(
                "// header filler\n" * 20
                + "// architecture-guard: allow-external-group-kill-stub -- too late\n"
                + probe,
                encoding="utf-8",
            )
            rejected.write_text(
                "// architecture-guard: allow-external-group-kill-stub\n" + probe,
                encoding="utf-8",
            )

            rules, _ = self.guard.scan_rust(root)

            self.assertNotIn(
                "pinvou3-app/src-tauri/src/platform/allowed.rs",
                rules["rust_external_group_kill_spawn"],
            )
            # The probe hits both the Command-specific and the generic
            # `::new("kill")` pattern, so it counts twice per file.
            self.assertEqual(
                2,
                rules["rust_external_group_kill_spawn"][
                    "pinvou3-app/src-tauri/src/platform/rejected.rs"
                ],
            )
            self.assertEqual(
                2,
                rules["rust_external_group_kill_spawn"][
                    "pinvou3-app/src-tauri/src/platform/late.rs"
                ],
            )

    def test_guard_does_not_track_file_line_counts(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            frontend = root / "pinvou3-app/src/features/sample/view.jsx"
            rust = root / "pinvou3-app/src-tauri/src/features/sample/mod.rs"
            frontend.parent.mkdir(parents=True)
            rust.parent.mkdir(parents=True)
            frontend.write_text("const value = 1;\n" * 2000, encoding="utf-8")
            rust.write_text("fn value() {}\n" * 2000, encoding="utf-8")

            frontend_rules = self.guard.scan_frontend(root)
            rust_rules, _ = self.guard.scan_rust(root)

            self.assertFalse(any("line" in rule for rule in frontend_rules))
            self.assertFalse(any("line" in rule for rule in rust_rules))

    def test_resource_scanner_rejects_platform_binaries_in_common(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            common = root / "pinvou3-app/src-tauri/resources/common/bundle/bin"
            common.mkdir(parents=True)
            (common / "tool.exe").write_bytes(b"MZ\x00\x00")
            (common / "tool").write_bytes(b"\x7fELF")
            generated = (
                root
                / "pinvou3-app/src-tauri/resources/common/generated/node_modules/tool"
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
