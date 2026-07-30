import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"


class ReleaseArm64PolicyTests(unittest.TestCase):
    def setUp(self):
        self.workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.arm64_job = self.workflow.split("\n  build-linux-arm64:", maxsplit=1)[1].split(
            "\n  build-windows-x64:", maxsplit=1
        )[0]

    def test_release_workflow_does_not_run_for_pull_requests(self):
        self.assertNotIn("\n  pull_request:", self.workflow)
        self.assertIn("\n  workflow_dispatch:", self.workflow)
        trigger = self.workflow.split("\non:", maxsplit=1)[1].split(
            "\npermissions:", maxsplit=1
        )[0]
        self.assertIn("paths:\n      - 'VERSION'", trigger)

    def test_arm64_build_keeps_full_release_profile(self):
        job_env = self.arm64_job.split("\n    steps:", maxsplit=1)[0]
        build = self.arm64_job.split(
            "\n      - name: 构建 deb", maxsplit=1
        )[1].split("\n      # tauri deb 产物默认名", maxsplit=1)[0]

        self.assertIn("build-essential pkg-config cmake lld", self.arm64_job)
        for setting in (
            "CARGO_PROFILE_RELEASE_LTO",
            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
            "RUSTFLAGS",
        ):
            self.assertNotIn(setting, job_env)
            self.assertNotIn(setting, build)


if __name__ == "__main__":
    unittest.main()
