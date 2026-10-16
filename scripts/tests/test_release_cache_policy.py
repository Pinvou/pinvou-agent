import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release-packages.yml"
PR_CHECK_WORKFLOW = REPO_ROOT / ".github/workflows/pr-check.yml"
MAIN_ONLY_SAVE = "save-if: ${{ github.ref == 'refs/heads/main' }}"


def _without_yaml_comments(block):
    # Same helper as test_ci_gate_policy: a commented-out step or save-if
    # line must not satisfy a pin meant for live configuration.
    return "\n".join(
        line for line in block.splitlines() if not line.lstrip().startswith("#")
    )


class ReleaseCachePolicyTests(unittest.TestCase):
    def test_release_jobs_write_no_caches(self):
        # Release caches are 1-2 GB each. While they were main-save (the
        # pre-2026-10 policy), every VERSION push landed 4-6 GB of cache at
        # once and the 10 GB LRU flushed the PR-gate caches (windows/macos/
        # rust-test/lint) in the same stroke — the eviction dynamic that left
        # mac-build permanently cold. Releases are rare and have passed cold
        # throughout, so with #643 (landed 2026-10-09) the save side was
        # disabled; on 2026-10-15 the (by then guaranteed-miss) restore-only
        # rust-cache steps were removed entirely, and the npm side was held
        # to the same no-write policy: setup-node cannot be told not to
        # save, so keeping cache: 'npm' re-mints a main-scope
        # single-lockfile entry whenever the previous one is gone —
        # evicted, or displaced by a lockfile hash change (scopes are
        # refs: release jobs run in main scope alongside pr-check's push
        # legs; an exact hit in scope skips the re-save, and the 0.10.11
        # macOS release leg genuinely restored 294MB from exactly such an
        # entry). What the removal costs is seconds of npm ci on a rare
        # job (cold npm ci measured 10-25s in the release runs); what it
        # saves is ~0.2-0.3GB of per-release churn against the quota.
        # Re-adding any cache write to a release job needs a fresh look at
        # the quota policy (the PR-gate namespaces must stay resident).
        # Case-folded: GitHub resolves action repo refs case-insensitively,
        # so `uses: Swatinem/Rust-Cache@v2` writes real caches too —
        # casing must not be an evasion path for the absence pins.
        workflow = _without_yaml_comments(
            RELEASE_WORKFLOW.read_text(encoding="utf-8")
        ).lower()

        # Fork-generic pin: any *rust-cache action (Swatinem or a fork,
        # under any version) must trip — a re-added cache under a forked
        # action name must not pass an identity pin vacuously. Identity
        # bans cannot cover unknown names: a differently-named future
        # cache mechanism needs its own pin added here.
        self.assertNotIn(
            "rust-cache",
            workflow,
            "release jobs must not run rust-cache: they never save (quota "
            "policy) and the restore side was a guaranteed miss — zero "
            "release-* entries existed live when the steps were removed",
        )
        self.assertNotIn(
            "actions/cache",
            workflow,
            "the no-write policy covers every caching action, not just "
            "rust-cache's restore/save flags",
        )
        # The common Rust caching sidecar by name; a renamed successor is
        # the documented residual gap above.
        self.assertNotIn(
            "sccache",
            workflow,
            "release jobs must not run sccache either — the no-write "
            "policy covers every caching mechanism",
        )
        # Spelling-proof key absence: a value with a trailing comment
        # (cache: 'npm' # warm) or a folded scalar (cache: >-) parses as a
        # live npm cache while per-line value matchers stay green. Banning
        # the cache: key itself is immune to every YAML spelling and fails
        # closed. (The ban is substring-wide on stripped text: an innocent
        # *_cache: env key would trip it too — acceptable here, release
        # jobs must hold no cache config at all.)
        self.assertNotIn(
            "cache:",
            workflow,
            "release jobs must not declare any cache: key at all — any "
            "setup-node npm cache spelling (quoted, commented, folded) is "
            "banned with it",
        )
        self.assertNotIn(
            "cache-dependency-path",
            workflow,
            "a cache-dependency-path without cache: 'npm' is dead config; "
            "release legs hold no npm cache at all",
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
