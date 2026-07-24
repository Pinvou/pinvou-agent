from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "validate-commit-msg.py"
SPEC = importlib.util.spec_from_file_location("validate_commit_msg", SCRIPT)
assert SPEC and SPEC.loader
validate_commit_msg = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validate_commit_msg)


class ValidateCommitRangeTests(unittest.TestCase):
    def test_range_grandfathers_legacy_history_and_skips_merges(self) -> None:
        with (
            patch.object(validate_commit_msg, "git_commit_exists", return_value=True),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "0123456789abcdef",
                "fix: 修复提交门禁范围",
            ]

            self.assertEqual(validate_commit_msg.validate_range("base", "head"), [])

        self.assertEqual(
            git_mock.call_args_list[0].args,
            (
                "log",
                "--no-merges",
                "--format=%H",
                "base..head",
                "--not",
                validate_commit_msg.LEGACY_HISTORY_CUTOFF,
            ),
        )

    def test_range_works_when_legacy_cutoff_is_not_in_clean_history(self) -> None:
        with (
            patch.object(validate_commit_msg, "git_commit_exists", return_value=False),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "0123456789abcdef",
                "chore: 初始化社区版源码",
            ]

            self.assertEqual(validate_commit_msg.validate_range("base", "head"), [])

        self.assertEqual(
            git_mock.call_args_list[0].args,
            (
                "log",
                "--no-merges",
                "--format=%H",
                "base..head",
            ),
        )

    def test_range_still_rejects_new_nonconforming_commit(self) -> None:
        with (
            patch.object(validate_commit_msg, "git_commit_exists", return_value=False),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "fedcba9876543210",
                "fix: english only",
            ]

            errors = validate_commit_msg.validate_range("base", "head")

        self.assertTrue(errors)
        self.assertIn("description must use Chinese", errors[0])


if __name__ == "__main__":
    unittest.main()
