"""fork-guard.sh 行为名下限的一致性守卫。

回归背景(评审 #484 round-3 B2):下限从 54 提到 96 时只改了两条消息
字符串,`-ge` 条件仍是 54——删掉至多 42 个行为名仍会打印绿色的
"至少保留 96(实际 N)"。本测试钉死:条件里的下限、成功消息、失败消息
三处必须是同一个数字,且不得低于当前登记值。
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FORK_GUARD = ROOT / "scripts" / "fork-guard.sh"

# 当前登记下限(r1 收口之后的过渡期登记,随 fork-modifications 同步推进)。
REGISTERED_FLOOR = 96


class ForkGuardFloorTest(unittest.TestCase):
    def test_floor_is_enforced_and_consistent(self):
        text = FORK_GUARD.read_text(encoding="utf-8")
        condition = re.search(r'forkguard_count" -ge (\d+)', text)
        self.assertIsNotNone(condition, "fork-guard.sh 缺少 forkguard 行为名下限条件")
        floor = int(condition.group(1))
        self.assertEqual(
            floor,
            REGISTERED_FLOOR,
            f"-ge 条件的下限({floor})必须等于登记下限({REGISTERED_FLOOR});"
            "只改消息不改条件的回归曾让删除行为名仍然绿灯",
        )
        messages = re.findall(r"至少保留 (\d+) 条独立 forkguard", text)
        self.assertTrue(messages, "缺少下限成功消息")
        for number in messages:
            self.assertEqual(
                int(number),
                floor,
                f"消息中的下限({number})与 -ge 条件({floor})不一致",
            )
        failure = re.search(r"登记下限为 (\d+)", text)
        self.assertIsNotNone(failure, "缺少下限失败消息")
        self.assertEqual(int(failure.group(1)), floor, "失败消息的下限与条件不一致")


if __name__ == "__main__":
    unittest.main()
