import subprocess
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
RELEASE_SCRIPTS = (
    REPO_ROOT / "scripts" / "release-deb.sh",
    REPO_ROOT / "scripts" / "release-macos.sh",
)


class CommunityReleaseContractTests(unittest.TestCase):
    def test_release_scripts_are_valid_bash(self):
        for script in RELEASE_SCRIPTS:
            with self.subTest(script=script.name):
                subprocess.run(["bash", "-n", str(script)], check=True)

    def test_release_scripts_only_publish_to_github(self):
        for script in RELEASE_SCRIPTS:
            source = script.read_text(encoding="utf-8")
            with self.subTest(script=script.name):
                self.assertIn('gh release view "$TAG"', source)
                self.assertIn('gh release upload "$TAG"', source)
                self.assertIn("-community", source)
                self.assertNotIn("ssh ", source)
                self.assertNotIn("rsync ", source)
                self.assertNotIn("pinvou.com", source)

    def test_release_scripts_take_the_version_from_the_VERSION_authority(self):
        """Cross-comparing the packaging files passes when all of them are stale."""
        for script in RELEASE_SCRIPTS:
            source = script.read_text(encoding="utf-8")
            with self.subTest(script=script.name):
                self.assertIn('scripts/sync-version.mjs" --check', source)
                self.assertIn('< "$REPO_ROOT/VERSION"', source)
                self.assertNotIn("V_TAURI", source)
                self.assertNotIn("Version mismatch", source)

    def test_deb_script_reads_the_architecture_off_the_built_artifact(self):
        """dpkg's `|| echo amd64` default guessed, then failed on a name never built."""
        source = (REPO_ROOT / "scripts" / "release-deb.sh").read_text(encoding="utf-8")
        self.assertNotIn("dpkg --print-architecture", source)
        self.assertIn('BUILT=("$DEB_DIR/pinvou3_${VERSION}_"*.deb)', source)
        self.assertIn('[ "${#BUILT[@]}" -ne 1 ]', source)


if __name__ == "__main__":
    unittest.main()
