import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release-packages.yml"
RUST_CACHE_ACTION = "uses: Swatinem/rust-cache@v2"
MAIN_ONLY_SAVE = "save-if: ${{ github.ref == 'refs/heads/main' }}"


class ReleaseCachePolicyTests(unittest.TestCase):
    def test_release_rust_caches_are_read_only_outside_main(self):
        # Release caches are 1-2 GB each. A manual release run on a branch
        # may restore main's caches but must never write its own: branch
        # copies would fill the repository's 10 GB cache quota and evict
        # the warm caches the PR gates depend on.
        workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
        cache_steps = [
            step
            for step in workflow.split("\n      - name:")
            if RUST_CACHE_ACTION in step
        ]

        self.assertEqual(
            len(cache_steps),
            4,
            "adding or removing a release Rust cache needs a fresh look at the cache quota policy",
        )
        for step in cache_steps:
            with self.subTest(step=step.splitlines()[0].strip()):
                self.assertIn(
                    MAIN_ONLY_SAVE,
                    step,
                    "release runs outside main may only restore caches, never save them",
                )


if __name__ == "__main__":
    unittest.main()
