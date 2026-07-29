import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release-packages.yml"


class ReleaseArm64PolicyTests(unittest.TestCase):
    def test_arm64_release_validation_uses_fast_link_profile(self):
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        arm64_job = workflow.split("\n  build-linux-arm64:", maxsplit=1)[1].split(
            "\n  build-windows-x64:", maxsplit=1
        )[0]

        self.assertIn('CARGO_PROFILE_RELEASE_LTO: "false"', arm64_job)
        self.assertIn('CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"', arm64_job)
        self.assertIn('RUSTFLAGS: "-C link-arg=-fuse-ld=lld"', arm64_job)
        self.assertIn("build-essential pkg-config cmake lld", arm64_job)


if __name__ == "__main__":
    unittest.main()
