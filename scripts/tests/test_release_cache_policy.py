import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release-packages.yml"
RUST_CACHE_ACTION = "uses: Swatinem/rust-cache@v2"
NO_SAVE = "save-if: false"


def _without_yaml_comments(block):
    # Same helper as test_ci_gate_policy: a commented-out step or save-if
    # line must not satisfy a pin meant for live configuration.
    return "\n".join(
        line for line in block.splitlines() if not line.lstrip().startswith("#")
    )


class ReleaseCachePolicyTests(unittest.TestCase):
    def test_release_rust_caches_never_save(self):
        # Release caches are 1-2 GB each. While they were main-save (the
        # pre-2026-10 policy), every VERSION push landed 4-6 GB of cache at
        # once and the 10 GB LRU flushed the PR-gate caches (windows/macos/
        # rust-test/lint) in the same stroke — the eviction dynamic that left
        # mac-build permanently cold. Releases are rare and have passed cold
        # throughout, so they are now strictly read-only: restore if an entry
        # exists, never write, keep the gate caches resident.
        workflow = _without_yaml_comments(
            RELEASE_WORKFLOW.read_text(encoding="utf-8")
        )
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
                    NO_SAVE,
                    step,
                    "release runs must never write caches (they burst-evict the PR-gate caches)",
                )
                self.assertNotIn(
                    "save-if: ${{",
                    step,
                    "a release cache must not condition a save on any ref (save-if: false is the policy)",
                )


if __name__ == "__main__":
    unittest.main()
