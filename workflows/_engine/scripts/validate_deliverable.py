#!/usr/bin/env python3
"""validate_deliverable.py — 每个角色的交付物检查器

每个角色做完后，harness 调这个脚本检查产出是否合格。
不是"内容好不好"——那靠人判断。这里只检查"结构对不对、该有的都有没有"。

用法:
    python3 validate_deliverable.py <project_dir> <role_id>

输出: JSON {verdict, findings[]}
退出码: 0=PASS, 1=WARN, 2=FAIL
"""

import json
import os
import re
import sys
from pathlib import Path


def check_json_file(path: Path, required_fields: list[str] = None) -> list[dict]:
    """检查 JSON 文件：存在 + 可解析 + 必填字段。"""
    findings = []
    if not path.exists():
        findings.append(critical(f"文件不存在: {path.name}"))
        return findings
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except (json.JSONDecodeError, UnicodeDecodeError) as e:
        findings.append(critical(f"{path.name} JSON 解析失败: {e}"))
        return findings

    if required_fields:
        if isinstance(data, dict):
            for field in required_fields:
                if field not in data or data[field] is None or data[field] == "":
                    findings.append(critical(f"{path.name} 缺少必填字段: {field}"))
        else:
            findings.append(warn(f"{path.name} 顶层不是对象，无法检查字段"))

    return findings


def check_md_file(path: Path, min_lines: int = 5, required_sections: list[str] = None) -> list[dict]:
    """检查 Markdown 文件：存在 + 非空 + 必须节。"""
    findings = []
    if not path.exists():
        findings.append(critical(f"文件不存在: {path.name}"))
        return findings
    text = path.read_text(encoding="utf-8")
    lines = text.strip().split("\n")
    if len(lines) < min_lines:
        findings.append(critical(f"{path.name} 只有 {len(lines)} 行，太短（最少 {min_lines}）"))

    if required_sections:
        for section in required_sections:
            if section.lower() not in text.lower():
                findings.append(warn(f"{path.name} 缺少节: {section}"))

    return findings



def check_file_exists(path: Path, min_bytes: int = 10) -> list[dict]:
    """检查文件存在且非空。"""
    findings = []
    if not path.exists():
        findings.append(critical(f"文件不存在: {path.name}"))
    elif path.stat().st_size < min_bytes:
        findings.append(critical(f"{path.name} 文件太小（{path.stat().st_size} bytes）"))
    return findings






# ── finding 工厂 ──

def critical(msg: str, fix: str = "") -> dict:
    return {"severity": "CRITICAL", "message": msg, "fix_hint": fix, "rollback_scope": "local"}

def warn(msg: str, fix: str = "") -> dict:
    return {"severity": "WARNING", "message": msg, "fix_hint": fix, "rollback_scope": "local"}

def info(msg: str) -> dict:
    return {"severity": "INFO", "message": msg, "fix_hint": "", "rollback_scope": "local"}


# ── 真相源:从 agent_registry.json 的 output_schema 读校验规则 ──
# 不在本脚本里重复写"要哪些字段"——那是 registry 的职责(唯一真相源)。
# submit_output(落盘前校验) 和这里(L1 落盘后校验) 共用同一份 output_schema,
# 永不打架。registry 怎么改,质检自动跟着改。

def _load_registry() -> dict:
    """读 agent_registry.json(脚本同级目录的上一层就是 workflow 根)。"""
    # 脚本位于 <workflow>/scripts/,registry 在 <workflow>/agent_registry.json
    reg_path = Path(__file__).resolve().parent.parent / "agent_registry.json"
    try:
        with open(reg_path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return {}


def required_fields_from_schema(role_id: str) -> list[str] | None:
    """取该角色 output_schema 顶层 required 字段清单。
    返回 None = registry 没有标准 schema(自由文本角色),调用方退回旧逻辑。"""
    reg = _load_registry()
    schema = (
        reg.get("agents", {})
        .get(role_id, {})
        .get("output_schema")
    )
    if not isinstance(schema, dict):
        return None
    if schema.get("type") != "object" or "properties" not in schema:
        return None
    req = schema.get("required")
    return req if isinstance(req, list) else []



# ── 每个角色的检查逻辑 ──

ROLE_VALIDATORS = {}

def role_validator(role_id):
    def decorator(fn):
        ROLE_VALIDATORS[role_id] = fn
        return fn
    return decorator


# (h3c-ppt 的 10 个角色验证器随该工作流 2026-06-11 存档移除;
#  新工作流的验证器按下方三省六部的写法注册)

# ── 三省六部（sansheng_liubu 场景，edict 官制移植）──

@role_validator("taizi")
def validate_taizi(project: Path) -> list[dict]:
    return check_json_file(
        project / "_state" / "zhiyi.json",
        required_fields=["title", "objective", "constraints", "original_request"],
    )


@role_validator("zhongshu")
def validate_zhongshu(project: Path) -> list[dict]:
    return check_json_file(
        project / "plan.json",
        required_fields=["objective", "steps", "risks", "expected_outputs"],
    )


@role_validator("menxia")
def validate_menxia(project: Path) -> list[dict]:
    """门下省封驳机制（edict 三省制移植）：
    - verdict=准奏 → PASS
    - verdict=封驳 且 rollback_counts['zhongshu:fengbo']（缺省0）< 2
      → CRITICAL structural finding(violation_type=fengbo) → harness 回滚 zhongshu 重新起草
    - verdict=封驳 但计数已 ≥2 → PASS 放行（第 3 轮强制准奏，echo 一条 note）
    三轮规则在 menxia role prompt 也写明（prompt 层 + 校验层双保险）。"""
    shenyi_path = project / "_state" / "shenyi.json"
    f = check_json_file(
        shenyi_path,
        required_fields=["verdict", "dimensions"],
    )
    if f:
        return f
    try:
        shenyi = json.loads(shenyi_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        return [critical(f"shenyi.json 读取失败: {e}")]

    verdict = shenyi.get("verdict")
    if verdict not in ("准奏", "封驳"):
        return [critical(f"shenyi.json verdict 非法: {verdict!r}（只能是 准奏/封驳）")]
    if verdict == "准奏":
        return []

    # verdict == 封驳 → 查回滚计数（_state/workflow_progress.json，与 workflow_state.apply_rollback 同源）
    fengbo_count = 0
    progress_path = project / "_state" / "workflow_progress.json"
    try:
        progress = json.loads(progress_path.read_text(encoding="utf-8"))
        fengbo_count = int(progress.get("rollback_counts", {}).get("zhongshu:fengbo", 0))
    except (OSError, json.JSONDecodeError, ValueError, TypeError):
        fengbo_count = 0

    if fengbo_count >= 2:
        # 第 3 轮：强制准奏放行，不再回滚（不能走 BLOCKED）
        return [info(
            f"门下省第 {fengbo_count + 1} 轮仍封驳，按三轮规则【强制准奏】放行；"
            f"issues 作为改进建议随 shenyi.json 留档"
        )]

    issues = shenyi.get("issues") or []
    issues_digest = "；".join(str(i) for i in issues[:5])[:300] or "（门下省未列具体 issues）"
    return [{
        "severity": "CRITICAL",
        "message": f"门下省封驳（第 {fengbo_count + 1} 轮）：{issues_digest}",
        "fix_hint": "回滚中书省重新起草：必须读 _state/shenyi.json 逐条回应 issues",
        "rollback_scope": "structural",
        "violation_type": "fengbo",
    }]


@role_validator("shangshu")
def validate_shangshu(project: Path) -> list[dict]:
    return check_json_file(
        project / "dispatch.json",
        required_fields=["assignments", "skip_bus"],
    )


def _validate_bu(project: Path, bu: str) -> list[dict]:
    """六部统一校验：deliverables/<bu>.md 存在+非空（有差事=产出；无差事=一行声明）。"""
    return check_file_exists(project / "deliverables" / f"{bu}.md", min_bytes=10)


@role_validator("hubu")
def validate_hubu(project: Path) -> list[dict]:
    return _validate_bu(project, "hubu")


@role_validator("libu")
def validate_libu(project: Path) -> list[dict]:
    return _validate_bu(project, "libu")


@role_validator("bingbu")
def validate_bingbu(project: Path) -> list[dict]:
    return _validate_bu(project, "bingbu")


@role_validator("xingbu")
def validate_xingbu(project: Path) -> list[dict]:
    return _validate_bu(project, "xingbu")


@role_validator("gongbu")
def validate_gongbu(project: Path) -> list[dict]:
    return _validate_bu(project, "gongbu")


@role_validator("libu_renshi")
def validate_libu_renshi(project: Path) -> list[dict]:
    return _validate_bu(project, "libu_renshi")


@role_validator("huizou")
def validate_huizou(project: Path) -> list[dict]:
    return check_md_file(project / "final_report.md", min_lines=5)


# ── 主入口 ──

def validate(project_dir: str, role_id: str) -> dict:
    project = Path(project_dir).resolve()
    # [B2] 差事节点(<bu>~<seq>):校验其专属产物 deliverables/<bu>_<seq>.md 存在且非空。
    # 用 ~ 不用 # 分隔,避开 per_page 页实例(<role>#pNN)的 # 约定,防引擎误判。
    if "~" in role_id:
        bu, _, seq = role_id.partition("~")
        rel = f"deliverables/{bu}_{seq}.md"
        out = project / rel
        if not out.exists():
            return {"role_id": role_id, "verdict": "FAIL", "findings": [critical(f"文件不存在: {rel}")]}
        if out.stat().st_size == 0:
            return {"role_id": role_id, "verdict": "FAIL", "findings": [critical(f"文件为空: {rel}")]}
        return {"role_id": role_id, "verdict": "PASS", "findings": []}
    validator = ROLE_VALIDATORS.get(role_id)
    if not validator:
        return {
            "role_id": role_id,
            "verdict": "SKIP",
            "message": f"无 {role_id} 的验证器",
            "findings": [],
        }

    findings = validator(project)
    has_critical = any(f["severity"] == "CRITICAL" for f in findings)
    has_warn = any(f["severity"] == "WARNING" for f in findings)

    if has_critical:
        verdict = "FAIL"
    elif has_warn:
        verdict = "WARN"
    else:
        verdict = "PASS"

    return {
        "role_id": role_id,
        "verdict": verdict,
        "findings": findings,
        "summary": {
            "critical": sum(1 for f in findings if f["severity"] == "CRITICAL"),
            "warning": sum(1 for f in findings if f["severity"] == "WARNING"),
        },
    }


def main():
    if len(sys.argv) < 3:
        print(f"用法: {sys.argv[0]} <project_dir> <role_id>", file=sys.stderr)
        sys.exit(1)

    result = validate(sys.argv[1], sys.argv[2])
    print(json.dumps(result, ensure_ascii=False, indent=2))

    if result["verdict"] == "FAIL":
        sys.exit(2)
    elif result["verdict"] == "WARN":
        sys.exit(1)
    else:
        sys.exit(0)


if __name__ == "__main__":
    main()
