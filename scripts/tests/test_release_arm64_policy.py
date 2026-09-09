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

        # The release profile already sets thin LTO in Cargo.toml (thin
        # replaces fat), so ARM no longer needs an env override; lld stays
        # (proven BFD OOM on large-binary links + BFD has no --icf; thin LTO
        # itself is executed by rustc and is linker-independent, see the
        # Cargo.toml [profile.release] comments). This assertion also pins
        # the size-policy flags: --icf=safe (lld identical-code folding; the
        # safe tier only folds functions whose address is never taken,
        # conservatively handling the known fn-address identity boundary)
        # and remap-path-prefix (normalizes build-machine paths embedded in
        # the artifact to /).
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
