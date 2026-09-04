#!/usr/bin/env python3
"""safe_file_name 纯函数单元测试（不触网、不依赖 dws 二进制）。

用途：为 scripts/aitable_export_via_task.py 中的 safe_file_name 提供
可重放回归防护（NOTICE-dingtalk.md「PR #299 审阅代修」第 3 条登记），
覆盖三类核心规整：UTF-8 字节截断（保留扩展名、不切断多字节字符）、
尾随点/空格剥离（Win32 静默改名与全点名 PermissionError 防护）、
非法字符替换与路径穿越防护。

运行方式（在 scripts/ 目录或仓库任意位置）：
    python3 -m unittest discover -s scripts/tests
    python3 scripts/tests/test_dws_safe_file_name.py
"""

from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from aitable_export_via_task import safe_file_name


class TestIllegalCharAndTraversal(unittest.TestCase):
    """非法字符替换与路径穿越防护。"""

    def test_replaces_windows_reserved_chars(self):
        # `:` `*` `?` `"` `<` `>` `|` 逐一替换为 _（相邻非法字符产生连续下划线）
        self.assertEqual(safe_file_name('a:b*c?"d<e>f|g'), "a_b_c__d_e_f_g")

    def test_replaces_control_chars(self):
        self.assertEqual(safe_file_name("re\x01port.xlsx"), "re_port.xlsx")

    def test_keeps_basename_only(self):
        # 反斜杠与正斜杠统一按路径分隔符处理，只保留 basename
        self.assertEqual(safe_file_name("..\\..\\evil.xlsx"), "evil.xlsx")
        self.assertEqual(safe_file_name("../../evil.xlsx"), "evil.xlsx")
        self.assertEqual(safe_file_name("/etc/passwd"), "passwd")

    def test_strips_surrounding_whitespace(self):
        self.assertEqual(safe_file_name("  report.xlsx  "), "report.xlsx")


class TestTrailingDotStrip(unittest.TestCase):
    """Win32 尾随点/空格规整。"""

    def test_trailing_dot_stripped(self):
        self.assertEqual(safe_file_name("report.xlsx."), "report.xlsx")

    def test_trailing_spaces_stripped(self):
        self.assertEqual(safe_file_name("report.xlsx. . "), "report.xlsx")

    def test_all_dots_falls_back_to_default(self):
        # 全点名是目录别名，剥空后回退默认名
        self.assertEqual(safe_file_name("..."), "export_result.bin")

    def test_trailing_dots_only_stripped_not_leading(self):
        # rstrip 只剥尾随点/空格，前导点保留
        self.assertEqual(safe_file_name("...xlsx..."), "...xlsx")


class TestUtf8ByteTruncation(unittest.TestCase):
    """超长名按 UTF-8 字节截断到 200 字节。"""

    def test_long_ascii_name_truncated_with_extension(self):
        name = "a" * 300 + ".xlsx"
        out = safe_file_name(name)
        self.assertLessEqual(len(out.encode("utf-8")), 200)
        self.assertTrue(out.endswith(".xlsx"))

    def test_long_cjk_name_truncated_by_bytes_not_chars(self):
        # 中文每字符 3 字节：按字符数截断无法防 OSError，必须按字节
        name = "测" * 120 + ".xlsx"  # 360 + 5 字节
        out = safe_file_name(name)
        self.assertLessEqual(len(out.encode("utf-8")), 200)
        self.assertTrue(out.endswith(".xlsx"))
        # 截断不得产生无法解码的残缺多字节序列（decode ignore 后仍合法）
        out.encode("utf-8")

    def test_truncation_then_strip_new_trailing_dot(self):
        # 截断可能切在点上留下新尾点，截断后需再剥一次
        name = "a" * 197 + "." + "b" * 5 + ".xlsx"  # 截断点落在扩展名前的点附近
        out = safe_file_name(name)
        self.assertLessEqual(len(out.encode("utf-8")), 200)
        self.assertFalse(out.endswith("."))

    def test_normal_name_unchanged(self):
        self.assertEqual(safe_file_name("报表-2026.xlsx"), "报表-2026.xlsx")


class TestWindowsReservedDeviceNames(unittest.TestCase):
    """Windows 保留设备名加前缀规避。"""

    def test_con_prefixed(self):
        self.assertEqual(safe_file_name("CON"), "_CON")

    def test_con_with_extension_prefixed(self):
        self.assertEqual(safe_file_name("NUL.xlsx"), "_NUL.xlsx")

    def test_com_port_prefixed(self):
        self.assertEqual(safe_file_name("com1"), "_com1")


if __name__ == "__main__":
    unittest.main()
