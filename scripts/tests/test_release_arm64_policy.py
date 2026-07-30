import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"


class ReleaseArm64PolicyTests(unittest.TestCase):
    def setUp(self):
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        self.arm64_job = workflow.split("\n  build-linux-arm64:", maxsplit=1)[1].split(
            "\n  build-windows-x64:", maxsplit=1
        )[0]

    def test_arm64_pr_validation_uses_fast_link_profile(self):
        pr_build = self.arm64_job.split(
            "\n      - name: 构建 deb（PR 快速验证）", maxsplit=1
        )[1].split("\n      - name: 构建 deb（正式发布配置）", maxsplit=1)[0]

        self.assertIn("if: github.event_name == 'pull_request'", pr_build)
        self.assertIn('CARGO_PROFILE_RELEASE_LTO: "false"', pr_build)
        self.assertIn('CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"', pr_build)
        self.assertIn('RUSTFLAGS: "-C link-arg=-fuse-ld=lld"', pr_build)
        self.assertIn("build-essential pkg-config cmake lld", self.arm64_job)

    def test_arm64_formal_release_keeps_full_release_profile(self):
        job_env = self.arm64_job.split("\n    steps:", maxsplit=1)[0]
        formal_build = self.arm64_job.split(
            "\n      - name: 构建 deb（正式发布配置）", maxsplit=1
        )[1].split("\n      # tauri deb 产物默认名", maxsplit=1)[0]

        self.assertIn("if: github.event_name != 'pull_request'", formal_build)
        for setting in (
            "CARGO_PROFILE_RELEASE_LTO",
            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
            "RUSTFLAGS",
        ):
            self.assertNotIn(setting, job_env)
            self.assertNotIn(setting, formal_build)


if __name__ == "__main__":
    unittest.main()
