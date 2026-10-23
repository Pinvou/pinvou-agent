#!/usr/bin/env python3
"""Opt-in live-model A/B command generation; never executes model commands.

Export a fixture with CodeWhale's ignored export_shell_guidance_eval_fixture test.
Set SHELL_EVAL_BASE_URL, SHELL_EVAL_API_KEY, and SHELL_EVAL_MODEL in the process
environment, then pass --fixture and --output (a private directory outside git).
Only synthetic task data and repository-owned instructions are sent. Credentials
and provider addresses are never written to output. Review generated commands
before executing them in a disposable workspace. This is a focused simulation,
not a full desktop/Engine end-to-end test.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import copy
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SHARED = "pinvou3-app/src-tauri/resources/common/bundle/instructions-shared.md"
WORK = "pinvou3-app/src-tauri/resources/common/bundle/instructions-work.md"
APP_PLATFORM_MOD = "pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs"


def git_text(repository, ref, path):
    return subprocess.check_output(
        ["git", "-C", str(repository), "show", f"{ref}:{path}"],
        encoding="utf-8",
    )


def baseline_description(ref):
    source = git_text(ROOT / "CodeWhale", ref, "crates/tui/src/tools/shell.rs")
    # The baseline is the exact Rust string literal, not a handwritten paraphrase.
    line = next((line.strip() for line in source.splitlines()
                 if line.strip().startswith('"Execute a shell command in the workspace.')), None)
    if line is None:
        raise ValueError("--engine-base must contain the pre-guidance shell description; "
                         "choose an earlier baseline or use --before-fixture")
    return json.loads(line)


def memory_off_notice():
    """Mirror the production memory-disabled render (memory_section(false)).

    Read MEMORY_OFF_NOTICE_MD out of the Rust source so the fixture cannot
    drift from the shipped prompt; the placeholder used to be dropped to an
    empty string, which stopped matching production when the off-notice was
    introduced (PR #658).
    """
    source = (ROOT / APP_PLATFORM_MOD).read_text(encoding="utf-8")
    match = re.search(r'const MEMORY_OFF_NOTICE_MD: &str = "((?:[^"\\]|\\.)*)";', source)
    if match is None:
        raise ValueError("MEMORY_OFF_NOTICE_MD not found in " + APP_PLATFORM_MOD)
    literal = match.group(1)
    out = []
    i = 0
    while i < len(literal):
        ch = literal[i]
        if ch == "\\" and i + 1 < len(literal):
            nxt = literal[i + 1]
            if nxt == "n":
                out.append("\n")
                i += 2
                continue
            if nxt == "t":
                out.append("\t")
                i += 2
                continue
            if nxt in ('"', "\\"):
                out.append(nxt)
                i += 2
                continue
        out.append(ch)
        i += 1
    return "".join(out)


def instructions(shared, work, model, title_language):
    sections = work.split("\n\n")
    return (shared.replace("{{PINVOU3_MODE_ENV_SECTION}}", "\n\n".join(sections[:2]))
            .replace("{{PINVOU3_MODE_ARTIFACT_RULE}}", "\n\n".join(sections[2:]))
            .replace("{{PINVOU3_MODEL}}", model)
            .replace("{{PINVOU3_TITLE_LANG}}", title_language)
            # Synthetic shell comparisons use the production memory-disabled
            # prompt; never read the operator's personal memory settings.
            .replace("{{PINVOU3_MEMORY_SECTION}}\n", memory_off_notice())
            .replace("{{PINVOU3_SUDO_INSTRUCTION}}", ""))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--before-fixture", type=Path,
                        help="Compare two tool fixtures with identical current app instructions")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--app-base", help="Explicit app baseline for the before arm")
    parser.add_argument("--engine-base", help="Explicit pre-guidance engine baseline")
    parser.add_argument("--language", choices=("zh-Hans", "en", "ja"), default="zh-Hans",
                        help="Application title language (default: zh-Hans)")
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--workers", type=int, default=3)
    parser.add_argument("--arms", nargs="+", choices=("before", "after"), default=["before", "after"])
    parser.add_argument("--fixture-port", type=int, default=18765)
    parser.add_argument("--tasks", nargs="+",
                        choices=("http_preview", "json_fields", "csv_sum", "quoted_path",
                                 "verified_interpreter_reuse", "script_transport", "probe_outcomes"),
                        default=("http_preview", "json_fields", "csv_sum", "quoted_path"))
    args = parser.parse_args()
    if "before" in args.arms and not args.before_fixture:
        if not args.app_base or not args.engine_base:
            parser.error("the before arm requires --app-base and --engine-base, "
                         "or --before-fixture")
    if args.repeats < 1 or args.workers < 1:
        parser.error("--repeats and --workers must be positive")
    missing = [name for name in ("SHELL_EVAL_BASE_URL", "SHELL_EVAL_MODEL")
               if not os.environ.get(name, "").strip()]
    if missing:
        parser.error("missing required environment variables: " + ", ".join(missing))
    model = os.environ["SHELL_EVAL_MODEL"]
    title_language = {"zh-Hans": "简体中文", "en": "English", "ja": "日本語"}[args.language]
    if ROOT == args.output.resolve() or ROOT in args.output.resolve().parents:
        parser.error("--output must be outside the repository")
    args.output.mkdir(parents=True, exist_ok=True)
    current = json.loads(args.fixture.read_text(encoding="utf-8"))
    current_prompt = instructions((ROOT / SHARED).read_text(encoding="utf-8"),
                                  (ROOT / WORK).read_text(encoding="utf-8"),
                                  model, title_language)
    previous = None
    prompts = {"after": current_prompt}
    if "before" in args.arms:
        if args.before_fixture:
            previous = json.loads(args.before_fixture.read_text(encoding="utf-8"))
            if previous["shell"] != current["shell"]:
                parser.error("both fixtures must declare the same execution shell")
            prompts["before"] = current_prompt
        else:
            previous = copy.deepcopy(current)
            try:
                previous["description"] = baseline_description(args.engine_base)
                prompts["before"] = instructions(git_text(ROOT, args.app_base, SHARED),
                                                 git_text(ROOT, args.app_base, WORK),
                                                 model, title_language)
            except (ValueError, subprocess.CalledProcessError):
                parser.error("cannot load the before baseline; --engine-base must contain "
                             "the pre-guidance description and --app-base must contain "
                             "the app instructions, or supply --before-fixture")
            previous["input_schema"]["properties"]["command"]["description"] = (
                "The shell command to execute (action=run)"
            )
    url = f"http://127.0.0.1:{args.fixture_port}/weather.json"
    tasks = {
        "http_preview": f"读取测试天气接口 {url}，超时15秒，只输出响应文本的前2000个字符。",
        "json_fields": f"从测试天气接口 {url} 读取并输出 current_condition 第一项的 temp_C 和 humidity，超时15秒。",
        "csv_sum": "读取当前目录的 sales.csv，求 amount 列总和并输出结果。",
        "quoted_path": "读取当前目录下的 中文 folder/sample data.txt，只输出文件的前3行。",
        "verified_interpreter_reuse": (
            "同一测试环境中，前次探针已确认 C:/shell-eval/python/python.exe 能输出 Python 3.13.14；"
            "python3 别名则是不可运行的占位符。请复用已验证的完整入口，计算并输出 2+3。"
            "不要重新探测别名、安装依赖或因另一个别名失败而报告 Python 不可用。"
        ),
        "script_transport": (
            "只做本机回显，不连接 SSH 或执行远端操作；powershell.exe 已验证可运行。"
            "通过工具的 stdin 参数把下面的 PowerShell 脚本明文传给另一个 PowerShell 进程"
            "（-NoProfile -Command -），不要修改脚本、写文件或转为编码后解码执行。"
            "保留空格、引号、美元符号、中文和换行，只输出脚本中的文本。脚本：\n"
            "$text = @'\nspace \"quote\" $HOME\n第二行\n'@\n[Console]::Write($text)\n"
        ),
        "probe_outcomes": (
            "执行三个独立、无副作用的内存检查：必需 A 确认 1+1=2；"
            "必需 B 确认 2+2=5（该检查明确应失败）；"
            "可选 C 在仅包含 ok 的内存字符串中查找 absent（无匹配为预期）。"
            "分别报告成功、失败或不适用及原因，最后汇总必需检查状态。"
            "必需 B 的失败必须使脚本进程非零退出，不能由 catch 或最后的可选检查覆盖。"
        ),
    }
    key = os.environ.get("SHELL_EVAL_API_KEY", "")
    if args.tasks:
        tasks = {name: tasks[name] for name in args.tasks}
    base = os.environ["SHELL_EVAL_BASE_URL"].rstrip("/")
    endpoint = base if base.endswith("/chat/completions") else base + "/chat/completions"

    def generate(arm, task, repeat):
        fixture = previous if arm == "before" else current
        system = (f"## Environment\n- platform: windows\n- shell: {current['shell']}\n\n"
                  + prompts[arm]
                  + "\nThis is a synthetic command compatibility test. Return exactly one foreground "
                  "Bash run call that completes the read-only task. Do not modify files, install "
                  "programs, or access services other than the supplied loopback fixture. "
                  "The current directory is the disposable test workspace.")
        request = {
            "model": model,
            "messages": [{"role": "system", "content": system},
                         {"role": "user", "content": tasks[task]}],
            "tools": [{"type": "function", "function": {
                "name": fixture["name"], "description": fixture["description"],
                "parameters": fixture["input_schema"],
            }}],
            "tool_choice": {"type": "function", "function": {"name": "Bash"}},
            "max_tokens": 1800,
            "stream": False,
        }
        if model.startswith("deepseek"):
            request["thinking"] = {"type": "disabled"}
        run_id = f"{arm}-{task}-{repeat}"
        (args.output / f"{run_id}.request.json").write_text(
            json.dumps(request, ensure_ascii=False, indent=2), encoding="utf-8")
        headers = {"Content-Type": "application/json"}
        if key:
            headers["Authorization"] = "Bearer " + key
        record = {"id": run_id, "arm": arm, "task": task, "repeat": repeat}
        try:
            req = urllib.request.Request(endpoint, json.dumps(request).encode(), headers)
            with urllib.request.urlopen(req, timeout=120) as response:
                body = json.load(response)
            calls = body["choices"][0]["message"].get("tool_calls", [])
            record["usage"] = body.get("usage", {})
            if len(calls) != 1 or calls[0]["function"]["name"] != "Bash":
                record["error"] = "expected_one_bash_call"
            else:
                record["arguments"] = json.loads(calls[0]["function"]["arguments"])
        except urllib.error.HTTPError as error:
            record["error"] = f"http_{error.code}"
        except Exception as error:
            record["error"] = type(error).__name__
        (args.output / f"{run_id}.result.json").write_text(
            json.dumps(record, ensure_ascii=False, indent=2), encoding="utf-8")
        return record

    runs = [(arm, task, repeat) for repeat in range(args.repeats)
            for task in tasks for arm in args.arms]
    records = []
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        for future in as_completed([pool.submit(generate, *run) for run in runs]):
            record = future.result()
            records.append(record)
            print(record["id"], record.get("error", "generated"), flush=True)
    (args.output / "generation.json").write_text(
        json.dumps({"model": model, "shell": current["shell"], "runs": records},
                   ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"Generated {sum('arguments' in r for r in records)}/{len(records)} calls; none executed.")


if __name__ == "__main__":
    main()
