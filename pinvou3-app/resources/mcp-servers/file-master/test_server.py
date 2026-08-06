#!/usr/bin/env python3
"""file-master MCP server 的可重复测试（纯 stdlib unittest）。

覆盖：
  1. JSON-RPC 协议：子进程跑 server.py，逐行喂 initialize → ping → tools/list → tools/call，
     含 notification 静默、未知 method、异步删除冒烟（验证 worker 不污染 stdout）。
  2. file_find：命中/大小写/limit/剪枝 + extensions 与修改时间过滤 + 非法日期报错。
  3. junction 循环防护（Windows）：自指 junction 环下 file_find / _dir_stats / disk_scan
     不无限循环、不重复求和（修复前 4 秒预算被耗尽）。
  4. disk_scan：分组结构 / risk_legend / 磁盘信息 / drives（非系统盘）/ 输出 ≤10K 字符。
  5. file_trash：dry-run 不执行；白名单硬拒绝；confirm=true 异步提交 → file_trash_status
     轮询到 done → 逐项结果；并发任务日志不丢；真回收站验证（Windows）。
  6. file_restore：日志 list / _pinvou_filemaster_trash 兜底还原 / 真回收站还原（Windows）/ 错误分支。
  7. file_empty_recycle：非 Windows unsupported；confirm=false 只查占用不清空。
  8. manifest.json：可解析、字段完备、6 个工具名/依赖/companion_skills/版本正确。

运行：python test_server.py  （Windows / macOS / Linux 均可）
"""
import datetime
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import server  # noqa: E402

IS_WIN = sys.platform == "win32"

FIND_TIME_BUDGET_SEC = server.FIND_TIME_BUDGET_SEC


def build_tree(root):
    """造一棵测试文件树，返回关键路径 dict。"""
    paths = {}
    paths["report_doc"] = os.path.join(root, "proj_report_2024.docx")
    with open(paths["report_doc"], "w", encoding="utf-8") as f:
        f.write("report")
    sub = os.path.join(root, "SubDir")
    os.makedirs(sub)
    paths["report_txt"] = os.path.join(sub, "Report_final.txt")
    with open(paths["report_txt"], "w", encoding="utf-8") as f:
        f.write("report2")
    paths["target_dir"] = os.path.join(root, "MixedCase_Dir_TARGET")
    os.makedirs(paths["target_dir"])
    with open(os.path.join(paths["target_dir"], "inner.txt"), "w") as f:
        f.write("x")
    pruned = os.path.join(root, "node_modules")
    os.makedirs(pruned)
    paths["pruned_file"] = os.path.join(pruned, "report_in_pruned.txt")
    with open(paths["pruned_file"], "w") as f:
        f.write("secret")
    return paths


def _local_noon(y, m, d):
    """该日期本地时区正午的 epoch 秒（避开时区边界）。"""
    return datetime.datetime(y, m, d, 12, 0, 0).timestamp()


def _fake_move_to_recycle(path, delay=0.0):
    """测试替身：sleep 模拟耗时后把文件挪到同级 _trash_test（绕开真实回收站，
    非 Windows 也能跑完整链路；dest 供 restore 用）。"""
    if delay:
        time.sleep(delay)
    parent = os.path.dirname(os.path.abspath(path))
    trash = os.path.join(parent, "_trash_test")
    os.makedirs(trash, exist_ok=True)
    dest = os.path.join(trash, os.path.basename(path))
    shutil.move(path, dest)
    return ("fallback-trash-dir", "已移入测试 _pinvou_filemaster_trash 目录: %s" % dest, dest)


def _poll_done(task_id, timeout=8.0):
    """轮询 file_trash_status 直到 done（异步用例统一先轮询再 teardown，
    避免 rmtree 与后台 worker 的 move 竞争）。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        st = server.file_trash_status(task_id=task_id)
        if st["status"] == "done":
            return st
        time.sleep(0.05)
    raise AssertionError("task %s 未在 %ss 内完成" % (task_id, timeout))


class _FakeHomeMixin(unittest.TestCase):
    """把 HOME/USERPROFILE 指到临时目录，保证日志不污染真实 ~/.pinvou3。"""

    def fake_home(self):
        self._fake_home = tempfile.mkdtemp(prefix="fm_home_")
        self._old_home = os.environ.get("HOME")
        self._old_profile = os.environ.get("USERPROFILE")
        self._old_xdg = os.environ.get("XDG_DATA_HOME")
        os.environ["HOME"] = self._fake_home
        if IS_WIN:
            os.environ["USERPROFILE"] = self._fake_home
        os.environ.pop("XDG_DATA_HOME", None)  # 强制走 ~/.local/share 回退，隔离 XDG Trash

    def restore_home(self):
        if self._old_home is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = self._old_home
        if IS_WIN:
            if self._old_profile is None:
                os.environ.pop("USERPROFILE", None)
            else:
                os.environ["USERPROFILE"] = self._old_profile
        if self._old_xdg is None:
            os.environ.pop("XDG_DATA_HOME", None)
        else:
            os.environ["XDG_DATA_HOME"] = self._old_xdg
        shutil.rmtree(self._fake_home, ignore_errors=True)


class FileFindTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="fm_find_")
        self.paths = build_tree(self.tmp)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_hits_files_case_insensitive(self):
        out = server.file_find(query="report", dir=self.tmp)
        hits = {r["path"] for r in out["results"]}
        self.assertIn(self.paths["report_doc"], hits)
        self.assertIn(self.paths["report_txt"], hits)  # 大小写不敏感
        self.assertNotIn(self.paths["pruned_file"], hits)  # node_modules 被剪枝
        for r in out["results"]:
            self.assertIn("size_human", r)
            self.assertIn("modified", r)
            self.assertIn("is_dir", r)

    def test_hits_directory_names(self):
        out = server.file_find(query="target", dir=self.tmp)
        dirs = [r for r in out["results"] if r["is_dir"]]
        self.assertTrue(any(d["path"] == self.paths["target_dir"] for d in dirs),
                        "目录名也应参与匹配")

    def test_limit_clamped_to_50(self):
        for i in range(60):
            with open(os.path.join(self.tmp, "bulk_%02d.txt" % i), "w") as f:
                f.write("x")
        out = server.file_find(query="bulk", dir=self.tmp, limit=100)
        self.assertLessEqual(len(out["results"]), 50)
        self.assertTrue(out["truncated"])

    def test_empty_query_and_bad_dir(self):
        # 空 query + 无过滤 → 拒绝（防全盘）；空 query + 过滤 → 纯类型搜索
        self.assertIn("error", server.file_find(query="  ", dir=self.tmp))
        self.assertIn("error", server.file_find(query="x", dir=os.path.join(self.tmp, "nope")))

    def test_empty_query_type_search(self):
        """"找所有安装包"类场景：query 留空 + extensions = 纯类型搜索。"""
        self.paths["setup_exe"] = os.path.join(self.tmp, "setup_v2.1.exe")
        self.paths["app_msi"] = os.path.join(self.tmp, "app_installer.msi")
        self.paths["notes_txt"] = os.path.join(self.tmp, "notes.txt")
        for k in ("setup_exe", "app_msi", "notes_txt"):
            with open(self.paths[k], "w") as f:
                f.write("x")
        out = server.file_find(query="", dir=self.tmp, extensions=["exe", "msi"])
        hits = {r["name"] for r in out["results"]}
        self.assertEqual(hits, {"setup_v2.1.exe", "app_installer.msi"}, "应只命中安装包")
        self.assertIn("纯类型搜索", out["note"])
        # 空 query + 无过滤 → 拒绝
        self.assertIn("error", server.file_find(query="", dir=self.tmp))
        # 空 query + 大小过滤也可用（造 1KB 文件，min 0.0001MB≈105B）
        big = os.path.join(self.tmp, "big_type.bin")
        with open(big, "wb") as f:
            f.write(b"x" * 1024)
        time.sleep(0.05)  # 等目录 mtime 刷新（目录缓存快照语义）
        out = server.file_find(query="", dir=self.tmp, min_size_mb=0.0001)
        self.assertIn("big_type.bin", {r["name"] for r in out["results"]})

    def test_output_fields(self):
        out = server.file_find(query="report", dir=self.tmp)
        self.assertEqual(out["type"], "file_find")
        self.assertIn(self.tmp, out["searched_dirs"])
        self.assertIn("note", out)

    def test_no_hit_without_dir_hints_other_drives(self):
        """未命中且未定向时，note 提示默认范围仅主目录、可用 dir 定向其他盘。"""
        out = server.file_find(query="zzz_nonexistent_fm_probe")
        self.assertEqual(out["count"], 0)
        self.assertIn("dir", out["note"])
        self.assertIn("其他盘", out["note"])
        # 定向搜索未命中时不提示（用户已给位置）
        out2 = server.file_find(query="zzz_nonexistent_fm_probe", dir=self.tmp)
        self.assertNotIn("其他盘", out2["note"])

    def test_extensions_filter(self):
        """扩展名过滤：大小写/点前缀兼容，目录不参与（extensions 非空时）。"""
        out = server.file_find(query="report", dir=self.tmp, extensions=["txt"])
        hits = {r["path"] for r in out["results"]}
        self.assertIn(self.paths["report_txt"], hits)
        self.assertNotIn(self.paths["report_doc"], hits)
        out2 = server.file_find(query="report", dir=self.tmp, extensions=["xlsx"])
        self.assertEqual(out2["results"], [], "无 xlsx 文件应返回空")
        out3 = server.file_find(query="report", dir=self.tmp, extensions=[".TXT"])
        self.assertIn(self.paths["report_txt"], {r["path"] for r in out3["results"]})
        self.assertIn("扩展名", out3["note"])

    def test_modified_filters(self):
        a = os.path.join(self.tmp, "mod_old.txt")
        b = os.path.join(self.tmp, "mod_mid.txt")
        c = os.path.join(self.tmp, "mod_new.txt")
        for p, ts in ((a, _local_noon(2024, 1, 1)),
                      (b, _local_noon(2024, 6, 15)),
                      (c, _local_noon(2025, 1, 1))):
            with open(p, "w") as f:
                f.write("x")
            os.utime(p, (ts, ts))
        after = server.file_find(query="mod_", dir=self.tmp, modified_after="2024-02-01")
        self.assertEqual({r["name"] for r in after["results"]}, {"mod_mid.txt", "mod_new.txt"})
        before = server.file_find(query="mod_", dir=self.tmp, modified_before="2024-12-31")
        self.assertEqual({r["name"] for r in before["results"]}, {"mod_old.txt", "mod_mid.txt"})
        both = server.file_find(query="mod_", dir=self.tmp, modified_after="2024-02-01",
                                modified_before="2024-12-31")
        self.assertEqual({r["name"] for r in both["results"]}, {"mod_mid.txt"})

    def test_multi_word_and_matching(self):
        """空格/标点分词 + 多词 AND：所有词都命中才返回。"""
        out = server.file_find(query="report final", dir=self.tmp)
        self.assertEqual({r["name"] for r in out["results"]}, {"Report_final.txt"},
                         "多词 AND 应只命中同时含两个词的文件")
        out = server.file_find(query="report docx", dir=self.tmp)
        self.assertEqual({r["name"] for r in out["results"]}, {"proj_report_2024.docx"})
        out = server.file_find(query="report final xyz", dir=self.tmp)
        self.assertEqual(out["results"], [], "任一词不命中 → 空")
        # 中文标点也分词
        out = server.file_find(query="report，final", dir=self.tmp)
        self.assertEqual({r["name"] for r in out["results"]}, {"Report_final.txt"})

    def test_sort_by_modified_alias(self):
        """sort_by=modified 是 mtime 的别名（模型直觉常用值，不得报错）。"""
        tmp = tempfile.mkdtemp(prefix="fm_alias_")
        try:
            files = {
                "old_report.txt": _local_noon(2026, 1, 1),
                "new_report.txt": _local_noon(2026, 1, 5),
            }
            for n, ts in files.items():
                with open(os.path.join(tmp, n), "w") as f:
                    f.write("x")
                os.utime(os.path.join(tmp, n), (ts, ts))
            out = server.file_find(query="report", dir=tmp, sort_by="modified", order="desc")
            self.assertNotIn("error", out)
            self.assertEqual([r["name"] for r in out["results"]],
                             ["new_report.txt", "old_report.txt"])
            # 与 mtime 等价
            out2 = server.file_find(query="report", dir=tmp, sort_by="mtime", order="desc")
            self.assertEqual([r["name"] for r in out2["results"]],
                             [r["name"] for r in out["results"]])
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_total_hits_reports_truncation(self):
        """total_hits=收集到的全部命中数：limit 截断时 total_hits > count，模型据此重搜。"""
        for i in range(30):
            with open(os.path.join(self.tmp, "bulk_hits_%02d.txt" % i), "w") as f:
                f.write("x")
        out = server.file_find(query="bulk_hits", dir=self.tmp, limit=20)
        self.assertEqual(out["count"], 20, "返回条数受 limit 截断")
        self.assertEqual(out["total_hits"], 30, "total_hits 应为收集到的全部命中")
        self.assertTrue(out["total_hits"] > out["count"], "截断信号")
        # 未截断时两者相等
        out = server.file_find(query="report", dir=self.tmp)
        self.assertEqual(out["total_hits"], out["count"])

    def test_relevance_sort_order(self):
        """相关度排序（默认）：全词 > 前缀 > 子串，同分按修改时间从新到旧。
        用独立目录避免 build_tree 的 report 文件干扰排序断言。"""
        tmp = tempfile.mkdtemp(prefix="fm_rel_")
        try:
            files = {
                "report.pdf": _local_noon(2026, 1, 3),        # 全词 100
                "report_old.pdf": _local_noon(2026, 1, 1),    # 全词 100（同分，旧 mtime 应靠后）
                "reporting.txt": _local_noon(2026, 1, 4),     # 前缀 60
                "myreportdata.xlsx": _local_noon(2026, 1, 5), # 子串 30
            }
            for n, ts in files.items():
                with open(os.path.join(tmp, n), "w") as f:
                    f.write("x")
                os.utime(os.path.join(tmp, n), (ts, ts))
            out = server.file_find(query="report", dir=tmp)
            self.assertEqual([r["name"] for r in out["results"]],
                             ["report.pdf", "report_old.pdf", "reporting.txt", "myreportdata.xlsx"],
                             "应按 全词(新)>全词(旧)>前缀>子串 排序")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_sort_by_size_and_name(self):
        """sort_by=size（目录恒排最后）/name + order 方向。"""
        big = os.path.join(self.tmp, "big_data.txt")
        with open(big, "wb") as f:
            f.write(b"x" * 4096)
        small = os.path.join(self.tmp, "small_data.txt")
        with open(small, "wb") as f:
            f.write(b"x" * 8)
        os.makedirs(os.path.join(self.tmp, "data_dir"))
        out = server.file_find(query="data", dir=self.tmp, sort_by="size", order="desc")
        self.assertEqual([r["name"] for r in out["results"]],
                         ["big_data.txt", "small_data.txt", "data_dir"])
        out = server.file_find(query="data", dir=self.tmp, sort_by="size", order="asc")
        self.assertEqual([r["name"] for r in out["results"]],
                         ["small_data.txt", "big_data.txt", "data_dir"], "目录应恒排最后")
        out = server.file_find(query="data", dir=self.tmp, sort_by="name", order="asc")
        self.assertEqual([r["name"] for r in out["results"]],
                         ["big_data.txt", "data_dir", "small_data.txt"])
        out = server.file_find(query="data", dir=self.tmp, sort_by="name", order="desc")
        self.assertEqual([r["name"] for r in out["results"]],
                         ["small_data.txt", "data_dir", "big_data.txt"])
        # 非法 sort_by/order → error
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, sort_by="bogus"))
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, order="sideways"))

    def test_size_filters(self):
        """min_size_mb/max_size_mb：只作用于文件（目录不参与）；非法值报错。"""
        big = os.path.join(self.tmp, "size_big.bin")
        with open(big, "wb") as f:
            f.write(b"x" * (2 * 1024 * 1024))
        mid = os.path.join(self.tmp, "size_mid.bin")
        with open(mid, "wb") as f:
            f.write(b"x" * (100 * 1024))
        os.makedirs(os.path.join(self.tmp, "size_dir"))
        out = server.file_find(query="size", dir=self.tmp, min_size_mb=1)
        self.assertEqual({r["name"] for r in out["results"]}, {"size_big.bin"})
        out = server.file_find(query="size", dir=self.tmp, max_size_mb=0.5)
        self.assertEqual({r["name"] for r in out["results"]}, {"size_mid.bin"})
        out = server.file_find(query="size", dir=self.tmp, min_size_mb=1)
        self.assertNotIn("size_dir", {r["name"] for r in out["results"]}, "目录不参与大小过滤")
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, min_size_mb="abc"))
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, max_size_mb=-5))
        self.assertIn("error", server.file_find(query="x", dir=self.tmp,
                                                min_size_mb=10, max_size_mb=1))

    def test_query_dedup_and_single_char_dropped(self):
        """重复词去重；长度 1 且非数字的词丢弃（"report x" 不应因 x 误杀）。"""
        out = server.file_find(query="report report", dir=self.tmp)
        self.assertIn(self.paths["report_doc"], {r["path"] for r in out["results"]})
        out = server.file_find(query="report x", dir=self.tmp)
        self.assertIn(self.paths["report_doc"], {r["path"] for r in out["results"]},
                      "单字符 x 应被丢弃，不等价于 AND")

    def test_query_all_punctuation_falls_back(self):
        """全标点/被分词掏空的 query：回退整串子串（不报错、不返回全盘）。"""
        out = server.file_find(query="___", dir=self.tmp)
        self.assertNotIn("error", out)
        self.assertEqual(out["results"], [], "tmp 中无含 ___ 的文件")
        # 版本号类 query 分词后仍可命中
        vf = os.path.join(self.tmp, "v1.4.0_notes.txt")
        with open(vf, "w") as f:
            f.write("x")
        out = server.file_find(query="v1.4.0", dir=self.tmp)
        self.assertTrue(any(r["name"] == "v1.4.0_notes.txt" for r in out["results"]))

    def test_multi_word_chinese_and(self):
        """中文多词 AND：词不要求连续（"散热 报表" 命中 散热周报表 而非 散热周报）。"""
        for n in ("散热周报表.xlsx", "散热周报.txt"):
            with open(os.path.join(self.tmp, n), "w") as f:
                f.write("x")
        out = server.file_find(query="散热 报表", dir=self.tmp)
        self.assertEqual({r["name"] for r in out["results"]}, {"散热周报表.xlsx"})
        out = server.file_find(query="散热，报表", dir=self.tmp)  # 顿号分词
        self.assertEqual({r["name"] for r in out["results"]}, {"散热周报表.xlsx"})
        # 单短语仍走子串（回归）
        out = server.file_find(query="散热周报表", dir=self.tmp)
        self.assertEqual({r["name"] for r in out["results"]}, {"散热周报表.xlsx"})

    def test_and_works_before_cap_filled(self):
        """AND 在 cap 填满前生效：60 个只含首词的文件不得占满 limit 淹没真命中。"""
        for i in range(60):
            with open(os.path.join(self.tmp, "bulk_%02d.txt" % i), "w") as f:
                f.write("x")
        target = os.path.join(self.tmp, "bulk_report.txt")
        with open(target, "w") as f:
            f.write("x")
        out = server.file_find(query="bulk report", dir=self.tmp, limit=20)
        self.assertTrue(any(r["path"] == target for r in out["results"]),
                        "AND 应命中 bulk_report.txt，纯 bulk 文件不应占满 cap")

    def test_output_fuse_trims_overflow(self):
        """输出保险丝：满 limit + 深路径（曾 28,574 字符）必须压缩到预算内且说明。"""
        base = tempfile.mkdtemp(prefix="fm_fuse_")
        try:
            deep = base
            for i in range(5):
                deep = os.path.join(deep, "very_long_directory_name_%02d_" % i + "x" * 40)
            os.makedirs(deep, exist_ok=True)
            for i in range(50):
                with open(os.path.join(deep, "overflow_test_file_%02d.txt" % i), "w") as f:
                    f.write("x")
            out = server.file_find(query="overflow_test", dir=base, limit=50)
            n = len(json.dumps(out, ensure_ascii=False))
            self.assertLess(n, server.OUTPUT_BUDGET_CHARS,
                            "压缩后应低于预算: %d" % n)
            self.assertIn("输出超过底座上限", out["note"])
            self.assertTrue(out["truncated"], "保险丝触发后 truncated 应为 true（结果可能不全）")
            self.assertTrue(out["results"], "至少应保留 1 条")
            # 下钻同样受保险丝约束
            dr = server.disk_scan(path=deep)
            n2 = len(json.dumps(dr, ensure_ascii=False))
            self.assertLess(n2, server.OUTPUT_BUDGET_CHARS, "下钻压缩后应低于预算: %d" % n2)
        finally:
            shutil.rmtree(base, ignore_errors=True)

    def test_size_filter_invalid_values(self):
        """大小过滤的非法值：NaN/Inf/bool/负数 → error（不得 500）。"""
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, min_size_mb="nan"))
        self.assertIn("error", server.file_find(query="x", dir=self.tmp,
                                                max_size_mb=float("inf")))
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, min_size_mb=True))

    def test_exclude_dirs(self):
        """exclude_dirs：额外排除同名目录（数组与字符串形式）。"""
        out = server.file_find(query="report", dir=self.tmp, exclude_dirs=["SubDir"])
        hits = {r["path"] for r in out["results"]}
        self.assertNotIn(self.paths["report_txt"], hits, "exclude_dirs 应排除该目录内文件")
        self.assertIn(self.paths["report_doc"], hits)
        out = server.file_find(query="report", dir=self.tmp, exclude_dirs="SubDir")
        self.assertNotIn(self.paths["report_txt"], {r["path"] for r in out["results"]})

    def test_relative_date_filter(self):
        """相对天数 Nd：模型不必知道当前日期（修复"上周算成 2025 年"的严重错误）。"""
        import datetime as _dt
        today = _dt.date.today()
        files = {
            "rel_old.txt": _local_noon(today.year, today.month, today.day) - 10 * 86400,  # 10 天前
            "rel_mid.txt": _local_noon(today.year, today.month, today.day) - 3 * 86400,   # 3 天前
            "rel_new.txt": _local_noon(today.year, today.month, today.day),               # 今天
        }
        for n, ts in files.items():
            with open(os.path.join(self.tmp, n), "w") as f:
                f.write("x")
            os.utime(os.path.join(self.tmp, n), (ts, ts))
        # 最近 7 天（含今天）：mid + new，old 不在
        out = server.file_find(query="rel_", dir=self.tmp, modified_after="7d")
        self.assertEqual({r["name"] for r in out["results"]}, {"rel_mid.txt", "rel_new.txt"})
        # 相对 before：早于 3 天前 → 只 old
        out = server.file_find(query="rel_", dir=self.tmp, modified_before="3d")
        self.assertEqual({r["name"] for r in out["results"]}, {"rel_old.txt"})
        # 大小写与空格容忍
        out = server.file_find(query="rel_", dir=self.tmp, modified_after="7D")
        self.assertEqual(len(out["results"]), 2)
        # 非法相对值 → error
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, modified_after="7x"))
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, modified_after="-3d"))

    def test_invalid_date_errors(self):
        out = server.file_find(query="x", dir=self.tmp, modified_after="2024/01/01")
        self.assertIn("error", out)
        self.assertIn("YYYY-MM-DD", out["error"])
        self.assertIn("error", server.file_find(query="x", dir=self.tmp, modified_before="yesterday"))

    @unittest.skipUnless(IS_WIN, "junction 仅 Windows")
    def test_junction_loop_terminates(self):
        """自指 junction 环：file_find 不得无限循环（修复前 4 秒预算被耗尽 → truncated）。"""
        loop = os.path.join(self.tmp, "loop")
        r = subprocess.run(["cmd", "/c", "mklink", "/J", loop, self.tmp],
                           capture_output=True, timeout=30)
        if r.returncode != 0:
            self.skipTest("mklink /J 失败（非 NTFS?）: %s" % r.stdout.decode(errors="replace"))
        probe = os.path.join(self.tmp, "junction_probe.txt")
        with open(probe, "w") as f:
            f.write("x")
        t0 = time.monotonic()
        out = server.file_find(query="junction_probe", dir=self.tmp)
        elapsed = time.monotonic() - t0
        self.assertLess(elapsed, FIND_TIME_BUDGET_SEC * 0.9,
                        "不应耗尽时间预算: %.2fs" % elapsed)
        self.assertFalse(out["truncated"], "环目录不应导致截断")
        self.assertTrue(any(r["path"] == probe for r in out["results"]),
                        "环目录内的文件应仍能命中")


class DiskScanTest(unittest.TestCase):
    def test_structure(self):
        out = server.disk_scan()
        self.assertEqual(out["type"], "disk_scan_overview")
        self.assertIn("disk", out)
        self.assertIn("groups", out)
        self.assertIn("drives", out)
        self.assertIn("large_files", out)
        legend = out["risk_legend"]
        self.assertEqual(set(legend), {"green", "yellow", "red"})
        self.assertTrue(out["groups"], "至少应有一个存在的分组")
        keys = {g["key"] for g in out["groups"]}
        if IS_WIN:
            self.assertIn("temp", keys)
            self.assertIn("windows", keys,
                          "Windows 系统目录应在概览中可见（否则模型会退化为 exec_shell 兜底）")
            win = next(g for g in out["groups"] if g["key"] == "windows")
            self.assertEqual(win["risk"], "red")
        for g in out["groups"]:
            self.assertIn(g["risk"], ("green", "yellow", "red"))
            self.assertIn(g["status"], ("ok", "estimated", "denied", "skipped"))
            self.assertIn("size_human", g)
            self.assertIn("top_children", g)
            self.assertLessEqual(len(g["top_children"]), server.SHOWN_MAX_ITEMS,
                                 "概览每组 top_children ≤%d（瘦身）" % server.SHOWN_MAX_ITEMS)
            self.assertIn("others_count", g, "未展示子项应归入 others 统计")
            self.assertIn("others_size_human", g)
            self.assertIn("shown_threshold", g)
            self.assertNotIn("paths", g, "概览不带 paths 数组（瘦身）")
            self.assertIn("note", g)
        self.assertLessEqual(len(out["large_files"]), 10)
        if IS_WIN:
            for d in out["drives"]:
                self.assertIn("drive", d)
                self.assertIn("total", d)

    def test_scan_group_parallel_matches_serial(self):
        """组内并行求和不改变结果：串行与并行（内层池）的总量/文件数/Top 一致。"""
        tmp = tempfile.mkdtemp(prefix="fm_pg_")
        try:
            for d in ("a", "b", "c"):
                os.makedirs(os.path.join(tmp, d))
                with open(os.path.join(tmp, d, "f.bin"), "wb") as f:
                    f.write(b"x" * 2048)
            group = {"key": "t", "name": "t", "paths": [tmp], "risk": "green", "note": ""}
            serial = server._scan_group(group, time.monotonic() + 10.0)
            from concurrent.futures import ThreadPoolExecutor as _TPE
            with _TPE(max_workers=4) as ex:
                parallel = server._scan_group(group, time.monotonic() + 10.0, ex)
            self.assertEqual(serial["size_bytes"], parallel["size_bytes"])
            self.assertEqual(serial["file_count"], parallel["file_count"])
            self.assertEqual({c["name"] for c in serial["top_children"]},
                             {c["name"] for c in parallel["top_children"]})
            self.assertEqual(serial["status"], parallel["status"])
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_top_children_by_threshold_includes_all_big_folders(self):
        """阈值策略：组内多个大文件夹全部展示（回归：固定 Top3 会漏第 4 个+），小项归 others。"""
        tmp = tempfile.mkdtemp(prefix="fm_thr_")
        old = server.SHOWN_MIN_BYTES
        try:
            server.SHOWN_MIN_BYTES = 1024  # 放宽下限，便于用小块文件构造阈值场景
            for i in range(5):
                d = os.path.join(tmp, "big_%d" % i)
                os.makedirs(d)
                with open(os.path.join(d, "f.bin"), "wb") as f:
                    f.write(b"x" * (200 * 1024))  # 每个 200KB
            for i in range(10):
                with open(os.path.join(tmp, "small_%d.txt" % i), "w") as f:
                    f.write("x")  # 1B，远低于阈值
            group = {"key": "t", "name": "t", "paths": [tmp], "risk": "green", "note": ""}
            out = server._scan_group(group, time.monotonic() + 10.0)
            names = [c["name"] for c in out["top_children"]]
            self.assertEqual(len(names), 5, "5 个 200KB 子目录应全部展示（≥阈值），实际 %s" % names)
            for i in range(5):
                self.assertIn("big_%d" % i, names)
            self.assertEqual(out["others_count"], 10, "10 个小文件应归入 others")
            self.assertEqual(out["others_size_human"], "10 B")
            self.assertTrue(out["shown_threshold"])
        finally:
            server.SHOWN_MIN_BYTES = old
            shutil.rmtree(tmp, ignore_errors=True)

    def test_overview_groups_keep_definition_order(self):
        """并行概览：分组输出必须保持定义顺序（线程池收集后按定义序合并）。"""
        out = server.disk_scan()
        defined = [g["key"] for g in server._scan_groups()]
        actual = [g["key"] for g in out["groups"]]
        self.assertEqual(actual, [k for k in defined if k in set(actual)],
                         "并行结果应按定义顺序合并输出: %s" % actual)

    def test_overview_output_fits_context_limit(self):
        """概览输出必须在输出保险丝预算内（不触发折叠丢组），且远低于底座 12,000 硬上限。"""
        out = server.disk_scan()
        n = len(json.dumps(out, ensure_ascii=False))
        self.assertLess(n, server.OUTPUT_BUDGET_CHARS,
                        "概览 JSON 应 < 保险丝 %d 字符（否则折叠丢关键组），实际 %d"
                        % (server.OUTPUT_BUDGET_CHARS, n))
        self.assertLess(n, 10_800, "仍应远低于底座 12,000 字符硬上限，实际 %d" % n)

    def test_drill_down_lists_children_sorted(self):
        """下钻模式：直接子项按大小降序、≤20 条、输出同样受限。"""
        tmp = tempfile.mkdtemp(prefix="fm_drill_")
        try:
            os.makedirs(os.path.join(tmp, "big_dir"))
            with open(os.path.join(tmp, "big_dir", "a.bin"), "wb") as f:
                f.write(b"x" * 4096)
            with open(os.path.join(tmp, "small.txt"), "wb") as f:
                f.write(b"x" * 8)
            out = server.disk_scan(path=tmp)
            self.assertEqual(out["type"], "disk_scan_drill")
            names = [c["name"] for c in out["children"]]
            self.assertEqual(names[0], "big_dir", "大目录应排在最前: %s" % names)
            self.assertIn("small.txt", names)
            self.assertLessEqual(len(out["children"]), 20)
            n = len(json.dumps(out, ensure_ascii=False))
            self.assertLess(n, 10_000)
            # children 应带 size_estimated 标记（estimated 只影响大小、不影响清单完整）
            for c in out["children"]:
                self.assertIn("size_estimated", c)
            # 错误路径的三分支
            self.assertIn("error", server.disk_scan(path="relative\\path"))
            self.assertIn("error", server.disk_scan(path=os.path.join(tmp, "nope")))
            self.assertIn("error", server.disk_scan(path=os.path.join(tmp, "small.txt")))
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_dir_cache_hit_and_invalidation(self):
        """目录 mtime 快照缓存：未变目录命中返回同一列表；变更后自动重扫自愈。"""
        tmp = tempfile.mkdtemp(prefix="fm_cache_")
        try:
            for i in range(50):
                with open(os.path.join(tmp, "cache_file_%02d.txt" % i), "w") as f:
                    f.write("x")
            e1 = server._cached_scandir(tmp)
            self.assertEqual(len(e1), 50)
            key = server._norm(tmp)
            self.assertIn(key, server._dir_cache, "首次扫描应建立缓存")
            e2 = server._cached_scandir(tmp)
            self.assertIs(e2, e1, "mtime 未变应命中缓存（同一列表对象）")
            # 文件变更 → 目录 mtime 更新 → 重扫
            time.sleep(0.01)
            with open(os.path.join(tmp, "cache_new.txt"), "w") as f:
                f.write("new")
            e3 = server._cached_scandir(tmp)
            self.assertTrue(any(n == "cache_new.txt" for n, *_ in e3), "变更后应重扫出新文件")
            self.assertIsNot(e3, e1, "缓存应已刷新")
            # 走缓存的 file_find 结果正确
            out = server.file_find(query="cache_file", dir=tmp)
            self.assertEqual(out["count"], 20)  # limit 默认 20
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_dir_cache_concurrent_safe(self):
        """worker 线程与主线程并发访问缓存不崩（file_trash 异步删除同遍历路径）。"""
        tmp = tempfile.mkdtemp(prefix="fm_cache_c_")
        errors = []
        try:
            for i in range(30):
                with open(os.path.join(tmp, "c_%02d.txt" % i), "w") as f:
                    f.write("x")

            def worker():
                try:
                    for _ in range(10):
                        server._cached_scandir(tmp)
                        server.file_find(query="c_", dir=tmp)
                except Exception as e:  # noqa: BLE001
                    errors.append(e)

            threads = [threading.Thread(target=worker) for _ in range(4)]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
            self.assertEqual(errors, [], "并发访问缓存不应抛异常: %s" % errors)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_cached_scandir_checked_budget_and_cache(self):
        """预算感知物化：超时返回 (部分条目, True) 且不写缓存；正常则写缓存。"""
        tmp = tempfile.mkdtemp(prefix="fm_ck_")
        try:
            for i in range(30):
                with open(os.path.join(tmp, "f_%02d.txt" % i), "w") as f:
                    f.write("x")
            # 已过期的 deadline → 立即截断，部分结果不写缓存
            entries, trunc = server._cached_scandir_checked(tmp, time.monotonic() - 1.0)
            self.assertTrue(trunc)
            self.assertIsInstance(entries, list)
            self.assertNotIn(server._norm(tmp), server._dir_cache, "部分结果不应写缓存")
            # 远未来的 deadline → 全量物化 + 写缓存
            entries, trunc = server._cached_scandir_checked(tmp, time.monotonic() + 30.0)
            self.assertFalse(trunc)
            self.assertEqual(len(entries), 30)
            self.assertIn(server._norm(tmp), server._dir_cache, "完整结果应写缓存")
            # 目录不存在 → (None, False)
            self.assertEqual(
                server._cached_scandir_checked(os.path.join(tmp, "nope"), time.monotonic() + 30.0),
                (None, False),
            )
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_dir_stats_deadline_passed_marks_estimated(self):
        """预算已过期时 _dir_stats 立即停止并标 estimated（回归：时间基座混用曾让预算失效）。"""
        tmp = tempfile.mkdtemp(prefix="fm_ds_")
        try:
            with open(os.path.join(tmp, "a.bin"), "wb") as f:
                f.write(b"x" * 1024)
            total, count, est, denied = server._dir_stats(tmp, time.monotonic() - 1.0)
            self.assertTrue(est, "预算过期应标 estimated")
            self.assertEqual(total, 0, "预算过期应立即停止，不应统计到文件")
            self.assertFalse(denied)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_large_files_depth_boundary_not_timeout(self):
        """深度边界（>SCAN_MAX_DEPTH）不应误报超时截断（回归：曾与超时共用 truncated 标记）。"""
        tmp = tempfile.mkdtemp(prefix="fm_lf_")
        try:
            leaf = tmp
            for i in range(10):  # 深度 10，超过限深 8
                leaf = os.path.join(leaf, "d%d" % i)
            os.makedirs(leaf)
            with open(os.path.join(leaf, "x.txt"), "w") as f:
                f.write("x")
            found, truncated = server._large_files(tmp, time.monotonic() + 30.0)
            self.assertFalse(truncated, "深度边界是设计行为，不应标记超时截断")
            self.assertEqual(found, [])
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    @unittest.skipUnless(IS_WIN, "junction 仅 Windows")
    def test_dir_stats_junction_loop_no_double_count(self):
        """自指 junction 环：_dir_stats 不无限循环、不重复求和（修复前会双计/循环）。"""
        tmp = tempfile.mkdtemp(prefix="fm_junc_")
        try:
            loop = os.path.join(tmp, "loop")
            r = subprocess.run(["cmd", "/c", "mklink", "/J", loop, tmp],
                               capture_output=True, timeout=30)
            if r.returncode != 0:
                self.skipTest("mklink /J 失败（非 NTFS?）")
            with open(os.path.join(tmp, "a.bin"), "wb") as f:
                f.write(b"x" * 4096)
            total, count, est, denied = server._dir_stats(tmp, time.monotonic() + 5.0)
            self.assertEqual(total, 4096, "环内文件只应统计一次，实际 %d" % total)
            self.assertEqual(count, 1)
            self.assertFalse(denied)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


class FileTrashTest(_FakeHomeMixin):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="fm_trash_")
        self.victim = os.path.join(self.tmp, "fm_victim_test.txt")
        with open(self.victim, "w", encoding="utf-8") as f:
            f.write("delete me")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_paths_cap_rejects_overflow(self):
        """paths 超过上限（50）明确报错，防输出爆炸。"""
        many = [os.path.join(self.tmp, "f%02d.txt" % i) for i in range(51)]
        out = server.file_trash(paths=many, confirm=False)
        self.assertIn("最多", out.get("error", ""))
        out = server.file_erase(paths=many, confirm=False)
        self.assertIn("最多", out.get("error", ""))

    def test_dry_run_does_not_execute(self):
        out = server.file_trash(paths=[self.victim], confirm=False)
        self.assertFalse(out["executed"])
        item = out["items"][0]
        self.assertTrue(item["allowed"])
        self.assertTrue(os.path.exists(self.victim), "dry-run 不得动文件")
        self.assertIn("confirm=true", out["note"])

    def test_whitelist_rejections(self):
        home = os.path.expanduser("~")
        cases = [
            (os.path.join(self.tmp, "*.txt"), "通配符"),
            ("relative\\path\\x.txt" if IS_WIN else "relative/x.txt", "非绝对路径"),
            (os.path.join(self.tmp, "not_exist.txt"), "不存在"),
            (home, "主目录本身"),
        ]
        if IS_WIN:
            cases += [
                ("C:\\Windows", "系统目录"),
                (os.environ.get("SystemDrive", "C:") + "\\", "盘符根"),
            ]
            pf = os.environ.get("ProgramFiles")
            if pf and os.path.isdir(pf):
                cases.append((pf, "Program Files"))
        else:
            cases.append(("/", "根目录"))
        for raw, why in cases:
            out = server.file_trash(paths=[raw], confirm=False)
            item = out["items"][0]
            self.assertFalse(item["allowed"], "%s 应被拒绝: %s" % (why, raw))
            self.assertTrue(item["rejected_reason"])

    def test_confirm_submits_task_and_polls_to_done(self):
        """confirm=true 立即返回 task_id（不阻塞），轮询到 done 后逐项结果齐全。
        真实环境（Windows）还验证确实进了系统回收站。"""
        self.fake_home()
        try:
            out = server.file_trash(paths=[self.victim], confirm=True)
            self.assertEqual(out["type"], "file_trash_submitted")
            self.assertIn("task_id", out)
            task_id = out["task_id"]

            st = server.file_trash_status(task_id=task_id)
            self.assertIn(st["status"], ("running", "done"))  # 立即返回，不阻塞
            self.assertEqual(st["total_count"], 1)

            done = _poll_done(task_id)
            self.assertEqual(done["status"], "done")
            item = done["results"][0]
            self.assertEqual(item["status"], "moved", json.dumps(done, ensure_ascii=False))
            self.assertFalse(os.path.exists(self.victim), "原路径应已消失")
            self.assertEqual(done["summary"]["moved"], 1)
            self.assertIn(item["via"], ("recycle-bin", "fallback-trash-dir"))
            if item["via"] == "fallback-trash-dir":
                self.assertTrue(os.path.exists(item["detail"].split(": ", 1)[-1]))
            elif IS_WIN:
                # 用 Shell.Application 验证回收站里确有该项（PowerShell 不可用则跳过）
                ps = shutil.which("powershell") or shutil.which("powershell.exe")
                if not ps:
                    self.skipTest("无 PowerShell，跳过回收站内容验证")
                cmd = ("(New-Object -ComObject Shell.Application).NameSpace(0xA).Items()"
                       " | Select-Object -ExpandProperty Name")
                proc = subprocess.run([ps, "-NoProfile", "-Command", cmd],
                                      capture_output=True, timeout=60)
                names = proc.stdout.decode("gbk", errors="replace")
                self.assertIn("fm_victim_test.txt", names,
                              "回收站里应能找到被删文件: %s" % names[-500:])
        finally:
            self.restore_home()

    def test_status_lists_tasks_and_unknown_id(self):
        """无 task_id 列最近任务；未知 id 报错。"""
        self.fake_home()
        real = server._move_to_recycle
        server._move_to_recycle = lambda p: _fake_move_to_recycle(p, delay=0.1)
        try:
            o1 = server.file_trash(paths=[self.victim], confirm=True)
            lst = server.file_trash_status()
            self.assertEqual(lst["type"], "file_trash_tasks")
            self.assertTrue(any(t["task_id"] == o1["task_id"] for t in lst["tasks"]))
            self.assertIn("error", server.file_trash_status(task_id="no-such-task-xyz"))
            _poll_done(o1["task_id"])
        finally:
            server._move_to_recycle = real
            self.restore_home()

    def test_oversize_quota_uses_fallback_trash(self):
        """目标超过回收站配额：改走 _pinvou_filemaster_trash 兜底（可恢复），绝不物理删除——
        修复前 Shell 在 FOF_NOCONFIRMATION 下会静默物理删除且日志误记 recycle-bin。"""
        self.fake_home()
        real_quota = server._recycle_quota
        real_move = server._move_to_recycle
        server._recycle_quota = lambda p: 0  # 零配额，任何文件都超
        server._move_to_recycle = lambda p: (_ for _ in ()).throw(
            AssertionError("超配额不应走 SHFileOperationW"))
        try:
            out = server.file_trash(paths=[self.victim], confirm=True)
            done = _poll_done(out["task_id"])
            item = done["results"][0]
            self.assertEqual(item["status"], "moved")
            self.assertEqual(item["via"], "fallback-trash-dir")
            self.assertIn("配额", item["detail"])
            self.assertFalse(os.path.exists(self.victim), "原路径应已移走")
            # _pinvou_filemaster_trash 兜底必须可恢复
            rst = server.file_restore(action="restore", path=self.victim)
            self.assertEqual(rst.get("status"), "restored", json.dumps(rst, ensure_ascii=False))
            self.assertTrue(os.path.exists(self.victim), "兜底删除应可还原")
        finally:
            server._recycle_quota = real_quota
            server._move_to_recycle = real_move
            self.restore_home()

    def test_quota_within_limit_uses_recycle_bin(self):
        """配额内：正常走回收站路径（_move_to_recycle）。"""
        self.fake_home()
        real_quota = server._recycle_quota
        real_move = server._move_to_recycle
        calls = []
        server._recycle_quota = lambda p: 10 ** 12  # 1TB 配额
        server._move_to_recycle = lambda p: (calls.append(p), _fake_move_to_recycle(p))[1]
        try:
            out = server.file_trash(paths=[self.victim], confirm=True)
            done = _poll_done(out["task_id"])
            self.assertEqual(done["results"][0]["status"], "moved")
            self.assertEqual(calls, [self.victim], "配额内应走 _move_to_recycle")
        finally:
            server._recycle_quota = real_quota
            server._move_to_recycle = real_move
            self.restore_home()

    def test_estimated_size_uses_fallback(self):
        """大小估算不可靠（estimated）时保守走 _pinvou_filemaster_trash 兜底（实际可能超配额）。"""
        real_quota = server._recycle_quota
        real_move = server._move_to_recycle
        server._recycle_quota = lambda p: 10 ** 12
        server._move_to_recycle = lambda p: (_ for _ in ()).throw(
            AssertionError("estimated 不应走 SHFileOperationW"))
        try:
            via, detail, dest = server._execute_trash_item(
                {"path": self.victim, "size": 10, "size_estimated": True})
            self.assertEqual(via, "fallback-trash-dir")
            self.assertIn("配额", detail)
            self.assertTrue(os.path.exists(dest), "文件应已移入 _pinvou_filemaster_trash")
        finally:
            server._recycle_quota = real_quota
            server._move_to_recycle = real_move

    def test_concurrent_tasks_no_log_loss(self):
        """两个任务并发执行时删除日志两条都落盘（_log_lock 防读-改-写覆盖）。"""
        self.fake_home()
        v2 = os.path.join(self.tmp, "fm_victim2.txt")
        with open(v2, "w", encoding="utf-8") as f:
            f.write("delete me 2")
        real = server._move_to_recycle
        server._move_to_recycle = lambda p: _fake_move_to_recycle(p, delay=0.1)
        try:
            o1 = server.file_trash(paths=[self.victim], confirm=True)
            o2 = server.file_trash(paths=[v2], confirm=True)
            _poll_done(o1["task_id"])
            _poll_done(o2["task_id"])
            log = server._read_trash_log()
            originals = {e["original_path"] for e in log if e["status"] == "trashed"}
            self.assertTrue({self.victim, v2} <= originals,
                            "两条删除记录都应落盘: %s" % [e["original_path"] for e in log])
        finally:
            server._move_to_recycle = real
            self.restore_home()


class FileRestoreTest(_FakeHomeMixin):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="fm_restore_")
        self.victim = os.path.join(self.tmp, "fm_restore_victim.txt")
        with open(self.victim, "w", encoding="utf-8") as f:
            f.write("restore me")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_error_branches(self):
        out = server.file_restore(action="restore", path=None)
        self.assertIn("error", out)
        out = server.file_restore(action="bogus")
        self.assertIn("error", out)
        if not IS_WIN:
            # list 读日志是跨平台的；restore 仅回收站方式在非 Windows 报 unsupported
            self.assertEqual(server.file_restore(action="list")["type"], "file_restore_list")

    def test_trash_log_written_and_listed(self):
        """删除后日志落盘且 list 来源是日志（redirect HOME 不碰真实日志）。"""
        self.fake_home()
        try:
            server._append_trash_log([{"path": self.victim, "via": "fallback-trash-dir",
                                       "dest": os.path.join(self.tmp, "_pinvou_filemaster_trash", "x.txt")}])
            lst = server.file_restore(action="list")
            self.assertEqual(lst["type"], "file_restore_list")
            hit = [i for i in lst["items"] if i["original_path"] == self.victim]
            self.assertEqual(len(hit), 1, "list 应来自删除日志: %s" % lst)
            self.assertEqual(hit[0]["via"], "fallback-trash-dir")
        finally:
            self.restore_home()

    def test_fallback_restore_is_deterministic(self):
        """_pinvou_filemaster_trash 兜底方式：restore 直接从落点挪回原路径，不依赖回收站。"""
        self.fake_home()
        try:
            dest = server._fallback_trash(self.victim)
            self.assertFalse(os.path.exists(self.victim))
            server._append_trash_log([{"path": self.victim, "via": "fallback-trash-dir",
                                       "dest": dest}])
            rst = server.file_restore(action="restore", path=self.victim)
            self.assertEqual(rst.get("status"), "restored", json.dumps(rst, ensure_ascii=False))
            self.assertTrue(os.path.exists(self.victim))
            with open(self.victim, encoding="utf-8") as f:
                self.assertEqual(f.read(), "restore me")
            # 日志已被标记 restored，再次 restore 应报错
            rst2 = server.file_restore(action="restore", path=self.victim)
            self.assertIn("error", rst2)
        finally:
            self.restore_home()

    def test_async_trash_then_restore_roundtrip(self):
        """异步删除（走测试替身兜底）→ 日志 list 枚举到 → restore 还原 → 内容一致。"""
        self.fake_home()
        real = server._move_to_recycle
        server._move_to_recycle = _fake_move_to_recycle
        try:
            out = server.file_trash(paths=[self.victim], confirm=True)
            _poll_done(out["task_id"])
            self.assertFalse(os.path.exists(self.victim))

            lst = server.file_restore(action="list", limit=50)
            self.assertEqual(lst["type"], "file_restore_list", json.dumps(lst, ensure_ascii=False))
            hit = [i for i in lst["items"] if i["name"] == "fm_restore_victim.txt"]
            self.assertTrue(hit, "list 应能枚举到刚删的文件: %s" % lst["items"][:5])

            rst = server.file_restore(action="restore", path=hit[0]["original_path"])
            self.assertEqual(rst.get("status"), "restored", json.dumps(rst, ensure_ascii=False))
            self.assertTrue(os.path.exists(self.victim), "文件应已还原到原路径")
            with open(self.victim, encoding="utf-8") as f:
                self.assertEqual(f.read(), "restore me")
        finally:
            server._move_to_recycle = real
            self.restore_home()

    @unittest.skipUnless(IS_WIN, "回收站 $I/$R 结构仅 Windows")
    def test_rb_locate_and_manual_restore(self):
        """COM 还原动词不可用（NOVERB）时的降级路径：按 $I 元数据定位 $R 数据文件挪回。
        $I 格式：header8 + size8 + FILETIME8 + 路径字节长度4 + UTF-16LE 路径。"""
        tmp = tempfile.mkdtemp(prefix="fm_rb_")
        try:
            # 构造假回收站：$I + $R
            rb = os.path.join(tmp, "S-1-5-21-fake")
            os.makedirs(rb, exist_ok=True)
            orig = os.path.join(tmp, "work", "CodeWhale", "target")
            data_file = os.path.join(rb, "$RABC123")
            with open(data_file, "wb") as f:
                f.write(b"x" * 100)
            path_bytes = orig.encode("utf-16-le")
            # header8 + size8 + FILETIME8 + 占位4字节(offset24,非长度字段) + UTF-16LE 路径（双 null 结尾）
            i_data = (b"\x02" + b"\x00" * 7) + struct.pack("<Q", 100) + \
                struct.pack("<Q", 0) + b"\x34\x00\x00\x00" + path_bytes + b"\x00\x00"
            with open(os.path.join(rb, "$IABC123"), "wb") as f:
                f.write(i_data)

            old_plat = server.PLATFORM
            old_root = server._rb_root_for
            server.PLATFORM = "win32"
            server._rb_root_for = lambda drive: tmp  # 注入假回收站根
            try:
                # 定位
                rfile = server._rb_locate(orig)
                self.assertEqual(rfile, data_file, "应按原始路径定位到 $R 数据文件")
                # 手动挪回
                restored = server._rb_manual_restore(orig)
                self.assertEqual(restored, orig)
                self.assertTrue(os.path.exists(orig), "数据文件应挪回原位置")
                self.assertFalse(os.path.exists(data_file), "$R 应被移走")
                # 原位置已存在 → 拒绝（不覆盖）
                with open(os.path.join(tmp, "conflict.txt"), "w") as f:
                    f.write("x")
                self.assertIsNone(server._rb_manual_restore(os.path.join(tmp, "conflict.txt")))
            finally:
                server.PLATFORM = old_plat
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    @unittest.skipUnless(IS_WIN, "回收站还原仅 Windows")
    def test_ps_run_no_window_flags(self):
        """_ps_run 必须带 CREATE_NO_WINDOW + SW_HIDE，杜绝弹出控制台窗口。"""
        captured = {}
        real_run = subprocess.run

        class FakeCompleted:
            returncode = 0
            stdout = b""
            stderr = b""

        def fake_run(cmd, **kw):
            captured.update(kw)
            return FakeCompleted()

        subprocess.run = fake_run
        try:
            server._ps_run("echo hi")
        finally:
            subprocess.run = real_run
        self.assertEqual(captured.get("creationflags"), subprocess.CREATE_NO_WINDOW)
        si = captured.get("startupinfo")
        self.assertIsNotNone(si)
        self.assertEqual(si.wShowWindow, 0, "应 SW_HIDE 隐藏窗口")


class FileEraseTest(_FakeHomeMixin):
    """file_erase：物理删除仅限 _pinvou_filemaster_trash 区域；预览/执行/日志标记。"""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="fm_erase_")
        self.trash = os.path.join(self.tmp, "_pinvou_filemaster_trash")
        os.makedirs(self.trash, exist_ok=True)
        self.victim = os.path.join(self.trash, "20260803-120000_backup.bin")
        with open(self.victim, "wb") as f:
            f.write(b"x" * 100)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_rejects_non_trash_paths(self):
        """非 _pinvou_filemaster_trash 路径拒绝（白名单/区域双重校验）。"""
        outside = os.path.join(self.tmp, "normal.txt")
        with open(outside, "w") as f:
            f.write("x")
        out = server.file_erase(paths=[outside])
        self.assertIn("仅支持删除", out["items"][0]["rejected_reason"])
        # 系统区域仍被白名单拒绝（即使构造在 _pinvou_filemaster_trash 语义下）
        if IS_WIN:
            fake = os.path.join(self.trash, "..", "..", "Windows")
            out = server.file_erase(paths=[os.path.abspath(fake)])
            self.assertFalse(out["items"][0]["allowed"])
        # 通配符/相对路径拒绝
        out = server.file_erase(paths=[os.path.join(self.trash, "*.bin")])
        self.assertFalse(out["items"][0]["allowed"])

    def test_preview_then_erase_and_log_marked(self):
        """confirm=false 预览不执行；confirm=true 异步提交 → 轮询 done → 日志标记 erased。"""
        self.fake_home()
        try:
            server._append_trash_log([{"path": "C:/orig/backup.bin",
                                       "via": "fallback-trash-dir", "dest": self.victim}])
            prev = server.file_erase(paths=[self.victim])
            self.assertEqual(prev["type"], "file_erase_preview")
            self.assertFalse(prev["executed"])
            self.assertTrue(prev["items"][0]["allowed"])
            self.assertTrue(os.path.exists(self.victim), "预览不得删文件")

            sub = server.file_erase(paths=[self.victim], confirm=True)
            self.assertEqual(sub["type"], "file_erase_submitted")
            done = _poll_done(sub["task_id"])
            self.assertEqual(done["kind"], "erase")
            self.assertEqual(done["results"][0]["status"], "erased")
            self.assertFalse(os.path.exists(self.victim), "confirm=true 应物理删除")
            self.assertEqual(done["summary"]["erased"], 1)

            log = server._read_trash_log()
            marked = [e for e in log if e.get("dest") == self.victim]
            self.assertEqual(marked[0]["status"], "erased", "日志应标记 erased")
            # restore list 不再列出 erased 记录
            lst = server.file_restore(action="list")
            self.assertTrue(all(e.get("dest") != self.victim for e in lst["items"]))
        finally:
            self.restore_home()

    def test_prune_empty_trash_container(self):
        """恢复/清空后 _pinvou_filemaster_trash 空容器被清理；非空不动；系统废纸篓路径绝不误删。
        用独立目录隔离 setUp 的 self.trash/self.victim 污染。"""
        tmp = tempfile.mkdtemp(prefix="fm_prune_")
        try:
            # 1) _pinvou_filemaster_trash 内只剩被恢复项（文件已挪走）→ 容器链整体清理
            container = os.path.join(tmp, "_pinvou_filemaster_trash")
            sub = os.path.join(container, "sub")
            os.makedirs(sub, exist_ok=True)
            victim = os.path.join(sub, "restored.bin")
            with open(victim, "wb") as f:
                f.write(b"x")
            os.remove(victim)  # 模拟恢复：备份文件被挪回原位置
            server._prune_empty_trash_container(victim)
            self.assertFalse(os.path.exists(container), "空的 _pinvou_filemaster_trash 容器链应整体清理")
            # 2) _pinvou_filemaster_trash 内还有其他文件 → 不清理
            os.makedirs(container, exist_ok=True)
            keep = os.path.join(container, "keep.bin")
            with open(keep, "wb") as f:
                f.write(b"x")
            server._prune_empty_trash_container(keep)
            self.assertTrue(os.path.exists(container), "非空 _pinvou_filemaster_trash 不得清理")
            # 3) 系统废纸篓路径（.Trash / XDG files/）→ 绝不误删
            sys_trash = os.path.join(tmp, ".Trash")
            os.makedirs(sys_trash, exist_ok=True)
            server._prune_empty_trash_container(os.path.join(sys_trash, "item"))
            self.assertTrue(os.path.exists(sys_trash), "系统废纸篓目录不得被清理")
            xdg_files = os.path.join(tmp, "Trash", "files")
            os.makedirs(xdg_files, exist_ok=True)
            server._prune_empty_trash_container(os.path.join(xdg_files, "item"))
            self.assertTrue(os.path.exists(xdg_files), "XDG files/ 目录不得被清理")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_restore_prunes_empty_trash(self):
        """端到端：恢复后 _pinvou_filemaster_trash 空容器被带走（不留痕迹）。
        用独立目录隔离 setUp 的 self.trash/self.victim 污染。"""
        self.fake_home()
        tmp = tempfile.mkdtemp(prefix="fm_rst_")
        try:
            v = os.path.join(tmp, "outside_restore.txt")
            with open(v, "w", encoding="utf-8") as f:
                f.write("data")
            dest = server._fallback_trash(v)  # 真实 _pinvou_filemaster_trash 兜底路径
            server._append_trash_log([{"path": v, "via": "fallback-trash-dir", "dest": dest}])
            self.assertTrue(os.path.exists(os.path.join(tmp, "_pinvou_filemaster_trash")),
                            "删除后 _pinvou_filemaster_trash 容器应存在")
            rst = server.file_restore(action="restore", path=v)
            self.assertEqual(rst.get("status"), "restored")
            self.assertTrue(os.path.exists(v), "文件应还原")
            self.assertFalse(os.path.exists(os.path.join(tmp, "_pinvou_filemaster_trash")),
                             "恢复后空的 _pinvou_filemaster_trash 容器应被清理（不留痕迹）")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
            self.restore_home()

    def test_erase_directory_recursively(self):
        """目录递归删除：_pinvou_filemaster_trash 下子目录连同日志子记录一并标记。"""
        self.fake_home()
        try:
            sub = os.path.join(self.trash, "subdir")
            os.makedirs(sub, exist_ok=True)
            inner = os.path.join(sub, "inner.bin")
            with open(inner, "wb") as f:
                f.write(b"x")
            server._append_trash_log([{"path": "C:/orig/inner.bin",
                                       "via": "fallback-trash-dir", "dest": inner}])
            sub2 = server.file_erase(paths=[sub], confirm=True)
            done = _poll_done(sub2["task_id"])
            self.assertEqual(done["results"][0]["status"], "erased")
            self.assertFalse(os.path.exists(sub))
            marked = [e for e in server._read_trash_log() if e.get("dest") == inner]
            self.assertEqual(marked[0]["status"], "erased", "子文件日志记录也应标记")
        finally:
            self.restore_home()

    def test_rejects_user_built_trash_without_log(self):
        """自建 _pinvou_filemaster_trash（日志无记录）拒绝删除；有记录的 _pinvou_filemaster_trash 容器=清空整个兜底目录。"""
        self.fake_home()
        try:
            # 用户自建同名 _pinvou_filemaster_trash（无日志记录）→ 拒绝
            custom = os.path.join(self.tmp, "userdata", "_pinvou_filemaster_trash")
            os.makedirs(custom, exist_ok=True)
            keep = os.path.join(custom, "user_own_file.txt")
            with open(keep, "w") as f:
                f.write("keep me")
            out = server.file_erase(paths=[keep])
            self.assertIn("删除日志中", out["items"][0]["rejected_reason"],
                          "无日志记录的自建 _pinvou_filemaster_trash 应拒绝")
            self.assertTrue(os.path.exists(keep), "拒绝后文件必须保留")
            # _pinvou_filemaster_trash 容器本身（有日志记录在容器内）→ 允许 = 清空整个兜底目录（预览可见大小）
            server._append_trash_log([{"path": "C:/orig/backup.bin",
                                       "via": "fallback-trash-dir", "dest": self.victim}])
            out = server.file_erase(paths=[self.trash])
            self.assertTrue(out["items"][0]["allowed"], "有记录的 _pinvou_filemaster_trash 容器应允许（清空兜底）")
            self.assertIn("size_human", out["items"][0])
            # 祖先放大洞回归：有记录的目录祖先（项目目录）拒绝
            out = server.file_erase(paths=[self.tmp])
            self.assertFalse(out["items"][0]["allowed"], "日志落点的祖先目录不得整体擦除")
        finally:
            self.restore_home()

    def test_erased_restore_reports_explicit_error(self):
        """erased 记录 restore：明确报"已被物理删除"，不误走回收站还原。"""
        self.fake_home()
        try:
            server._append_trash_log([{"path": "C:/orig/backup.bin",
                                       "via": "fallback-trash-dir", "dest": self.victim}])
            sub = server.file_erase(paths=[self.victim], confirm=True)
            _poll_done(sub["task_id"])
            rst = server.file_restore(action="restore", path="C:/orig/backup.bin")
            self.assertIn("物理删除", rst.get("error", ""), "应明确提示已被物理删除")
        finally:
            self.restore_home()

    def test_readonly_file_erased_after_chmod(self):
        """只读文件：chmod 后仍可物理删除。"""
        self.fake_home()
        try:
            ro = os.path.join(self.trash, "readonly.bin")
            with open(ro, "wb") as f:
                f.write(b"x")
            os.chmod(ro, 0o444)  # 只读
            server._append_trash_log([{"path": "C:/orig/ro.bin",
                                       "via": "fallback-trash-dir", "dest": ro}])
            sub = server.file_erase(paths=[ro], confirm=True)
            done = _poll_done(sub["task_id"])
            self.assertEqual(done["results"][0]["status"], "erased")
            self.assertFalse(os.path.exists(ro))
        finally:
            self.restore_home()

    @unittest.skipUnless(IS_WIN, "junction 仅 Windows")
    def test_rejects_junction_inside_trash(self):
        """_pinvou_filemaster_trash 内 junction：删除前全树预检拒绝，目标目录内容必须完好。"""
        self.fake_home()
        try:
            target = os.path.join(self.tmp, "precious")
            os.makedirs(target, exist_ok=True)
            keep = os.path.join(target, "keep.txt")
            with open(keep, "w") as f:
                f.write("precious data")
            junc = os.path.join(self.trash, "junc_link")
            r = subprocess.run(["cmd", "/c", "mklink", "/J", junc, target],
                               capture_output=True, timeout=30)
            if r.returncode != 0:
                self.skipTest("mklink /J 失败")
            server._append_trash_log([{"path": "C:/orig/j.bin",
                                       "via": "fallback-trash-dir", "dest": junc}])
            sub = server.file_erase(paths=[junc], confirm=True)
            done = _poll_done(sub["task_id"])
            self.assertEqual(done["results"][0]["status"], "error", "含 junction 应拒绝")
            self.assertIn("reparse", done["results"][0]["error"])
            self.assertTrue(os.path.exists(keep), "junction 目标内容必须完好")
        finally:
            self.restore_home()


class FileEmptyRecycleTest(_FakeHomeMixin):
    def test_preview_never_empties(self):
        """confirm=false 只查占用，绝不清空；三端均返回 preview。"""
        self.fake_home()
        try:
            out = server.file_empty_recycle()
            self.assertEqual(out["type"], "file_empty_recycle_preview")
            self.assertFalse(out["executed"])
            self.assertIn("size_human", out)
            self.assertIn("item_count", out)
        finally:
            self.restore_home()

    def test_linux_trash_full_cycle(self):
        """Linux：XDG Trash 删除 → 预览/清空（fake home 隔离）。"""
        self.fake_home()
        old_plat = server.PLATFORM
        server.PLATFORM = "linux"
        try:
            victim = os.path.join(self._fake_home, "doc.txt")
            with open(victim, "wb") as f:
                f.write(b"x" * 256)
            via, _, dest = server._move_to_recycle(victim)
            self.assertEqual(via, "system-trash")
            self.assertIn("Trash", dest)
            self.assertFalse(os.path.exists(victim))
            server._append_trash_log([{"path": victim, "via": "system-trash", "dest": dest}])
            prev = server.file_empty_recycle()
            self.assertGreaterEqual(prev["item_count"], 1)
            res = server.file_empty_recycle(confirm=True)
            self.assertEqual(res["type"], "file_empty_recycle_result")
            self.assertGreaterEqual(res["emptied_count"], 1)
            files_dir = os.path.join(self._fake_home, ".local", "share", "Trash", "files")
            self.assertEqual(os.listdir(files_dir), [], "清空后 files/ 应为空")
        finally:
            server.PLATFORM = old_plat
            self.restore_home()

    def test_darwin_trash_and_conflict_suffix(self):
        """macOS：~/.Trash 移动 + Finder 风格同名冲突后缀（monkeypatch 平台）。"""
        self.fake_home()
        old_plat = server.PLATFORM
        server.PLATFORM = "darwin"
        try:
            # 同名不同源（不同目录下的 photo.jpg）→ Finder 风格冲突后缀
            sub = os.path.join(self._fake_home, "sub")
            os.makedirs(sub, exist_ok=True)
            a = os.path.join(self._fake_home, "photo.jpg")
            b = os.path.join(sub, "photo.jpg")
            with open(a, "w") as f:
                f.write("img1")
            with open(b, "w") as f:
                f.write("img2")
            _, _, dest1 = server._move_to_recycle(a)
            _, _, dest2 = server._move_to_recycle(b)
            self.assertEqual(os.path.basename(dest1), "photo.jpg")
            self.assertEqual(os.path.basename(dest2), "photo 2.jpg",
                             "同名冲突应加 Finder 风格后缀")
            trash = os.path.join(self._fake_home, ".Trash")
            self.assertTrue(os.path.exists(dest1) and os.path.exists(dest2))
            # 废纸篓根作为 file_trash 目标应拒绝（防内嵌）
            out = server.file_trash(paths=[trash], confirm=False)
            self.assertFalse(out["items"][0]["allowed"])
        finally:
            server.PLATFORM = old_plat
            self.restore_home()


class ProtocolTest(unittest.TestCase):
    """子进程跑 server.py，逐行喂 newline-delimited JSON-RPC。"""

    def _spawn(self):
        env = dict(os.environ)
        env["PYTHONIOENCODING"] = "utf-8"
        return subprocess.Popen(
            [sys.executable, os.path.join(HERE, "server.py")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            universal_newlines=True, encoding="utf-8", errors="replace", env=env)

    def _rpc(self, proc, msg):
        proc.stdin.write(json.dumps(msg) + "\n")
        proc.stdin.flush()
        line = proc.stdout.readline()
        self.assertTrue(line, "server 无响应")
        return json.loads(line)

    def test_full_handshake(self):
        proc = self._spawn()
        try:
            init = self._rpc(proc, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                                    "params": {"protocolVersion": "2024-11-05",
                                               "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}})
            self.assertEqual(init["result"]["serverInfo"]["name"], "pinvou3-file-master")
            self.assertEqual(init["result"]["serverInfo"]["version"], "1.7.0")

            listed = self._rpc(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
            tools = {t["name"]: t for t in listed["result"]["tools"]}
            self.assertEqual(set(tools), {"file_find", "disk_scan", "file_trash",
                                          "file_trash_status", "file_empty_recycle",
                                          "file_erase", "file_restore"})
            self.assertEqual(tools["file_find"]["inputSchema"]["required"], [],
                             "query 可选（空 query + 过滤 = 纯类型搜索）")
            self.assertEqual(tools["file_trash"]["inputSchema"]["required"], ["paths"])
            self.assertIn("limit", tools["file_find"]["inputSchema"]["properties"])
            self.assertIn("confirm", tools["file_trash"]["inputSchema"]["properties"])
            self.assertIn("extensions", tools["file_find"]["inputSchema"]["properties"])
            self.assertIn("modified_after", tools["file_find"]["inputSchema"]["properties"])
            self.assertIn("min_size_mb", tools["file_find"]["inputSchema"]["properties"])
            self.assertIn("sort_by", tools["file_find"]["inputSchema"]["properties"])
            self.assertIn("exclude_dirs", tools["file_find"]["inputSchema"]["properties"])
            self.assertIn("task_id", tools["file_trash_status"]["inputSchema"]["properties"])
            self.assertIn("confirm", tools["file_empty_recycle"]["inputSchema"]["properties"])
            self.assertIn("confirm", tools["file_erase"]["inputSchema"]["properties"])
            self.assertEqual(tools["file_erase"]["inputSchema"]["required"], ["paths"])

            tmp = tempfile.mkdtemp(prefix="fm_rpc_")
            try:
                probe = os.path.join(tmp, "rpc_probe_target.txt")
                with open(probe, "w") as f:
                    f.write("x")
                called = self._rpc(proc, {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                                          "params": {"name": "file_find",
                                                     "arguments": {"query": "rpc_probe", "dir": tmp}}})
                payload = json.loads(called["result"]["content"][0]["text"])
                self.assertTrue(any(r["path"] == probe for r in payload["results"]))

                called = self._rpc(proc, {"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                                          "params": {"name": "file_trash",
                                                     "arguments": {"paths": [probe], "confirm": False}}})
                payload = json.loads(called["result"]["content"][0]["text"])
                self.assertFalse(payload["executed"])
                self.assertTrue(os.path.exists(probe))
            finally:
                shutil.rmtree(tmp, ignore_errors=True)

            bad = self._rpc(proc, {"jsonrpc": "2.0", "id": 5, "method": "tools/call",
                                   "params": {"name": "nope", "arguments": {}}})
            self.assertIn("error", bad)
        finally:
            proc.kill()
            proc.wait()

    def test_ping_and_notifications(self):
        """ping 必须应答 {}；无 id 的 notification 必须静默（流不被破坏）。"""
        proc = self._spawn()
        try:
            # 无 id notification：不应有任何响应
            proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
            proc.stdin.flush()
            # 随后 ping：响应 id 必须是 ping 的 id（若 notification 被响应会先读到它）
            pong = self._rpc(proc, {"jsonrpc": "2.0", "id": 7, "method": "ping"})
            self.assertEqual(pong["id"], 7)
            self.assertEqual(pong["result"], {})

            pong2 = self._rpc(proc, {"jsonrpc": "2.0", "id": 8, "method": "ping"})
            self.assertEqual(pong2["id"], 8)
            # 未知 method → -32601
            bad = self._rpc(proc, {"jsonrpc": "2.0", "id": 9, "method": "bogus"})
            self.assertEqual(bad["error"]["code"], -32601)
        finally:
            proc.kill()
            proc.wait()
            proc.stdin.close()
            proc.stdout.close()

    def test_async_trash_over_stdio(self):
        """子进程里走完整异步链路：confirm=true → submitted → 轮询 done。
        轮询期间 stdout 必须保持纯净（后台 worker 不写 stdout，否则流损坏）。"""
        proc = self._spawn()
        try:
            self._rpc(proc, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                             "params": {"protocolVersion": "2024-11-05",
                                        "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}})
            tmp = tempfile.mkdtemp(prefix="fm_rpc_async_")
            try:
                victim = os.path.join(tmp, "async_victim.txt")
                with open(victim, "w") as f:
                    f.write("x")
                sub = self._rpc(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                                       "params": {"name": "file_trash",
                                                  "arguments": {"paths": [victim], "confirm": True}}})
                payload = json.loads(sub["result"]["content"][0]["text"])
                self.assertEqual(payload["type"], "file_trash_submitted")
                task_id = payload["task_id"]

                deadline = time.monotonic() + 10
                done_payload = None
                while time.monotonic() < deadline:
                    st = self._rpc(proc, {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                                          "params": {"name": "file_trash_status",
                                                     "arguments": {"task_id": task_id}}})
                    done_payload = json.loads(st["result"]["content"][0]["text"])
                    if done_payload["status"] == "done":
                        break
                    time.sleep(0.05)
                self.assertEqual(done_payload["status"], "done", json.dumps(done_payload,
                                                                            ensure_ascii=False))
                self.assertEqual(done_payload["results"][0]["status"], "moved")
            finally:
                shutil.rmtree(tmp, ignore_errors=True)
        finally:
            proc.kill()
            proc.wait()
            proc.stdin.close()
            proc.stdout.close()


class ManifestTest(unittest.TestCase):
    def test_manifest_valid(self):
        with open(os.path.join(HERE, "manifest.json"), encoding="utf-8") as f:
            m = json.load(f)
        self.assertEqual(m["id"], "file-master")
        self.assertEqual(m["command"], "python")
        self.assertEqual(m["args"], ["server.py"])
        self.assertEqual(m["pip_dependencies"], [])
        self.assertEqual(m["mcp_tools"], ["mcp_file_master_file_find",
                                          "mcp_file_master_disk_scan",
                                          "mcp_file_master_file_trash",
                                          "mcp_file_master_file_trash_status",
                                          "mcp_file_master_file_empty_recycle",
                                          "mcp_file_master_file_erase",
                                          "mcp_file_master_file_restore"])
        self.assertEqual(m["companion_skills"], ["file-master"])
        self.assertEqual(len(m["tool_table_entries"]), 7)
        self.assertTrue(m["routing_rules"])
        self.assertEqual(m["version"], "1.7.1")
        # 磁盘扫描总预算 60s + 分组超调余量，须显著大于引擎默认 execute_timeout
        self.assertGreaterEqual(m["execute_timeout"], 90,
                                "扫描类工具应声明更长执行超时（引擎默认 60s 余量不足）")
        for key in ("name", "description", "icon", "category"):
            self.assertTrue(m.get(key), "manifest 缺字段 %s" % key)


if __name__ == "__main__":
    unittest.main(verbosity=2)
