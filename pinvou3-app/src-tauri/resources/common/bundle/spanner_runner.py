#!/usr/bin/env python3
"""
pinvou3 spanner_runner — 通用 MCP 适配器：把 spanner 插件包包装成「单工具 MCP server」。

用法：`<python> spanner_runner.py <plugin.json 路径>`
协议：newline-delimited JSON-RPC 2.0 over stdio（与 weather server.py 同款）。

spanner 包作者只写「读 stdin JSON → 写 stdout JSON」的入口脚本，本适配器负责：
1. 读 plugin.json 的 spanner 声明（name/schema/entry/runtime）；
2. 暴露一个 MCP 工具（name = 包 id，schema = input_schema）；
3. tools/call 时 spawn 入口脚本，把参数 JSON 写 stdin，读 stdout JSON 回传。

语言不限：入口脚本用什么语言/运行时，由 spanner.runtime 声明决定；缺省用内置 python。
"""
import io
import json
import os
import subprocess
import sys

# Windows 默认 stdout/stdin 编码为 GBK，MCP 协议要求 UTF-8
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")

DEFAULT_TIMEOUT_SECS = 30


def _send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _result(req_id, result):
    _send({"jsonrpc": "2.0", "id": req_id, "result": result})


def _error(req_id, code, message):
    _send({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}})


def _resolve_runtime(spanner, pkg_dir):
    """spanner.runtime 声明 → 解释器命令列表。缺省用内置 python（env 或系统 python）。"""
    runtime = (spanner or {}).get("runtime") or {}
    rt_dir = runtime.get("dir")
    kind = (runtime.get("kind") or "python").lower()
    if rt_dir:
        rt_path = os.path.join(pkg_dir, rt_dir)
        if os.path.isdir(rt_path):
            if kind in ("python", "python3"):
                for cand in ("bin/python", "bin/python3", "python.exe", "python3.exe", "python"):
                    p = os.path.join(rt_path, cand)
                    if os.path.isfile(p):
                        return [p]
            elif kind in ("node", "nodejs"):
                for cand in ("bin/node", "node.exe", "node"):
                    p = os.path.join(rt_path, cand)
                    if os.path.isfile(p):
                        return [p]
    # 缺省：内置 python（PINVOU3_PYTHON）→ 系统 python
    env_py = os.environ.get("PINVOU3_PYTHON")
    if env_py:
        return [env_py]
    return ["python" if sys.platform == "win32" else "python3"]


def _load_manifest(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def main():
    manifest_path = sys.argv[1] if len(sys.argv) > 1 else None
    if not manifest_path or not os.path.isfile(manifest_path):
        # 无清单 → 启动即报错（不阻塞 client）
        return

    try:
        manifest = _load_manifest(manifest_path)
    except Exception as e:
        sys.stderr.write("spanner_runner: 读 plugin.json 失败: %s\n" % e)
        return

    pkg_dir = os.path.dirname(os.path.abspath(manifest_path))
    spanner = manifest.get("spanner") or {}
    entry = (spanner.get("entry") or "").lstrip("/").replace("\\", "/").removeprefix("spanner/")
    runtime_cmd = _resolve_runtime(spanner, pkg_dir)
    tool_name = manifest.get("id", "spanner")
    tool_desc = manifest.get("description") or manifest.get("name") or tool_name
    input_schema = spanner.get("input_schema") or {"type": "object", "properties": {}}
    timeout = int(spanner.get("timeout_secs") or DEFAULT_TIMEOUT_SECS)

    tool_def = {
        "name": tool_name,
        "description": tool_desc,
        "inputSchema": input_schema,
    }

    entry_path = os.path.join(pkg_dir, "spanner", entry)
    entry_cwd = os.path.join(pkg_dir, "spanner")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        method = msg.get("method")
        req_id = msg.get("id")
        if req_id is None:
            continue
        try:
            if method == "initialize":
                _result(req_id, {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "pinvou3-spanner-%s" % tool_name, "version": "1.0.0"},
                })
            elif method == "tools/list":
                _result(req_id, {"tools": [tool_def]})
            elif method == "tools/call":
                params = msg.get("params") or {}
                if params.get("name") != tool_name:
                    _error(req_id, -32601, "unknown tool: %s" % params.get("name"))
                    continue
                args = params.get("arguments") or {}
                if not os.path.isfile(entry_path):
                    _error(req_id, -32603, "spanner 入口不存在: %s" % entry)
                    continue
                try:
                    proc = subprocess.run(
                        runtime_cmd + [entry_path],
                        input=json.dumps(args, ensure_ascii=False),
                        capture_output=True,
                        text=True,
                        encoding="utf-8",
                        timeout=timeout,
                        cwd=entry_cwd,
                    )
                except subprocess.TimeoutExpired:
                    _error(req_id, -32603, "spanner 执行超时(%ss)" % timeout)
                    continue
                except Exception as e:
                    _error(req_id, -32603, "spanner 启动失败: %s" % e)
                    continue
                if proc.returncode != 0:
                    _error(req_id, -32603, "spanner 退出码 %s: %s" % (proc.returncode, proc.stderr.strip()))
                    continue
                out = proc.stdout.strip()
                if not out:
                    out = "{}"
                _result(req_id, {"content": [{"type": "text", "text": out}]})
            else:
                _error(req_id, -32601, "method not found: %s" % method)
        except Exception as e:
            _error(req_id, -32603, "internal error: %s" % e)


if __name__ == "__main__":
    main()
