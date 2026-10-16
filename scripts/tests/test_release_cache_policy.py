import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release-packages.yml"
PR_CHECK_WORKFLOW = REPO_ROOT / ".github/workflows/pr-check.yml"
RUST_CACHE_ACTION = "uses: Swatinem/rust-cache@v2"
NO_SAVE = "save-if: false"
MAIN_ONLY_SAVE = "save-if: ${{ github.ref == 'refs/heads/main' }}"


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

    def test_rust_test_allows_main_to_rebuild_cold_cache(self):
        # 与上方 release 测试同款：先剥注释行，被注释掉的 save-if/key/timeout
        # 不得充当活配置钉的满足条件。
        workflow = _without_yaml_comments(
            PR_CHECK_WORKFLOW.read_text(encoding="utf-8")
        )
        rust_test_job = workflow.split("\n  rust-test:", maxsplit=1)[1].split(
            "\n  windows-rust-test:", maxsplit=1
        )[0]

        self.assertIn("timeout-minutes: 120", rust_test_job)
        self.assertIn("shared-key: rust-test-v2", rust_test_job)
        # 主保存策略必须落在 rust-test 自己的 Cargo cache 步骤内断言：rust-test
        # 与 windows-rust-test 之间还排着 cli-test job，其 save-if 文本与本 job
        # 完全相同，对整段切片断言会让本 job 丢掉 save-if 依旧绿灯。
        cargo_cache_step = rust_test_job.split(
            "- name: Cargo cache", maxsplit=1
        )[1].split("\n      - name:", maxsplit=1)[0]
        self.assertIn(MAIN_ONLY_SAVE, cargo_cache_step)


if __name__ == "__main__":
    unittest.main()
