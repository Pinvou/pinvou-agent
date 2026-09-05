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
import subprocess
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SHARED = "pinvou3-app/src-tauri/resources/common/bundle/instructions-shared.md"
WORK = "pinvou3-app/src-tauri/resources/common/bundle/instructions-work.md"


def git_text(repository, ref, path):
    return subprocess.check_output(
        ["git", "-C", str(repository), "show", f"{ref}:{path}"],
        encoding="utf-8",
    )


def baseline_description(ref):
    source = git_text(ROOT / "CodeWhale", ref, "crates/tui/src/tools/shell.rs")
    # The baseline is the exact Rust string literal, not a handwritten paraphrase.
    line = next(line.strip() for line in source.splitlines()
                if line.strip().startswith('"Execute a shell command in the workspace.'))
    return json.loads(line)


def instructions(shared, work):
    sections = work.split("\n\n")
    return (shared.replace("{{PINVOU3_MODEL}}", os.environ["SHELL_EVAL_MODEL"])
            .replace("{{PINVOU3_MODE_ENV_SECTION}}", "\n\n".join(sections[:2]))
            .replace("{{PINVOU3_MODE_ARTIFACT_RULE}}", "\n\n".join(sections[2:]))
            .replace("{{PINVOU3_SUDO_INSTRUCTION}}", ""))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--before-fixture", type=Path,
                        help="Compare two tool fixtures with identical current app instructions")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--app-base", default="HEAD")
    parser.add_argument("--engine-base", default="HEAD")
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--workers", type=int, default=3)
    parser.add_argument("--arms", nargs="+", choices=("before", "after"), default=["before", "after"])
    parser.add_argument("--fixture-port", type=int, default=18765)
    parser.add_argument("--tasks", nargs="+",
                        choices=("http_preview", "json_fields", "csv_sum", "quoted_path"))
    args = parser.parse_args()
    if ROOT == args.output.resolve() or ROOT in args.output.resolve().parents:
        parser.error("--output must be outside the repository")
    args.output.mkdir(parents=True, exist_ok=True)
    current = json.loads(args.fixture.read_text(encoding="utf-8"))
    current_prompt = instructions((ROOT / SHARED).read_text(encoding="utf-8"),
                                  (ROOT / WORK).read_text(encoding="utf-8"))
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
            previous["description"] = baseline_description(args.engine_base)
            previous["input_schema"]["properties"]["command"]["description"] = (
                "The shell command to execute (action=run)"
            )
            prompts["before"] = instructions(git_text(ROOT, args.app_base, SHARED),
                                             git_text(ROOT, args.app_base, WORK))
    url = f"http://127.0.0.1:{args.fixture_port}/weather.json"
    tasks = {
        "http_preview": f"读取测试天气接口 {url}，超时15秒，只输出响应文本的前2000个字符。",
        "json_fields": f"从测试天气接口 {url} 读取并输出 current_condition 第一项的 temp_C 和 humidity，超时15秒。",
        "csv_sum": "读取当前目录的 sales.csv，求 amount 列总和并输出结果。",
        "quoted_path": "读取当前目录下的 中文 folder/sample data.txt，只输出文件的前3行。",
    }
    key = os.environ.get("SHELL_EVAL_API_KEY", "")
    if args.tasks:
        tasks = {name: tasks[name] for name in args.tasks}
    base = os.environ["SHELL_EVAL_BASE_URL"].rstrip("/")
    endpoint = base if base.endswith("/chat/completions") else base + "/chat/completions"
    model = os.environ["SHELL_EVAL_MODEL"]

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
