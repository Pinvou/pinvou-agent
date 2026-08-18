import importlib.util
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = (
    REPO_ROOT
    / "pinvou3-app"
    / "src-tauri"
    / "resources"
    / "common"
    / "bundle"
    / "dingtalk-skills"
    / "dws"
    / "scripts"
    / "aitable_export_via_task.py"
)

WINDOWS_DEVICE_STEMS = {
    "CON", "PRN", "AUX", "NUL",
    *(f"COM{i}" for i in range(1, 10)),
    *(f"LPT{i}" for i in range(1, 10)),
}


def load_script_module():
    spec = importlib.util.spec_from_file_location("aitable_export_via_task", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class SafeFileNameTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.mod = load_script_module()

    @property
    def safe(self):
        return self.mod.safe_file_name

    def test_path_traversal_keeps_basename_only(self):
        self.assertEqual("evil.xlsx", self.safe("../evil.xlsx"))
        self.assertEqual("evil.xlsx", self.safe("a\\b\\evil.xlsx"))
        self.assertEqual("evil.xlsx", self.safe("/etc/evil.xlsx"))

    def test_empty_and_all_dot_names_fall_back_to_default(self):
        # Win32 把全点名（"." ".." "..." 等）当目录别名，write_bytes 抛
        # PermissionError；剥尾点后为空一律回退默认名。
        for name in ("", "   ", ".", "..", "...", "....", "." * 300, "../", "/"):
            self.assertEqual("export_result.bin", self.safe(name), repr(name))

    def test_trailing_dots_and_spaces_stripped(self):
        # Win32 落盘时会剥尾随点/空格：不剥则 "report.xlsx." 被静默改名、
        # 回显的 savedPath 与实际文件不一致。
        self.assertEqual("report.xlsx", self.safe("report.xlsx."))
        self.assertEqual("report.xlsx", self.safe("report.xlsx. "))
        self.assertEqual("report.xlsx", self.safe("report.xlsx .."))
        self.assertEqual("name", self.safe("name ."))

    def test_leading_dot_preserved(self):
        self.assertEqual(".hidden", self.safe(".hidden"))
        self.assertEqual(".hidden", self.safe(".hidden."))

    def test_windows_reserved_characters_replaced(self):
        self.assertEqual("a_b_c_d.xlsx", self.safe('a:b*c?d.xlsx'))
        self.assertEqual("a_b.xlsx", self.safe("a\x00b.xlsx"))
        self.assertEqual('a_b_c.xlsx_', self.safe('a"b<c.xlsx|'))
        self.assertEqual("a_b.xlsx", self.safe("a\nb.xlsx"))

    def test_windows_device_names_prefixed(self):
        for name in ("CON", "con.txt", "NUL.xlsx", "LPT1", "COM9.tar", "aux"):
            out = self.safe(name)
            self.assertTrue(out.startswith("_"), repr(name))
            self.assertNotIn(out.split(".")[0].upper(), WINDOWS_DEVICE_STEMS, repr(name))
        # 设备名带尾点：先剥点再判设备名，避免 "_CON." 落盘变 "_CON"
        self.assertEqual("_CON", self.safe("CON."))
        # 仅完整 stem 相等才算设备名，普通词不受影响
        self.assertEqual("consumer.txt", self.safe("consumer.txt"))
        self.assertEqual("COM10.txt", self.safe("COM10.txt"))

    def test_overlong_name_truncated_to_200_utf8_bytes(self):
        out = self.safe("a" * 250)
        self.assertLessEqual(len(out.encode("utf-8")), 200)
        self.assertEqual("a" * 200, out)

    def test_overlong_cjk_name_truncated_by_bytes_with_ext_kept(self):
        out = self.safe("报" * 100 + ".xlsx")
        self.assertTrue(out.endswith(".xlsx"))
        self.assertLessEqual(len(out.encode("utf-8")), 200)

    def test_truncation_does_not_leave_trailing_dot(self):
        # 整体截断恰好切在点上时不能留下新尾点（Win32 又会剥掉、名实不符）
        out = self.safe("a" * 199 + "." + "b" * 100)
        self.assertLessEqual(len(out.encode("utf-8")), 200)
        self.assertFalse(out.endswith(".") or out.endswith(" "), repr(out))

    def test_output_invariants(self):
        corpus = (
            "../evil.xlsx", "...", "report.xlsx.", "CON.", "NUL", "报" * 100 + ".xlsx",
            "a" * 250, "." * 300, "  spaced . name . ", "\x00\x01name", "a\\b.xlsx",
            "com1.zip", "trailing... ", "e" * 199 + "." + "x" * 50,
        )
        for name in corpus:
            out = self.safe(name)
            self.assertTrue(out, repr(name))
            self.assertNotIn("/", out)
            self.assertNotIn("\\", out)
            self.assertNotIn("\x00", out)
            self.assertLessEqual(len(out.encode("utf-8")), 200, repr(name))
            self.assertFalse(out.endswith(".") or out.endswith(" "), repr(name))
            stem = out.split(".")[0]
            self.assertNotIn(stem.upper(), WINDOWS_DEVICE_STEMS, repr(name))


if __name__ == "__main__":
    unittest.main()
