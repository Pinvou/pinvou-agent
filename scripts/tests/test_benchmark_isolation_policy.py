import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class BenchmarkIsolationPolicyTests(unittest.TestCase):
    def read(self, relative_path: str) -> str:
        return (ROOT / relative_path).read_text(encoding="utf-8")

    def test_desktop_default_build_does_not_enable_benchmark_runtime(self):
        manifest = self.read("pinvou3-app/src-tauri/Cargo.toml")
        features = manifest.split("[features]", 1)[1].split("[build-dependencies]", 1)[0]

        self.assertIn('default = ["local-embed"]', features)
        self.assertIn(
            'benchmark-hooks = ["dep:agent-backend-api", "deepseek-tui/benchmark-observability", "deepseek-tui/benchmark-eval-controls"]',
            features,
        )
        codewhale_manifest = self.read("CodeWhale/crates/tui/Cargo.toml")
        self.assertIn("benchmark-observability = []", codewhale_manifest)
        self.assertIn("benchmark-eval-controls = []", codewhale_manifest)
        self.assertIn("agent-backend-api = {", manifest)
        self.assertIn("optional = true", manifest)

    def test_benchmark_modules_and_public_entrypoint_are_feature_gated(self):
        assistant_modules = self.read(
            "pinvou3-app/src-tauri/src/features/assistant/mod.rs"
        )
        lib = self.read("pinvou3-app/src-tauri/src/lib.rs")

        self.assertIn(
            '#[cfg(any(feature = "benchmark-hooks", test))]\n'
            "pub(crate) mod product_runtime;",
            assistant_modules,
        )
        self.assertIn(
            '#[cfg(feature = "benchmark-hooks")]\n'
            "pub use features::assistant::product_runtime::{agentic_task, headless_bridge};",
            lib,
        )

    def test_product_backend_is_owned_by_the_standalone_cli_workspace(self):
        cli_manifest = self.read("pinvou-cli/crates/cli/Cargo.toml")
        backend_manifest = self.read(
            "pinvou-cli/crates/pinvou-product-backend/Cargo.toml"
        )
        release_workflow = self.read(".github/workflows/release-packages.yml")

        self.assertIn('default = ["product-backend"]', cli_manifest)
        self.assertIn('features = ["benchmark-hooks", "local-embed"]', backend_manifest)
        self.assertNotIn("pinvou-cli", release_workflow)

    def test_missing_notification_state_changes_only_for_benchmark_builds(self):
        support = self.read(
            "pinvou3-app/src-tauri/src/features/assistant/engine_support.rs"
        )

        self.assertIn(
            '#[cfg(feature = "benchmark-hooks")]\n'
            "    let should_notify = notification_state.unwrap_or_else(|| {",
            support,
        )
        self.assertIn("eval_observation_enabled(session_id)", support)
        self.assertIn(
            '#[cfg(not(feature = "benchmark-hooks"))]\n'
            "    let should_notify = notification_state.unwrap_or(true);",
            support,
        )

    def test_benchmark_observation_fields_do_not_change_default_diagnostics(self):
        timing = self.read("pinvou3-app/src-tauri/src/features/assistant/timing.rs")

        self.assertIn(
            '#[cfg(any(feature = "benchmark-hooks", test))]\n'
            "    pub tool_name: Option<String>,",
            timing,
        )
        self.assertIn(
            'let is_base_event = matches!(event, "user_start" | "assistant_done" | "context_snapshot");',
            timing,
        )
        self.assertIn(
            '#[cfg(not(any(feature = "benchmark-hooks", test)))]\n'
            "    let is_observation_event = false;",
            timing,
        )


if __name__ == "__main__":
    unittest.main()
