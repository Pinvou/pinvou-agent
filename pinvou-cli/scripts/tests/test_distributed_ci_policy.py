import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/pr-check.yml"


class DistributedCiPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_boundary_build_is_required_and_desktop_free(self):
        changes = self.workflow.split("\n  changes:", maxsplit=1)[1].split(
            "\n  fast-gate:", maxsplit=1
        )[0]
        self.assertIn("distributed_stage1:", changes)
        self.assertIn("- 'pinvou-cli/**'", changes)
        self.assertIn("- '.github/workflows/pr-check.yml'", changes)
        self.assertIn("- 'AGENTS.md'", changes)

        job = self.workflow.split(
            "\n  distributed-boundary-build:", maxsplit=1
        )[1].split("\n  benchmark-test:", maxsplit=1)[0]
        self.assertIn("needs: changes", job)
        self.assertIn("needs.changes.outputs.distributed_stage1 == 'true'", job)
        self.assertIn("python3 pinvou-cli/scripts/check_stage1_zero_diff.py", job)
        self.assertIn("python3 pinvou-cli/scripts/check_distributed_dependencies.py", job)
        self.assertIn("git submodule update --init --recursive -- CodeWhale", job)
        self.assertIn("cargo check", job)
        self.assertIn("cargo test", job)
        self.assertIn("--no-default-features", job)
        self.assertIn("--features pinvou-cli/distributed", job)
        for package in (
            "pinvou-cli",
            "pinvou-controller",
            "pinvou-node",
            "pinvou-protocol",
            "pinvou-seglog",
            "pinvou-runtime-api",
            "pinvou-agent-adapter-codex",
        ):
            self.assertIn(f"-p {package}", job)
        self.assertNotIn("pinvou3-app/src-tauri/Cargo.toml", job)
        self.assertNotIn("cargo check --workspace", job)
        self.assertNotIn("cargo test --workspace", job)

        required_gate = self.workflow.split("\n  required-gate:", maxsplit=1)[1]
        self.assertIn("- distributed-boundary-build", required_gate)
        self.assertIn(
            '"distributed-boundary-build:$DISTRIBUTED_BOUNDARY_BUILD_RESULT"',
            required_gate,
        )


if __name__ == "__main__":
    unittest.main()
