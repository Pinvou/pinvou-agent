import subprocess
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
RELEASE_SCRIPTS = (
    REPO_ROOT / "scripts" / "release-deb.sh",
    REPO_ROOT / "scripts" / "release-macos.sh",
)


class ReleaseManifestProbeContractTests(unittest.TestCase):
    def test_release_scripts_are_valid_bash(self):
        for script in RELEASE_SCRIPTS:
            with self.subTest(script=script.name):
                subprocess.run(["bash", "-n", str(script)], check=True)

    def test_ssh_probe_fails_closed(self):
        for script in RELEASE_SCRIPTS:
            source = script.read_text(encoding="utf-8")
            with self.subTest(script=script.name):
                self.assertIn("if ! REMOTE_STATE=$(ssh", source)
                self.assertIn('if [ "$REMOTE_STATE" = "exists" ]; then', source)
                self.assertIn('elif [ "$REMOTE_STATE" = "missing" ]; then', source)
                self.assertIn("SSH/权限/网络异常", source)
                self.assertNotIn("REMOTE_EXISTS=$(ssh", source)
                probe_start = source.index("if ! REMOTE_STATE=$(ssh")
                probe_end = source.index(
                    'if [ "$REMOTE_STATE" = "exists" ]; then', probe_start
                )
                self.assertNotIn("|| true", source[probe_start:probe_end])


if __name__ == "__main__":
    unittest.main()
