"""fork-guard.sh 行为名下限的一致性守卫。

钉死:条件里的下限、成功消息、失败消息三处必须是同一个数字
(评审 #484 round-3 B2 的教训——只改消息字符串不改 `-ge` 条件的回归
让删行为名仍然绿灯)。历史备注:该测试曾随 96 下限的废弃谱系撰写;
rebase 到 #721 后的 main 起,登记下限按 main 的 64 执行(round-47 B1:
钉值未随"随 main 骑行"同步,是当时唯一让 CI 常红的自红测试)。
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FORK_GUARD = ROOT / "scripts" / "fork-guard.sh"

# 当前登记下限(main @ c08c8c022 起 docs/fork-modifications.md 登记为 64;
# 随 fork 登记同步推进)。
REGISTERED_FLOOR = 64


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
        messages = re.findall(r"retains at least (\d+) distinct forkguard", text)
        self.assertTrue(messages, "缺少下限成功消息")
        for number in messages:
            self.assertEqual(
                int(number),
                floor,
                f"消息中的下限({number})与 -ge 条件({floor})不一致",
            )
        failure = re.search(r"registered floor is (\d+)", text)
        self.assertIsNotNone(failure, "缺少下限失败消息")
        self.assertEqual(int(failure.group(1)), floor, "失败消息的下限与条件不一致")


if __name__ == "__main__":
    unittest.main()
