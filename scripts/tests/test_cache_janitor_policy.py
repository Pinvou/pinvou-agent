"""Pin the 2026-10 cache-janitor policy: daily cadence and v0-rust-* dedup.

The janitor is the only workflow that deletes caches; a silent regression
either stops reclaiming quota (rule D vanishes and multi-GB stale generations
pile back up until LRU evicts the gate caches again) or over-deletes (the
dedup escapes its per-series-per-scope bounds, or touches scopes rule A just
classified). Both faces are pinned here.
"""

import re
import subprocess
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github/workflows/cache-janitor.yml"
SCRIPT = REPO_ROOT / "scripts/cache-janitor.sh"


def _without_comment_lines(block):
    # Same discipline as the YAML suites' _without_yaml_comments: a pin
    # meant for live script text must not be satisfied by a commented-out
    # copy of the pinned line.
    return "\n".join(
        line for line in block.splitlines() if not line.lstrip().startswith("#")
    )


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
        rule_d = self.script.split("# ---------- Rule D:", maxsplit=1)[1]
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
            self.script.index("# ---------- Rule D:"),
            "CLOSED_REFS must be exported before rule D reads it",
        )

    def test_rule_a2_cleans_queue_scopes_and_skips_them_downstream(self):
        # Merge-queue refs (refs/heads/gh-readonly-queue/**) are one-shot
        # merge trees: rule A only looks at refs/pull/*, so queue-scoped
        # entries would otherwise wait for the 7-day unused-entry GC. A2
        # deletes them unconditionally (existence = residual; even an active
        # entry only loses a re-download or a cold compile), and must fold
        # its refs into
        # CLOSED_REFS so rules B/C/D — reading the same enumeration
        # snapshot — do not re-attempt entries A2 just deleted and warn
        # about a bogus "concurrent eviction".
        self.assertIn(
            "# ---------- Rule A2:", self.script,
            "the Rule A2 marker must exist — the slice below fails closed "
            "instead of widening to the whole script",
        )
        rule_a2 = _without_comment_lines(
            self.script.split("# ---------- Rule A2:", maxsplit=1)[1].split(
                "# ---------- 规则 B:", maxsplit=1
            )[0]
        )
        # The OUTER enumeration must itself be queue-scoped, as a live
        # line prefix: widening the enumeration to all refs mass-deletes
        # every scope, and the pattern literal surviving in a string
        # elsewhere (an echo, a comment copy) must not keep that green.
        self.assertTrue(
            any(
                line.strip().startswith(
                    "refs = {c['ref'] for c in json.load(open("
                    "'/tmp/cache-janitor-list.json')) if c['ref'].startswith("
                    "'refs/heads/gh-readonly-queue/')}"
                )
                for line in rule_a2.splitlines()
            ),
            "A2's scope enumeration must be queue-scoped",
        )
        # Pin the live filter and projection lines, not just the pattern
        # literal: flipping the scope filter to `if True` (literal kept in
        # a trailing comment) or projecting only the id would otherwise
        # stay green while A2 mass-deletes every scope or under-reports
        # freed bytes. The pins match the LINE PREFIX, not a substring: a
        # trailing `# if c['ref'] == ref: ...` copy would satisfy a bare
        # assertIn on the same surviving line.
        self.assertTrue(
            any(
                line.strip().startswith("if c['ref'] == ref: ")
                for line in rule_a2.splitlines()
            ),
            "A2's inner delete filter must be live: `if c['ref'] == ref: `",
        )
        self.assertTrue(
            any(
                line.strip().startswith("if c['ref'] == ref: ")
                and "c['sizeInBytes']" in line
                for line in rule_a2.splitlines()
            ),
            "A2's inner projection must emit the size (audit honesty)",
        )
        self.assertIn("A2_queue", rule_a2)
        self.assertIn('CLOSED_REFS="$CLOSED_REFS $QUEUE_REFS"', rule_a2)
        # The ref reaches the inner python through the environment, not
        # string interpolation: a quote inside a (hypothetical) queue
        # branch name must not be able to alter the filter expression.
        # BOTH loops (rule A and rule A2) must carry the env plumbing
        # themselves — a single whole-script pin is satisfied by either
        # copy while the other silently no-ops (renamed variable → KeyError
        # inside the unchecked process substitution, janitor stays green).
        self.assertEqual(
            self.script.count('JANITOR_REF="$ref" python3 -c'), 2,
            "every ref-consuming rule must pass the ref via JANITOR_REF "
            "(exactly rule A and rule A2 today) — a future rule following "
            "the pattern updates this count deliberately instead of "
            "dropping its env plumbing",
        )
        self.assertIn('JANITOR_REF="$ref" python3 -c', rule_a2)
        self.assertIn("ref = os.environ['JANITOR_REF']", rule_a2)
        # Quote-agnostic interpolation ban: single- AND double-quoted
        # interpolation both let a quote inside a ref alter the filter
        # expression; ban every quoting spelling.
        self.assertIsNone(
            re.search(r'==\s*["\']\$ref["\']\s*:', self.script),
            "the ref must never be interpolated into the python filter "
            "expression, under any quoting",
        )
        # Same risk via an assignment indirection (ref = '$ref' feeding
        # the filter) — banned as well.
        self.assertIsNone(
            re.search(r"(?<![\w.])ref\s*=\s*[\"']\$ref[\"']", self.script),
            "the ref must never be aliased from an interpolated literal",
        )
        # The suite never executes this script, so at minimum it must stay
        # parseable — a syntax error inside the A2 block would otherwise
        # ship green (same discipline as the linux floor policy's guard
        # script test).
        self.assertEqual(
            subprocess.run(
                ["bash", "-n", str(SCRIPT)], capture_output=True
            ).returncode,
            0,
            "cache-janitor.sh must stay parseable (bash -n)",
        )

    def test_rule_a3_reclaims_over_threshold_v2_snapshots(self):
        # The prune is `if: always()`, so even a run cancelled at its
        # timeout cap prunes before the cache-on-failure post-save; A3
        # insures the residual window where a save lands without the prune
        # having run (runner forced down mid-prune) — exact-hit would
        # otherwise pin that fat snapshot until the next natural key
        # rotation. A3 deletes v2-series entries at or above the
        # threshold; the threshold value is a projection (Linux analog +
        # release-dir delta), calibrated after the first post-merge
        # generations. The suite never executes this script, so the
        # load-bearing lines are pinned on comment-stripped text (a
        # commented-out copy must not satisfy a pin meant for live
        # configuration).
        self.assertIn(
            "# ---------- Rule A3:", self.script,
            "the Rule A3 marker must exist — the slice below fails closed "
            "instead of widening to the whole script",
        )
        rule_a3 = _without_comment_lines(
            self.script.split("# ---------- Rule A3:", maxsplit=1)[1]
        )
        # The threshold value must be the exact live line: a bare
        # substring pin is a literal prefix of its own inflation
        # (2000000000 -> 20000000000 makes A3 delete nothing forever).
        self.assertEqual(
            re.findall(r"(?m)^FAT_SAVE_BYTES=2000000000$", rule_a3),
            ["FAT_SAVE_BYTES=2000000000"],
            "the A3 threshold must be exactly the 2GB value, live, once",
        )
        # The series filter must be live, matched as a LINE PREFIX: a bare
        # substring pin is satisfied by the same line carrying the literal
        # in a trailing comment while the filter itself matches everything.
        self.assertTrue(
            any(
                line.strip().startswith(
                    "if c['key'].startswith('v0-rust-macos-rust-check-v2-')"
                    " and c['sizeInBytes'] >= threshold:"
                )
                for line in rule_a3.splitlines()
            ),
            "A3's series filter must be live WITH the size threshold on "
            "the same line — dropping the comparison would delete every "
            "v2-series entry on every run, not parked in a comment",
        )
        self.assertIn("A3_fat_save", rule_a3)
        self.assertIn("A3_fat_save=$STAT_A3_FAT", self.script)
        # The threshold reaches the inner python through the environment
        # (same interpolation ban as the ref plumbing in rules A/A2).
        self.assertIn('JANITOR_FAT_BYTES="$FAT_SAVE_BYTES"', rule_a3)
        self.assertIn("int(os.environ['JANITOR_FAT_BYTES'])", rule_a3)
        # Entries an earlier rule deleted must be skipped: a fat older
        # generation is deleted by rule D first, and re-attempting it under
        # A3 would print a bogus concurrent-eviction warning.
        self.assertIn('JANITOR_DELETED_IDS="$DELETED_IDS"', rule_a3)
        self.assertIn("str(c['id']) in deleted", rule_a3)
        # Both ends of the skip chain must stay in lockstep: the producer
        # append inside delete_cache() and the python read side — dropping
        # either silently resurrects the bogus concurrent-eviction
        # warnings while every other pin stays green.
        self.assertIn(
            'DELETED_IDS="$DELETED_IDS $id"',
            _without_comment_lines(self.script),
        )
        self.assertIn("os.environ.get('JANITOR_DELETED_IDS'", rule_a3)

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
