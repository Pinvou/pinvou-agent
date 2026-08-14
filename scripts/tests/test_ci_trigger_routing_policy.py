import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MAC_BUILD_WORKFLOW = ROOT / ".github/workflows/mac-build.yml"
WRAPPER_WORKFLOW = ROOT / ".github/workflows/rustc-wrapper-smoke.yml"


class CiTriggerRoutingPolicyTests(unittest.TestCase):
    def test_pure_frontend_changes_do_not_trigger_macos_rust_build(self):
        workflow = MAC_BUILD_WORKFLOW.read_text(encoding="utf-8")
        trigger = workflow.split("\non:", maxsplit=1)[1].split(
            "\npermissions:", maxsplit=1
        )[0]

        self.assertIn("'pinvou3-app/src-tauri/**'", trigger)
        self.assertNotIn("'pinvou3-app/src/**'", trigger)
        self.assertIn("'pinvou3-app/package.json'", trigger)
        self.assertIn("'pinvou3-app/package-lock.json'", trigger)

    def test_wrapper_smoke_routes_merge_groups_before_platform_matrix(self):
        workflow = WRAPPER_WORKFLOW.read_text(encoding="utf-8")
        trigger = workflow.split("\non:", maxsplit=1)[1].split(
            "\npermissions:", maxsplit=1
        )[0]
        pull_request = trigger.split("\n  pull_request:", maxsplit=1)[1].split(
            "\n  merge_group:", maxsplit=1
        )[0]
        push = trigger.split("\n  push:", maxsplit=1)[1]
        changes = workflow.split("\n  changes:", maxsplit=1)[1].split(
            "\n  smoke:", maxsplit=1
        )[0]
        smoke = workflow.split("\n  smoke:", maxsplit=1)[1]

        self.assertIn("merge_group:", trigger)
        self.assertIn("push:", trigger)
        self.assertIn("paths:", trigger)
        workflow_path = "'.github/workflows/rustc-wrapper-smoke.yml'"
        self.assertIn(workflow_path, pull_request)
        self.assertIn(workflow_path, push)
        self.assertIn(workflow_path, changes)
        self.assertIn("uses: dorny/paths-filter@v4", changes)
        self.assertIn("wrapper: ${{ steps.filter.outputs.wrapper }}", changes)
        self.assertIn("needs: changes", smoke)
        self.assertIn(
            "if: ${{ needs.changes.outputs.wrapper == 'true' }}", smoke
        )
        self.assertIn("os: [macos-15, ubuntu-latest, windows-latest]", smoke)


if __name__ == "__main__":
    unittest.main()
