#!/usr/bin/env python3
"""通用 Agent 输出 Gate 校验脚本

约定:
- 从 ../agent_registry.json 读 role 的 outputs + output_schema
- output_schema.properties.<key> 描述 outputs 中文件名 stem 含 <key> 的文件
  示例: schema.properties.inventory 对应 _state/materials_inventory.json
- 找不到对应 key 时，只校验文件存在 + JSON 合法
- outputs 含 '*' 通配（如 _research/*.md）→ 只校验至少 1 个匹配 + 非空

签名:
    python3 validate_agent_output.py <role_id> <project_dir>

输出 (stdout, JSON):
    {
      "role_id": "...",
      "project_dir": "...",
      "verdict": "PASS" | "FAIL",
      "findings": [{"file": "...", "severity": "...", "rule": "...", "message": "..."}]
    }

退出码:
    0 — PASS (无 critical/high findings)
    1 — FAIL (有 critical 或 high findings)
    2 — 参数错误
"""
import json
import sys
from pathlib import Path

REGISTRY_PATH = Path(__file__).resolve().parent.parent / "agent_registry.json"


def load_registry():
    with open(REGISTRY_PATH) as f:
        return json.load(f)


def find_schema_for_file(output_rel, schema_props):
    """按约定 schema.properties.<key> 匹配 output 文件 stem"""
    stem = Path(output_rel).stem  # materials_inventory
    for key, sub_schema in schema_props.items():
        if stem == key or stem.endswith("_" + key) or stem.endswith(key):
            return sub_schema
    return None


def validate_against_schema(data, schema, path_prefix=""):
    """轻量 JSON Schema 校验。支持 type / required / enum / items / properties。"""
    findings = []
    t = schema.get("type")
    if t == "array" and not isinstance(data, list):
        findings.append({"severity": "high", "rule": "type",
                         "message": f"{path_prefix}: 期望 array，实际 {type(data).__name__}"})
        return findings
    if t == "object" and not isinstance(data, dict):
        findings.append({"severity": "high", "rule": "type",
                         "message": f"{path_prefix}: 期望 object，实际 {type(data).__name__}"})
        return findings
    if t == "string" and not isinstance(data, str):
        findings.append({"severity": "high", "rule": "type",
                         "message": f"{path_prefix}: 期望 string"})
        return findings
    if t == "boolean" and not isinstance(data, bool):
        findings.append({"severity": "high", "rule": "type",
                         "message": f"{path_prefix}: 期望 boolean"})
        return findings

    if "enum" in schema and data not in schema["enum"]:
        findings.append({"severity": "high", "rule": "enum",
                         "message": f"{path_prefix}: 值 {data!r} 不在枚举 {schema['enum']}"})

    if isinstance(data, dict) and "required" in schema:
        for req_key in schema["required"]:
            if req_key not in data:
                findings.append({"severity": "high", "rule": "required",
                                 "message": f"{path_prefix}: 缺必填字段 '{req_key}'"})

    if isinstance(data, list) and "items" in schema:
        for i, item in enumerate(data):
            findings.extend(validate_against_schema(item, schema["items"], f"{path_prefix}[{i}]"))

    if isinstance(data, dict) and "properties" in schema:
        for k, sub in schema["properties"].items():
            if k in data:
                findings.extend(validate_against_schema(data[k], sub, f"{path_prefix}.{k}"))

    return findings


def validate_concrete_file(project_dir, output_rel, schema):
    """校验具体文件路径（非通配）。
    对 .json 文件：检查文件存在 + 非空 + JSON 合法 + schema 匹配。
    对其他扩展名（.md/.html/.css 等）：只检查文件存在 + 非空。
    """
    file_path = project_dir / output_rel
    findings = []
    if not file_path.exists():
        return [{"file": output_rel, "severity": "critical", "rule": "missing",
                 "message": "输出文件不存在"}]
    try:
        size = file_path.stat().st_size
    except OSError as e:
        return [{"file": output_rel, "severity": "high", "rule": "stat_failed",
                 "message": str(e)}]
    if size == 0:
        return [{"file": output_rel, "severity": "high", "rule": "empty_file",
                 "message": "文件为空"}]

    # 非 JSON 文件：到此为止，已校验存在 + 非空
    if not output_rel.lower().endswith(".json"):
        return findings

    # JSON 文件：解析 + schema 校验
    try:
        data = json.loads(file_path.read_text(encoding="utf-8"))
    except Exception as e:
        return [{"file": output_rel, "severity": "critical", "rule": "invalid_json",
                 "message": f"JSON 解析失败: {e}"}]
    if schema:
        sub = validate_against_schema(data, schema)
        for f in sub:
            f["file"] = output_rel
            findings.append(f)
    if isinstance(data, list) and len(data) == 0:
        findings.append({"file": output_rel, "severity": "low", "rule": "empty_array",
                         "message": "数组为空（agent 可能没产出内容）"})
    return findings


def validate_glob_pattern(project_dir, output_rel):
    """校验通配模式（如 _research/*.md）"""
    matches = list(project_dir.glob(output_rel))
    findings = []
    if not matches:
        return [{"file": output_rel, "severity": "critical", "rule": "no_match",
                 "message": "通配模式无匹配文件"}]
    for m in matches:
        try:
            size = m.stat().st_size
        except OSError as e:
            findings.append({"file": str(m.relative_to(project_dir)), "severity": "high",
                             "rule": "stat_failed", "message": str(e)})
            continue
        if size == 0:
            findings.append({"file": str(m.relative_to(project_dir)), "severity": "high",
                             "rule": "empty_file", "message": "文件为空"})
    return findings


def main(argv):
    if len(argv) != 3:
        print("Usage: validate_agent_output.py <role_id> <project_dir>", file=sys.stderr)
        return 2

    role_id, project_dir_str = argv[1], argv[2]
    project_dir = Path(project_dir_str).resolve()

    registry = load_registry()
    role = registry.get("agents", {}).get(role_id)
    if not role:
        result = {"role_id": role_id, "verdict": "FAIL",
                  "findings": [{"severity": "critical", "rule": "unknown_role",
                                "message": f"registry 没有 role '{role_id}'"}]}
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 1

    outputs = role.get("outputs", [])
    schema_props = role.get("output_schema", {}).get("properties", {})

    all_findings = []
    for output_rel in outputs:
        if "*" in output_rel or "?" in output_rel:
            all_findings.extend(validate_glob_pattern(project_dir, output_rel))
        else:
            schema = find_schema_for_file(output_rel, schema_props)
            all_findings.extend(validate_concrete_file(project_dir, output_rel, schema))

    blocking = [f for f in all_findings if f["severity"] in ("critical", "high")]
    verdict = "FAIL" if blocking else "PASS"

    result = {
        "role_id": role_id,
        "project_dir": str(project_dir),
        "verdict": verdict,
        "findings": all_findings,
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if verdict == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
