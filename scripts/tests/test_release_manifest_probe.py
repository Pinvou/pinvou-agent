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


if __name__ == "__main__":
    unittest.main()
