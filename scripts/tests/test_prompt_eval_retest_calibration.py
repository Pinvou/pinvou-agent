"""Closed positive/negative controls for post-freeze progress-expression equivalence."""

import copy
from pathlib import Path
import sys
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from prompt_eval_retest_calibration import apply, calibrate


class ProgressCalibrationTests(unittest.TestCase):
    def row(self, case="relative_deadline", **other):
        checks = ({"facts_preserved": False} if case == "relative_deadline"
                  else {"task_and_due_preserved": False, "planned_state": True})
        checks.update(completed=True, format_preserved=True, **other)
        return {"scenario": case, "checks": checks, "dimensions": {"outcome": dict(checks)}, "passed": False}

    def test_redundant_weekday_spelling_preserves_frozen_row(self):
        row = self.row()
        before = copy.deepcopy(row)
        result = calibrate(row, "STATUS_NOTE: 验收测试尚未开始，按计划下周周四提交。")
        self.assertTrue(result["passed"])
        self.assertEqual(row, before)
        self.assertTrue(result["dimensions"]["outcome"]["facts_preserved"])

    def test_monday_variants_keep_numeric_date_failure(self):
        for day in ("下一周周一", "下星期一", "下礼拜一"):
            self.assertTrue(calibrate(self.row("skill_fidelity"),
                                      f"接口联调尚未启动，按计划{day}完成。")["passed"])
            result = calibrate(self.row("skill_fidelity", due_date_consistent=False),
                               f"接口联调尚未启动，按计划{day}（10月27日）完成。")
            self.assertTrue(result["checks"]["task_and_due_preserved"])
            self.assertFalse(result["checks"]["due_date_consistent"])
            self.assertFalse(result["passed"])

    def test_wrong_weekday_or_changed_deadline_is_not_promoted(self):
        for day in ("下周周三", "本周四", "未来一周内", "下下一周周四", "下下周周四"):
            self.assertFalse(calibrate(self.row(), f"STATUS_NOTE: 验收测试尚未开始，计划{day}提交。")["passed"])

    def test_wrong_subject_or_started_state_is_not_promoted(self):
        for text in ("STATUS_NOTE: 集成测试尚未开始，计划下周周四提交。",
                     "STATUS_NOTE: 验收测试已经开始，计划下周周四提交。"):
            self.assertFalse(calibrate(self.row(), text)["passed"])

    def test_equivalent_negative_start_wording_without_negation(self):
        for state in ("还没启动", "还没有启动", "还没有开始"):
            self.assertTrue(calibrate(self.row(), f"STATUS_NOTE: 验收测试{state}，计划下周四提交。")["passed"])
        for state in ("不是还没启动", "并非还没有启动", "已经启动"):
            self.assertFalse(calibrate(self.row(), f"STATUS_NOTE: 验收测试{state}，计划下周四提交。")["passed"])

    def test_english_unstarted_equivalence_keeps_other_facts(self):
        row = self.row()
        row["scenario"] = "skill_format"
        self.assertTrue(calibrate(row, "PROGRESS: The data migration remains unstarted, with completion scheduled for Thursday.")["passed"])
        for text in ("PROGRESS: Data migration is not unstarted; completion is planned for Thursday.",
                     "PROGRESS: Data migration is already started; completion is planned for Thursday.",
                     "PROGRESS: Data migration remains unstarted, with completion planned for Friday."):
            self.assertFalse(calibrate(row, text)["passed"])

    def test_exact_prefix_and_no_dates_checks_are_not_changed(self):
        for field in ("format_preserved", "no_added_calendar_date"):
            row = self.row()
            row["checks"][field] = False
            result = calibrate(row, "STATUS_NOTE：验收测试尚未开始，计划下周周四（1月7日）提交。")
            self.assertFalse(result["checks"][field])
            self.assertFalse(result["passed"])

    def test_other_cases_and_already_canonical_text_are_unchanged(self):
        row = self.row()
        row["scenario"] = "json_escape"
        self.assertEqual(calibrate(row, "下周周四"), row)
        row = self.row()
        self.assertEqual(calibrate(row, "STATUS_NOTE: 验收测试尚未开始，计划下周四提交。"), row)

    def test_replay_requires_unique_exact_coverage_and_text(self):
        row = dict(self.row(), model_id="local", variant="candidate", repetition=0, turn=0)
        annotation = {key: row[key] for key in ("model_id", "variant", "repetition", "scenario", "turn")}
        with self.assertRaises(ValueError):
            apply([row], [])
        with self.assertRaises(ValueError):
            apply([row], [annotation, annotation])
        with self.assertRaises(KeyError):
            apply([row], [annotation])
        annotation["progress_calibration_text"] = "STATUS_NOTE: 验收测试尚未开始，计划下周周四提交。"
        self.assertTrue(apply([row], [annotation])[0]["passed"])


if __name__ == "__main__":
    unittest.main()
