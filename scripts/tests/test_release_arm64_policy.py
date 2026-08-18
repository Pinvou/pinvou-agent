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

    def test_arm64_build_keeps_full_release_profile(self):
        job_env = self.arm64_job.split("\n    steps:", maxsplit=1)[0]
        build = self.arm64_job.split(
            "\n      - name: 构建 deb", maxsplit=1
        )[1].split("\n      # tauri deb 产物默认名", maxsplit=1)[0]

        # release profile 已在 Cargo.toml 设 thin LTO(thin 替代 fat),ARM 不再需要
        # env 覆盖;保留 lld(thin LTO 的 link 阶段需要支持 LLVM bitcode 的链接器)。
        self.assertIn('RUSTFLAGS: "-C link-arg=-fuse-ld=lld"', job_env)
        self.assertNotIn("CARGO_PROFILE_RELEASE_LTO", job_env)
        self.assertNotIn("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", job_env)

        self.assertIn("build-essential pkg-config cmake lld", self.arm64_job)
        self.assertNotIn("RUSTFLAGS", build)


if __name__ == "__main__":
    unittest.main()
