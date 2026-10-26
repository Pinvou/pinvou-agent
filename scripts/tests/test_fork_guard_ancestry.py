"""Exercise the registered-base gate with real, isolated Git histories."""

import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
BASH = shutil.which("bash")


def isolated_git_env():
    # Hooks and rebase --exec export repository selectors that override -C.
    # Config/namespace selectors can redirect writes too; inherit none of them.
    return {key: value for key, value in os.environ.items()
            if not key.upper().startswith("GIT_")}


@unittest.skipUnless(BASH, "requires bash")
class ForkGuardAncestryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "CodeWhale"
        self.repo.mkdir()
        self.git("init", "--quiet")
        self.tree = self.git("hash-object", "-t", "tree", "--stdin", input="")
        self.upstream = self.commit("upstream")
        self.r1 = self.commit("r1", self.upstream)
        self.maintenance = self.commit("maintenance", self.r1)
        self.head = self.commit("candidate", self.maintenance)
        self.git("update-ref", "HEAD", self.head)

    def git(self, *args, input=None):
        env = dict(isolated_git_env(), GIT_AUTHOR_NAME="Fixture", GIT_COMMITTER_NAME="Fixture",
                   GIT_AUTHOR_EMAIL="fixture@example.com", GIT_COMMITTER_EMAIL="fixture@example.com")
        return subprocess.run(
            ["git", "-C", str(self.repo), *args], input=input, env=env,
            text=True, encoding="utf-8", capture_output=True, check=True,
        ).stdout.strip()

    def commit(self, message, parent=None):
        args = ["commit-tree", self.tree, "-m", message]
        if parent:
            args.extend(["-p", parent])
        return self.git(*args)

    def run_gate(self, maintenance):
        # Execute the production topology block, stopping before unrelated
        # file fingerprints. No network, fake Git, or production checkout edits.
        source = (ROOT / "scripts/fork-guard.sh").read_text(encoding="utf-8")
        source, boundary, remainder = source.partition(
            '\nbold "── 第 1 层：登记主题与父仓适配指纹 ──"\n'
        )
        self.assertTrue(boundary, "missing topology/fingerprint boundary")
        self.assertRegex(remainder, r"\A# 格式：[^\n]*\nfingerprints=\(")
        for name, value in {
            "EXPECTED_UPSTREAM": self.upstream,
            "R1_CLOSURE": self.r1,
            "EXPECTED_HEAD": self.head,
            "MAINTENANCE_BASE": maintenance,
        }.items():
            source, count = re.subn(rf'^{name}="[^"]*"$', f'{name}="{value}"', source, flags=re.M)
            self.assertEqual(count, 1)
        source, count = re.subn(r"^EXPECTED_COMMITS=\d+$", "EXPECTED_COMMITS=3", source, flags=re.M)
        self.assertEqual(count, 1)
        script = self.root / "scripts/fork-guard.sh"
        script.parent.mkdir()
        script.write_text(source + '\nexit "$fail"\n', encoding="utf-8", newline="\n")
        return subprocess.run(
            [BASH, "scripts/fork-guard.sh", "--fast"], cwd=self.root,
            env=isolated_git_env(),
            text=True, encoding="utf-8", capture_output=True, timeout=30,
        )

    def test_candidate_contains_registered_maintenance_base(self):
        result = self.run_gate(self.maintenance)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_shared_r1_does_not_substitute_for_maintenance_ancestry(self):
        other_maintenance = self.commit("divergent maintenance", self.r1)
        result = self.run_gate(other_maintenance)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("does not contain public maintenance base", result.stdout)

    def test_unavailable_maintenance_base_fails_closed(self):
        result = self.run_gate("f" * 40)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("does not contain public maintenance base", result.stdout)


@unittest.skipUnless(BASH, "requires bash")
class GitEnvironmentIsolationTests(unittest.TestCase):
    def test_ambient_git_environment_cannot_redirect_fixture(self):
        # Only disposable repositories are exposed to the child, including when
        # this test is used as a mutation control against the unsafe old code.
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            foreign = root / "foreign.git"
            worktree = root / "foreign-worktree"
            alternate = root / "alternate-objects"
            worktree.mkdir()
            alternate.mkdir()
            (worktree / "keep.txt").write_bytes(b"foreign worktree sentinel\n")
            subprocess.run(["git", "init", "--bare", "--quiet", str(foreign)],
                           env=isolated_git_env(), check=True, capture_output=True)
            index = root / "foreign-index"
            index.write_bytes(b"foreign index sentinel\n")
            before = {path.relative_to(root): path.read_bytes()
                      for path in root.rglob("*") if path.is_file()}
            env = dict(os.environ,
                       GIT_DIR=str(foreign), GIT_WORK_TREE=str(worktree),
                       GIT_INDEX_FILE=str(index), GIT_COMMON_DIR=str(foreign),
                       GIT_OBJECT_DIRECTORY=str(foreign / "objects"),
                       GIT_ALTERNATE_OBJECT_DIRECTORIES=str(alternate))
            result = subprocess.run(
                [sys.executable, "-X", "utf8", str(Path(__file__).resolve()),
                 "ForkGuardAncestryTests"],
                env=env, text=True, encoding="utf-8", capture_output=True, timeout=120,
            )
            after = {path.relative_to(root): path.read_bytes()
                     for path in root.rglob("*") if path.is_file()}
            self.assertEqual(after, before, "fixture changed the foreign repository or index")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("Ran 3 tests", result.stderr)


if __name__ == "__main__":
    unittest.main()
