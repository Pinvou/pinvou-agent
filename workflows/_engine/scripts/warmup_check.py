#!/usr/bin/env python3
import importlib.util
import json
import os
from pathlib import Path
import sys
import time
import urllib.request

# 通用项目骨架(HTML_Deck 等具体工作流产物目录由各自角色按需建,不在引擎预检里)
REQUIRED_DIRS = ["_state", "_extracted", "_research", "_audit"]
PY_DEPS = [
    ("markitdown", "markitdown[all]"),
    ("pdfplumber", "pdfplumber"),
    ("PIL", "Pillow"),
    ("patoolib", "patool"),
]
# 引擎自身脚本清单(workflows/_engine/scripts 实际内容;具体工作流专属脚本
# 不在此列——它们缺失由对应角色/验证器自己报错)
REQUIRED_SCRIPTS = [
    "scheduler.py",
    "workflow_state.py",
    "dispatch_graph.py",
    "validate_deliverable.py",
    "validate_agent_output.py",
    "warmup_check.py",
    "workflow_logger.py",
]
REQUIRED_ENVS = ["DEEPSEEK_BASE_URL", "DEEPSEEK_MODEL", "PINVOU3_SEARCH_API_KEY"]


def item(status, details, remedy=""):
    return {"status": status, "details": details, "remedy": remedy}


def check_dirs(project):
    fixed = []
    for name in REQUIRED_DIRS:
        path = project / name
        if not path.exists():
            path.mkdir(parents=True, exist_ok=True)
            fixed.append(name)
    if fixed:
        return item("fixed", "created missing directories: " + ", ".join(fixed), "mkdir -p completed")
    return item("pass", "all required directories exist")


def check_python_deps():
    """只检测不自动安装。
    GB10 是 PEP 668 externally-managed Python，自动 pip install 会失败且污染系统。
    缺包时给三种 manual remedy 让用户选。
    """
    missing = [(mod, pkg) for mod, pkg in PY_DEPS if importlib.util.find_spec(mod) is None]
    if not missing:
        return item("pass", "all Python dependencies import successfully")
    missing_pkgs = " ".join(pkg for _, pkg in missing)
    remedy = (
        "Choose one (A recommended for first-time setup):\n"
        f"  A) python3 -m venv ~/.pinvou3/venv && ~/.pinvou3/venv/bin/pip install {missing_pkgs}\n"
        f"     then export PATH=~/.pinvou3/venv/bin:$PATH\n"
        f"  B) pipx install <each-pkg-individually> (for CLI tools like markitdown)\n"
        f"  C) pip install --break-system-packages {missing_pkgs}  # not recommended, pollutes system Python"
    )
    # internal_quick 场景不需要 markitdown/pdfplumber/patool（无 materials_auditor 角色）。
    # 把缺包降级为 warn，让 workflow 继续；用到时再阻塞具体角色。
    return item(
        "warn",
        f"missing Python packages: {', '.join(pkg for _, pkg in missing)} (workflow 可继续，缺包角色会自己报错)",
        remedy,
    )


def check_scripts():
    base = Path(__file__).resolve().parent
    missing = [name for name in REQUIRED_SCRIPTS if not (base / name).exists()]
    if missing:
        return item("blocked", "missing scripts: " + ", ".join(missing), "Restore missing engine scripts under workflows/_engine/scripts (源) 并重新装配 bundle")
    return item("pass", "all required scripts exist")


def check_env():
    missing = [name for name in REQUIRED_ENVS if not os.environ.get(name)]
    if missing:
        return item("blocked", "missing environment variables: " + ", ".join(missing), "Export required environment variables before starting workflow")
    return item("pass", "all required environment variables are set")


def check_llm_endpoint():
    base = os.environ.get("DEEPSEEK_BASE_URL", "").rstrip("/")
    if not base:
        return item("blocked", "DEEPSEEK_BASE_URL is missing", "Set DEEPSEEK_BASE_URL")
    # DEEPSEEK_BASE_URL 通常已含 /v1（OpenAI 兼容惯例，launch 脚本与 bridge fallback 都带），
    # 直接拼 /v1/models 会变成 /v1/v1/models → 404。base 已以 /v1 结尾时只补 /models。
    url = base + "/models" if base.endswith("/v1") else base + "/v1/models"
    # 抖动 / 大模型冷启动容忍：最多 3 次探测（每次 10s timeout，失败间隔 2s），
    # 任一次成功即 pass；3 次全失败才判 blocked。避免单次网络抖动误卡 harness。
    last = None
    for attempt in range(3):
        try:
            with urllib.request.urlopen(url, timeout=10) as resp:
                if 200 <= resp.status < 400:
                    suffix = f" (attempt {attempt + 1}/3)" if attempt else ""
                    return item("pass", f"LLM endpoint reachable: {url}{suffix}")
                last = f"HTTP {resp.status}"
        except Exception as exc:
            last = str(exc)
        if attempt < 2:
            time.sleep(2)
    return item("blocked", f"LLM endpoint unreachable after 3 tries: {last}", "Start/check LLM API service and DEEPSEEK_BASE_URL")


def check_search_endpoint():
    key = os.environ.get("PINVOU3_SEARCH_API_KEY")
    if not key:
        return item("warn", "PINVOU3_SEARCH_API_KEY missing; Bocha reachability skipped", "Set PINVOU3_SEARCH_API_KEY to enable web search")
    req = urllib.request.Request(
        "https://api.bochaai.com/v1/ai-search",
        method="HEAD",
        headers={"Authorization": f"Bearer {key}"},
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            if 200 <= resp.status < 500:
                return item("pass", f"Bocha endpoint reachable with HTTP {resp.status}")
            # 4xx 当 warn —— Bocha 经常 405 但实际 ai-search POST 工作正常
            return item("warn", f"Bocha HEAD returned HTTP {resp.status}; actual POST may still work", "Search 在 workflow 中按需检查")
    except Exception as exc:
        return item("warn", f"Bocha endpoint unreachable: {exc} (workflow 可降级运行)", "Check network and PINVOU3_SEARCH_API_KEY")


def check_materials(project):
    materials = project / "配套材料"
    if not materials.exists():
        return item("warn", "配套材料/ directory does not exist", "Create 配套材料/ and add source files when available")
    files = [p for p in materials.glob("**/*") if p.is_file()]
    if not files:
        return item("warn", "配套材料/ exists but is empty", "Add source materials when available; workflow may continue")
    return item("pass", f"配套材料/ contains {len(files)} file(s)")


def main(argv):
    if len(argv) != 2:
        print("Usage: python3 warmup_check.py <project_dir>", file=sys.stderr)
        return 1
    project = Path(argv[1]).resolve()
    project.mkdir(parents=True, exist_ok=True)
    state = project / "_state"
    state.mkdir(parents=True, exist_ok=True)
    checks = {
        "directory_skeleton": check_dirs(project),
        "python_dependencies": check_python_deps(),
        "script_presence": check_scripts(),
        "environment_variables": check_env(),
        "llm_endpoint": check_llm_endpoint(),
        "search_api": check_search_endpoint(),
        "materials_directory": check_materials(project),
    }
    report = {"project_dir": str(project), "checks": checks}
    report["status"] = "blocked" if any(v["status"] == "blocked" for v in checks.values()) else "pass"
    out = state / "warmup_report.json"
    out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 1 if report["status"] == "blocked" else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
