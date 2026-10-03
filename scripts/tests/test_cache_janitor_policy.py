"""Pin the 2026-10 cache-janitor policy: daily cadence and v0-rust-* dedup.

The janitor is the only workflow that deletes caches; a silent regression
either stops reclaiming quota (rule D vanishes and multi-GB stale generations
pile back up until LRU evicts the gate caches again) or over-deletes (the
dedup escapes its per-series-per-scope bounds, or touches scopes rule A just
classified). Both faces are pinned here.
"""

import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github/workflows/cache-janitor.yml"
SCRIPT = REPO_ROOT / "scripts/cache-janitor.sh"


class CacheJanitorPolicyTests(unittest.TestCase):
    def setUp(self):
        self.workflow = WORKFLOW.read_text(encoding="utf-8")
        self.script = SCRIPT.read_text(encoding="utf-8")

    def test_janitor_runs_daily_with_delete_permissions(self):
        # Weekly cadence let multi-GB stale generations sit in the 10 GB
        # quota for up to seven days (2026-10-01: rust-lint-v2 alone held
        # two generations ≈ 42% of quota while LRU evicted the gate caches).
        self.assertIn("cron: '23 3 * * *'", self.workflow)
        self.assertNotIn("23 3 * * 0", self.workflow)
        # Deleting caches needs the write scope; the cleanup is minutes of
        # work, so the job keeps its explicit cap.
        self.assertIn("actions: write", self.workflow)
        self.assertIn("bash scripts/cache-janitor.sh", self.workflow)
        self.assertIn("timeout-minutes: 30", self.workflow)
        # A dispatch overlapping the scheduled run queues behind it instead
        # of racing it (double-delete is warn-only, but serial is free).
        self.assertIn("group: cache-janitor", self.workflow)
        self.assertIn("cancel-in-progress: false", self.workflow)

    def test_real_deletion_only_on_schedule_or_explicit_opt_in(self):
        # Deleting is destructive: the scheduled run deletes for real, and a
        # manual dispatch deletes only with dry-run explicitly set false.
        # The default dispatch input must stay on the dry-run path.
        self.assertIn("default: true", self.workflow)
        self.assertIn(
            '[ "${{ github.event_name }}" = "schedule" ] '
            '|| [ "${{ inputs.dry-run }}" = "false" ]',
            self.workflow,
        )
        self.assertIn("--dry-run", self.workflow)

    def test_rule_d_keeps_newest_generation_per_series_per_scope(self):
        # The dedup groups by (shared-key series, ref scope) and keeps the
        # newest createdAt per group, deleting items[1:]. Platform markers
        # split the series off the rust-cache key shape
        # (v0-rust-<shared-key>-<platform>-...); a marker-less key fails
        # open (own group, never deleted). Grouping per scope is
        # load-bearing: cross-ref dedup would trade a main-usable cache for
        # a PR-scoped one main cannot read.
        rule_d = self.script.split("# ---------- 规则 D:", maxsplit=1)[1]
        self.assertIn("c['key'].startswith('v0-rust-')", rule_d)
        # Entries without createdAt cannot be ordered: they must be excluded
        # from the groups. A null reaching the sort raises TypeError inside
        # the process substitution, whose failure status nobody checks —
        # rule D would silently stop deduping while the janitor stays green.
        self.assertIn("if not c.get('createdAt'):", rule_d)
        for marker in ("-Linux-", "-Darwin-", "-Windows_NT-"):
            self.assertIn(f"'{marker}'", rule_d)
        self.assertIn("groups.setdefault((series, c['ref']), [])", rule_d)
        self.assertIn("key=lambda c: c['createdAt'], reverse=True", rule_d)
        self.assertIn("for c in items[1:]", rule_d)
        # CLOSED_REFS is exported by rule A and must be excluded here: the
        # dedup must not touch scopes rule A just classified.
        self.assertIn("os.environ.get('CLOSED_REFS','')", rule_d)
        self.assertIn("export CLOSED_REFS", self.script)
        self.assertLess(
            self.script.index("export CLOSED_REFS"),
            self.script.index("# ---------- 规则 D:"),
            "CLOSED_REFS must be exported before rule D reads it",
        )

    def test_deletion_failure_is_nonfatal_and_not_counted(self):
        # A concurrent LRU eviction can delete a cache between listing and
        # deleting: the janitor must warn and continue, and must not count
        # the missed deletion as freed bytes (audit figure stays honest).
        delete_fn = self.script.split("delete_cache() {", maxsplit=1)[1].split(
            "\n}", maxsplit=1
        )[0]
        self.assertIn("if ! gh cache delete", delete_fn)
        self.assertIn("::warning::", delete_fn)
        self.assertIn("return 0", delete_fn)


if __name__ == "__main__":
    unittest.main()
