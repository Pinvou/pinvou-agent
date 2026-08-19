"""ci-windows-imports-diagnose.py 的行为契约。

覆盖两类回归:
- SxS 清单豁免必须真实生效:嵌入 RT_MANIFEST 优先,外部清单回退
  (CI 先由 mt.exe 嵌入清单、诊断步骤读嵌入结果,才能消除 comctl32 假阳性);
- pr-check.yml 的步骤顺序契约:诊断必须排在嵌入清单之后,否则豁免必然落空
  (诊断读到的是无 v6 声明的默认清单,TaskDialogIndirect 仍计入根因结论)。
"""
import importlib.util
import os
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "ci-windows-imports-diagnose.py"
SPEC = importlib.util.spec_from_file_location("ci_windows_imports_diagnose", SCRIPT)
DIAG = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(DIAG)

ROOT = Path(__file__).resolve().parents[2]
PR_WORKFLOW = ROOT / ".github/workflows/pr-check.yml"

COMMON_CONTROLS_MANIFEST = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">'
    '<dependency><dependentAssembly>'
    '<assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" '
    'version="6.0.0.0" processorArchitecture="*" '
    'publicKeyToken="6595b64144ccf1df" language="*"/>'
    '</dependentAssembly></dependency></assembly>'
)

PLAIN_MANIFEST = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"/>'
)

SECTION_VA = 0x1000
SECTION_RAW = 0x200


def build_pe_with_manifest(manifest_text):
    """构造仅含资源目录(RT_MANIFEST)的最小 PE32。

    布局:资源目录树 类型(24)→名称(1)→语言(0x409)→DATA_ENTRY,
    全部落在唯一节内;导入表 RVA 为 0(诊断脚本据此跳过导入解析)。
    """
    manifest_bytes = manifest_text.encode("utf-8")

    # 节内偏移:类型目录 0x00,名称目录 0x40,语言目录 0x80,
    # DATA_ENTRY 0xC0,清单内容 0x100。
    def resource_dir(entry_name, entry_off):
        # Characteristics/TimeStamp/Versions 共 12 字节,然后条目数与条目
        return struct.pack("<IIHHHH", 0, 0, 0, 0, 0, 1) + struct.pack(
            "<II", entry_name, entry_off
        )

    type_dir = resource_dir(24, 0x80000040)  # RT_MANIFEST → 子目录(名称层)
    name_dir = resource_dir(1, 0x80000080)  # 清单 ID 1 → 子目录(语言层)
    lang_dir = resource_dir(0x409, 0xC0)  # 语言 → 叶子 DATA_ENTRY
    data_entry = struct.pack(
        "<IIII", SECTION_VA + 0x100, len(manifest_bytes), 0, 0
    )

    section = bytearray(0x100)
    section[0x00 : 0x00 + len(type_dir)] = type_dir
    section[0x40 : 0x40 + len(name_dir)] = name_dir
    section[0x80 : 0x80 + len(lang_dir)] = lang_dir
    section[0xC0 : 0xC0 + len(data_entry)] = data_entry
    section += manifest_bytes

    pe = 0x40
    opt_off = pe + 24
    opt_size = 224  # PE32
    data_dir_off = opt_off + 96  # 数据目录起始于可选头 +96
    res_rva_entry = data_dir_off + 16  # 索引 2:资源表

    buf = bytearray(SECTION_RAW + len(section))
    buf[0:2] = b"MZ"
    struct.pack_into("<I", buf, 0x3C, pe)
    buf[pe : pe + 4] = b"PE\0\0"
    # COFF:Machine(i386) 1 节,可选头 224 字节
    struct.pack_into("<HHIIIHH", buf, pe + 4, 0x14C, 1, 0, 0, 0, opt_size, 0x102)
    struct.pack_into("<H", buf, opt_off, 0x10B)  # PE32 magic
    struct.pack_into("<I", buf, res_rva_entry, SECTION_VA)
    struct.pack_into("<I", buf, res_rva_entry + 4, len(section))

    sec_off = opt_off + opt_size
    buf[sec_off : sec_off + 8] = b".rsrc\0\0\0"
    struct.pack_into(
        "<IIII", buf, sec_off + 8, len(section), SECTION_VA, len(section), SECTION_RAW
    )
    buf[SECTION_RAW:] = section
    return bytes(buf)


class ManifestSxsDllsTests(unittest.TestCase):
    def test_no_manifest_yields_no_exemption(self):
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "pinvou3_lib-test.exe"
            exe.write_bytes(build_pe_with_manifest(PLAIN_MANIFEST))
            self.assertEqual(DIAG.manifest_sxs_dlls(str(exe)), set())

    def test_external_manifest_declares_sxs_dll(self):
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "pinvou3_lib-test.exe"
            # 无嵌入清单的 PE:资源表存在但为空目录树,embedded_manifest 返回 None
            exe.write_bytes(build_pe_with_manifest(PLAIN_MANIFEST)[:SECTION_RAW])
            (Path(tmp) / "pinvou3_lib-test.exe.manifest").write_text(
                COMMON_CONTROLS_MANIFEST, encoding="utf-8"
            )
            self.assertEqual(
                DIAG.manifest_sxs_dlls(str(exe)), {"comctl32.dll"}
            )

    def test_embedded_manifest_declares_sxs_dll(self):
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "pinvou3_lib-test.exe"
            exe.write_bytes(build_pe_with_manifest(COMMON_CONTROLS_MANIFEST))
            self.assertEqual(
                DIAG.manifest_sxs_dlls(str(exe)), {"comctl32.dll"}
            )

    def test_embedded_manifest_takes_priority_over_external(self):
        # loader 优先嵌入清单;豁免判定必须同序,否则嵌入 v6 清单后
        # 旁边残留的旧外部清单仍会把 System32 comctl32 缺符号误报为根因。
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "pinvou3_lib-test.exe"
            exe.write_bytes(build_pe_with_manifest(COMMON_CONTROLS_MANIFEST))
            (Path(tmp) / "pinvou3_lib-test.exe.manifest").write_text(
                PLAIN_MANIFEST, encoding="utf-8"
            )
            self.assertEqual(
                DIAG.manifest_sxs_dlls(str(exe)), {"comctl32.dll"}
            )

    def test_script_diagnoses_explicit_exe_and_exempts_sxs(self):
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "pinvou3_lib-test.exe"
            exe.write_bytes(build_pe_with_manifest(COMMON_CONTROLS_MANIFEST))
            # 脚本自身把 stdout/stderr 强制为 UTF-8(见其模块头);父进程必须
            # 用同编码解码,否则在默认编码非 UTF-8 的 Windows(GBK/cp1252)上
            # 严格解码抛 UnicodeDecodeError 或产出乱码,断言必然失败。
            proc = subprocess.run(
                [sys.executable, str(SCRIPT), str(exe)],
                capture_output=True,
                encoding="utf-8",
                errors="replace",
            )
            self.assertEqual(proc.returncode, 0)
            self.assertIn("SxS 清单豁免", proc.stdout)
            self.assertNotIn("可能即 0xc0000139 根因", proc.stdout)


class WorkflowStepOrderContractTests(unittest.TestCase):
    """诊断步骤必须在「嵌入 Common-Controls v6 清单」之后运行(见类 docstring)。"""

    def setUp(self):
        self.workflow = PR_WORKFLOW.read_text(encoding="utf-8")

    def _step_block(self, step_name):
        start = self.workflow.index(f"- name: {step_name}")
        next_step = self.workflow.find("\n      - name:", start + 1)
        return start, self.workflow[start:next_step]

    def test_diagnose_runs_after_manifest_embedding(self):
        embed_start, _ = self._step_block("Windows 测试 exe 嵌入 Common-Controls v6 清单")
        diagnose_start, _ = self._step_block("Windows 测试二进制导入诊断")
        self.assertGreater(diagnose_start, embed_start)

    def test_diagnose_step_contract(self):
        _, block = self._step_block("Windows 测试二进制导入诊断")
        self.assertIn("continue-on-error: true", block)
        # 显式传入回归将运行的测试 exe(mt.exe 刚嵌入清单的那个),
        # 避免多产物并存时按 mtime 猜错诊断对象。
        self.assertIn("ci-windows-imports-diagnose.py", block)
        self.assertIn("PINVOU3_TEST_EXE", block)


if __name__ == "__main__":
    unittest.main()
