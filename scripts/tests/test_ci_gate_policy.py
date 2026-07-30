import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PR_WORKFLOW = ROOT / ".github/workflows/pr-check.yml"
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"


class CiGatePolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.pr_workflow = PR_WORKFLOW.read_text(encoding="utf-8")
        cls.release_workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    def test_full_release_only_runs_for_version_or_manual_trigger(self):
        trigger = self.release_workflow.split("\non:", maxsplit=1)[1].split(
            "\npermissions:", maxsplit=1
        )[0]
        self.assertNotIn("pull_request:", trigger)
        self.assertIn("push:", trigger)
        self.assertIn("paths:\n      - 'VERSION'", trigger)
        self.assertIn("workflow_dispatch:", trigger)
        self.assertIn("cancel-in-progress: false", self.release_workflow)

    def test_pull_request_has_lightweight_release_contract_gate(self):
        self.assertIn("release_contract:", self.pr_workflow)
        self.assertIn("  release-contract-test:", self.pr_workflow)
        self.assertIn(
            "needs.changes.outputs.release_contract == 'true'",
            self.pr_workflow,
        )
        required_gate = self.pr_workflow.split(
            "\n  required-gate:", maxsplit=1
        )[1]
        self.assertIn("- release-contract-test", required_gate)
        self.assertIn(
            '"release-contract-test:$RELEASE_CONTRACT_RESULT"',
            required_gate,
        )

    def test_full_rust_runs_in_merge_queue_or_by_explicit_label(self):
        self.assertIn("merge_group:", self.pr_workflow)
        self.assertIn("ci:full-rust", self.pr_workflow)
        rust_test = self.pr_workflow.split("\n  rust-test:", maxsplit=1)[1].split(
            "\n  windows-rust-test:", maxsplit=1
        )[0]
        self.assertIn("github.event_name == 'merge_group'", rust_test)
        self.assertIn(
            "contains(github.event.pull_request.labels.*.name, 'ci:full-rust')",
            rust_test,
        )
        self.assertIn(
            "github.event_name == 'push' && needs.changes.outputs.rust_code == 'true'",
            rust_test,
        )
        self.assertNotIn(
            "github.event_name == 'push' || needs.changes.outputs.rust_code == 'true'",
            rust_test,
        )

    def test_main_cache_writer_is_not_cancelled(self):
        concurrency = self.pr_workflow.split(
            "\nconcurrency:", maxsplit=1
        )[1].split("\njobs:", maxsplit=1)[0]
        self.assertIn(
            "cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
            concurrency,
        )


if __name__ == "__main__":
    unittest.main()
