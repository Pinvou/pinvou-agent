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
        # env 覆盖;保留 lld(BFD 大二进制 link OOM 实证 + BFD 无 --icf;thin LTO
        # 本身由 rustc 执行,与链接器无关,详见 Cargo.toml [profile.release] 注释)。
        # 体积策略 flag 一并由本断言钉住:--icf=safe(lld 相同代码折叠,safe 档
        # 只折叠地址未被取用的函数,保守应对 fn 地址同一性的已知边界)与
        # remap-path-prefix(产物内嵌构建机路径归一为 /)。
        self.assertIn(
            'RUSTFLAGS: "-C link-arg=-fuse-ld=lld '
            '-C link-arg=-Wl,--icf=safe '
            '-C remap-path-prefix=${{ github.workspace }}=/"',
            job_env,
        )
        self.assertNotIn("CARGO_PROFILE_RELEASE_LTO", job_env)
        self.assertNotIn("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", job_env)

        self.assertIn("build-essential pkg-config cmake lld", self.arm64_job)
        self.assertNotIn("RUSTFLAGS", build)


if __name__ == "__main__":
    unittest.main()
