"""Protect reliability reporting from best-of-N and incomplete-run inflation."""

import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    "community_stats", Path(__file__).resolve().parents[1] / "summarize-prompt-community.py")
stats = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stats)


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


if __name__ == "__main__":
    unittest.main()
