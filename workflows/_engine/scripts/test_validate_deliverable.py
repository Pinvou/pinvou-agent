#!/usr/bin/env python3
"""validate_deliverable 单测 — 工件清单物理核验。

报告自声明的二进制工件(「## 工件清单」段)必须真实存在且非空,
堵"报告说生成了但文件不存在"的静默假成功。
"""
from __future__ import annotations

import tempfile
from pathlib import Path

from validate_deliverable import check_artifact_manifest, validate


def _ok(msg: str) -> None:
    print(f"  ✅ {msg}")


def _project_with_report(tmp: str, body: str) -> Path:
    p = Path(tmp)
    (p / "deliverables").mkdir(exist_ok=True)
    (p / "deliverables" / "gongbu_1.md").write_text(body, encoding="utf-8")
    return p


def test_missing_artifact_fails():
    with tempfile.TemporaryDirectory() as d:
        _project_with_report(d, "# 报告\n\n## 工件清单\n- `deliverables/x.pptx` — 21页\n")
        r = validate(d, "gongbu~1")
        assert r["verdict"] == "FAIL", r
        assert "不存在" in r["findings"][0]["message"], r
    _ok("声明了工件但文件不存在 → FAIL")


def test_present_artifact_passes():
    with tempfile.TemporaryDirectory() as d:
        p = _project_with_report(d, "# 报告\n\n## 工件清单\n- `deliverables/x.pptx`\n")
        (p / "deliverables" / "x.pptx").write_bytes(b"PK fake pptx")
        r = validate(d, "gongbu~1")
        assert r["verdict"] == "PASS", r
    _ok("工件存在且非空 → PASS")


def test_empty_artifact_fails():
    with tempfile.TemporaryDirectory() as d:
        p = _project_with_report(d, "# 报告\n\n## 工件清单\n- `deliverables/x.pptx`\n")
        (p / "deliverables" / "x.pptx").write_bytes(b"")
        r = validate(d, "gongbu~1")
        assert r["verdict"] == "FAIL", r
    _ok("工件为空 → FAIL")


def test_no_manifest_section_skips_check():
    with tempfile.TemporaryDirectory() as d:
        _project_with_report(d, "# 报告\n\n正文若干,无工件清单段。\n")
        r = validate(d, "gongbu~1")
        assert r["verdict"] == "PASS", r
    _ok("报告无工件清单段 → 不检查,PASS(纯文档差事常态)")


def test_path_traversal_rejected():
    with tempfile.TemporaryDirectory() as d:
        _project_with_report(d, "# 报告\n\n## 工件清单\n- `../../etc/passwd`\n")
        r = validate(d, "gongbu~1")
        assert r["verdict"] == "FAIL", r
        assert "越界" in r["findings"][0]["message"], r
    _ok("工件路径越界 → FAIL(路径穿越防线)")


def test_section_ends_at_next_heading():
    with tempfile.TemporaryDirectory() as d:
        p = _project_with_report(
            d,
            "# 报告\n\n## 工件清单\n- `deliverables/x.pptx`\n\n## 验证说明\n- `deliverables/不存在.bin` 这是说明不是声明\n",
        )
        (p / "deliverables" / "x.pptx").write_bytes(b"PK")
        r = validate(d, "gongbu~1")
        assert r["verdict"] == "PASS", r
    _ok("清单段到下一个标题为止,后续段落的路径不误判")


def test_static_bu_validator_also_checks():
    with tempfile.TemporaryDirectory() as d:
        p = Path(d)
        (p / "deliverables").mkdir()
        (p / "deliverables" / "gongbu.md").write_text(
            "# 报告\n\n## 工件清单\n- `deliverables/y.xlsx`\n", encoding="utf-8"
        )
        findings = check_artifact_manifest(p, p / "deliverables" / "gongbu.md")
        assert findings and "不存在" in findings[0]["message"], findings
    _ok("静态六部校验路径同样核验工件清单")


if __name__ == "__main__":
    print("工件清单核验单测:")
    fns = [v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)]
    for fn in fns:
        fn()
    print(f"\n全部 {len(fns)} 项通过 ✅")
