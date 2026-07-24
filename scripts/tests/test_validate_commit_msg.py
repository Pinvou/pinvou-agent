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
            patch.object(validate_commit_msg, "git_commit_exists", side_effect=[True, True]),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "0123456789abcdef",
                "hexin\x00372726039@qq.com\x00fix: 修复提交门禁范围",
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
            patch.object(validate_commit_msg, "git_commit_exists", side_effect=[True, False]),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "0123456789abcdef",
                "hexin\x00372726039@qq.com\x00chore: 初始化社区版源码",
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
            patch.object(validate_commit_msg, "git_commit_exists", side_effect=[True, False]),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "fedcba9876543210",
                "hexin\x00372726039@qq.com\x00fix: english only",
            ]

            errors = validate_commit_msg.validate_range("base", "head")

        self.assertTrue(errors)
        self.assertIn("description must use Chinese", errors[0])

    def test_range_allows_exact_trusted_bot_identity(self) -> None:
        with (
            patch.object(validate_commit_msg, "git_commit_exists", side_effect=[True, False]),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "69de49cc7d1d81159e219c3d6b6494f9fc6859a7",
                (
                    "dependabot[bot]\x00"
                    "49699333+dependabot[bot]@users.noreply.github.com\x00"
                    "chore(deps): bump dompurify from 3.4.2 to 3.4.12"
                ),
            ]

            self.assertEqual(validate_commit_msg.validate_range("base", "head"), [])

    def test_range_rejects_spoofed_bot_name(self) -> None:
        with (
            patch.object(validate_commit_msg, "git_commit_exists", side_effect=[True, False]),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "fedcba9876543210",
                "dependabot[bot]\x00attacker@example.com\x00fix: english only",
            ]

            errors = validate_commit_msg.validate_range("base", "head")

        self.assertTrue(errors)
        self.assertIn("description must use Chinese", errors[0])

    def test_missing_base_validates_the_complete_rewritten_history(self) -> None:
        with (
            patch.object(validate_commit_msg, "git_commit_exists", side_effect=[False, False]),
            patch.object(validate_commit_msg, "git") as git_mock,
        ):
            git_mock.side_effect = [
                "0123456789abcdef",
                "hexin\x0013790929+h3c-hexin@users.noreply.github.com\x00chore: 重建安全的社区版开源基线",
            ]

            self.assertEqual(
                validate_commit_msg.validate_range("missing-base", "clean-head"),
                [],
            )

        self.assertEqual(
            git_mock.call_args_list[0].args,
            (
                "log",
                "--no-merges",
                "--format=%H",
                "clean-head",
            ),
        )


if __name__ == "__main__":
    unittest.main()
