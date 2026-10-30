"""Protect reliability reporting from best-of-N and incomplete-run inflation."""

import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    "community_stats", Path(__file__).resolve().parents[1] / "summarize-prompt-community.py")
stats = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stats)
legacy_spec = importlib.util.spec_from_file_location(
    "legacy_stats", Path(__file__).resolve().parents[1] / "summarize-prompt-eval.py")
legacy = importlib.util.module_from_spec(legacy_spec)
legacy_spec.loader.exec_module(legacy)


class CommunityStatisticsTests(unittest.TestCase):
    def test_all_five_successes_are_required_for_pass_power_five(self):
        self.assertEqual(stats.pass_power(4, 5, 5), 0)
        self.assertEqual(stats.pass_power(5, 5, 5), 1)
        self.assertAlmostEqual(stats.pass_power(4, 5, 1), .8)
        self.assertAlmostEqual(stats.pass_power(4, 5, 2), .6)

    def test_partial_multiturn_family_cannot_be_reported_as_complete(self):
        row = dict(variant="candidate", model="test", repetition=0, scenario="artifact", turn=0)
        with self.assertRaises(ValueError):
            stats.family_groups([row], 5)
        row["scenario"] = "consult"
        self.assertEqual(len(stats.family_groups([row], 5)), 1)
        with self.assertRaises(ValueError):
            stats.family_groups([row, row], 5)

    def test_whole_task_requires_every_turn_in_both_aggregators(self):
        rows = [dict(variant=variant, model="fixture", repetition=0, scenario="artifact", turn=turn,
                     passed=passed, partition="regression", dimensions={"outcome": {"correct": passed},
                     "workflow": {}, "safety": {}}, checks={"completed": True}, model_requests=[],
                     elapsed_secs=1, tool_calls=0, failed_tools=0, unsettled_tools=0, usage=None)
                for variant, passes in [("baseline", [True, True]), ("candidate", [True, False])]
                for turn, passed in enumerate(passes)]
        summary = stats.summarize(rows, repetitions=1)
        self.assertEqual({r["variant"]: r["passed_task_trials"] for r in summary["groups"]},
                         {"baseline": 1, "candidate": 0})
        self.assertEqual(legacy.summarize(rows)["passed_families"], 1)
        for incomplete in [rows[:1], rows + rows[:1]]:
            with self.assertRaises(ValueError):
                stats.summarize(incomplete, repetitions=1)
            with self.assertRaises(ValueError):
                legacy.summarize(incomplete)

    def test_different_or_missing_grader_revisions_cannot_be_aggregated(self):
        rows = [dict(variant=variant, model="fixture", repetition=0, scenario="consult", turn=0,
                     grader_revision=revision) for variant, revision in
                [("baseline", "old-generation"), ("candidate", "new-generation")]]
        for change in ["mixed", "legacy"]:
            if change == "legacy":
                rows[0].pop("grader_revision")
            with self.assertRaisesRegex(ValueError, "mixed grader_revision"):
                stats.summarize(rows, repetitions=1)
            with self.assertRaisesRegex(ValueError, "mixed grader_revision"):
                legacy.summarize(rows)
        rows[1].pop("grader_revision")
        self.assertEqual(len(stats.family_groups(rows, 1)), 2, "wholly legacy frozen corpora remain readable")


if __name__ == "__main__":
    unittest.main()
