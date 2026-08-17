#!/usr/bin/env python3
"""内置 MCP 契约矩阵：真实启动 stdio server，检查协议、工具调用与本地产物。"""
from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import tempfile
import threading
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MCP_ROOT = ROOT / "pinvou3-app" / "resources" / "mcp-servers"

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")


class RpcServer:
    def __init__(
        self,
        server_dir: Path,
        env: dict[str, str] | None = None,
        timeout_s: float | None = None,
    ):
        child_env = os.environ.copy()
        child_env.update(env or {})
        self.server_dir = server_dir
        self.timeout_s = timeout_s or float(os.environ.get("PINVOU3_MCP_RPC_TIMEOUT_SECS", "10"))
        self.proc = subprocess.Popen(
            [sys.executable, "server.py"],
            cwd=server_dir,
            env=child_env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        self.seq = 0

    def call(self, method: str, params=None) -> dict:
        self.seq += 1
        request = {"jsonrpc": "2.0", "id": self.seq, "method": method}
        if params is not None:
            request["params"] = params
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(request, ensure_ascii=False) + "\n")
        self.proc.stdin.flush()
        result: queue.Queue[tuple[str | None, BaseException | None]] = queue.Queue(maxsize=1)

        def read_response() -> None:
            try:
                result.put((self.proc.stdout.readline(), None))
            except BaseException as exc:  # surface pipe/decoder failures in the caller
                result.put((None, exc))

        threading.Thread(target=read_response, daemon=True).start()
        try:
            line, read_error = result.get(timeout=self.timeout_s)
        except queue.Empty as exc:
            self._terminate()
            stderr = self._stderr_tail()
            raise AssertionError(
                f"MCP {self.server_dir.name} {method} timed out after "
                f"{self.timeout_s:g}s; stderr tail: {stderr or '(empty)'}"
            ) from exc
        if read_error is not None:
            self._terminate()
            raise AssertionError(
                f"MCP {self.server_dir.name} {method} response read failed: {read_error}"
            ) from read_error
        assert line is not None
        if not line:
            self._terminate()
            stderr = self._stderr_tail()
            raise AssertionError(f"MCP server exited without response: {stderr[-1000:]}")
        response = json.loads(line.lstrip("\ufeff"))
        assert response.get("id") == self.seq, response
        return response

    def _terminate(self) -> None:
        if self.proc.poll() is not None:
            return
        self.proc.terminate()
        try:
            self.proc.wait(timeout=1)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=1)

    def _stderr_tail(self) -> str:
        if not self.proc.stderr:
            return ""
        return self.proc.stderr.read()[-1000:].strip()

    def close(self):
        if self.proc.stdin:
            try:
                self.proc.stdin.close()
            except OSError:
                pass
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self._terminate()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def content_json(response: dict) -> dict:
    assert "error" not in response, response
    content = response["result"]["content"]
    assert content and content[0]["type"] == "text", response
    return json.loads(content[0]["text"])


def check_protocol(tool_id: str, expected_tools: set[str], env=None):
    with RpcServer(MCP_ROOT / tool_id, env) as rpc:
        initialized = rpc.call("initialize")
        assert initialized["result"]["protocolVersion"] == "2024-11-05"
        listed = rpc.call("tools/list")
        actual = {tool["name"] for tool in listed["result"]["tools"]}
        assert actual == expected_tools, (tool_id, actual)
    print(f"✅ {tool_id}: initialize + tools/list ({len(expected_tools)})")


def check_rpc_timeout() -> None:
    with tempfile.TemporaryDirectory(prefix="pinvou-mcp-hung-") as tmp:
        server_dir = Path(tmp)
        (server_dir / "server.py").write_text(
            "import sys, time\n"
            "for _line in sys.stdin:\n"
            "    print('hung server marker', file=sys.stderr, flush=True)\n"
            "    time.sleep(60)\n",
            encoding="utf-8",
        )
        try:
            with RpcServer(server_dir, timeout_s=0.2) as rpc:
                rpc.call("initialize")
        except AssertionError as exc:
            message = str(exc)
            assert "timed out" in message and "hung server marker" in message, message
        else:
            raise AssertionError("hung MCP server should time out")
    print("✅ stdio RPC: 卡住时按单次超时终止，并带 stderr 尾部诊断")


def main():
    check_rpc_timeout()
    manifests = {}
    for manifest_path in sorted(MCP_ROOT.glob("*/manifest.json")):
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        assert manifest["id"] == manifest_path.parent.name
        manifests[manifest["id"]] = manifest
    assert set(manifests) == {
        "weather",
        "iwencai",
        "qcc",
        "obsidian",
        "pptx",
        "gongwen",
        "yuandian-mcp",
        "canva-mcp",
        "patsnap-search",
        "wecom-bot",
        "tencent-docs",
    }
    print("✅ manifest: 11 个可安装 MCP 清单完整且目录 ID 一致")

    expected = {
        "weather": {"get_weather"},
        "iwencai": {
            "hithink_market_query", "hithink_finance_query", "hithink_event_query",
            "hithink_management_query", "hithink_business_query", "hithink_industry_query",
            "hithink_zhishu_query", "hithink_sector_selector", "hithink_astock_selector",
            "hithink_macro_query", "news_search", "announcement_search",
        },
        "obsidian": {"search", "read_note", "list", "create_note", "edit_note", "rename_note", "delete_note"},
        "pptx": {"make_pptx"},
        "gongwen": {"make_gongwen"},
        "wecom-bot": {"send_text", "send_markdown", "send_news", "send_image", "send_file"},
    }
    for tool_id, names in expected.items():
        check_protocol(tool_id, names)

    with RpcServer(MCP_ROOT / "weather", {"AMAP_KEY": ""}) as rpc:
        result = content_json(rpc.call("tools/call", {"name": "get_weather", "arguments": {"city": "杭州"}}))
        assert result == {"error": "AMAP_KEY 未配置"}
    print("✅ weather: 未配置凭据返回稳定错误，不发起外网请求")

    with RpcServer(MCP_ROOT / "wecom-bot", {"WECOM_BOT_KEY": ""}) as rpc:
        result = content_json(rpc.call("tools/call", {"name": "send_text", "arguments": {"content": "测试"}}))
        assert result == {"error": "WECOM_BOT_KEY 未配置，请在工具详情里填写群机器人 webhook key"}
    print("✅ wecom-bot: 未配置凭据返回稳定错误，不发起外网请求")

    with RpcServer(MCP_ROOT / "iwencai", {"IWENCAI_API_KEY": ""}) as rpc:
        response = rpc.call("tools/call", {"name": "news_search", "arguments": {"query": "黄金价格"}})
        assert response["error"]["code"] == -32000
        assert "IWENCAI_API_KEY" in response["error"]["message"]
    print("✅ iwencai: 真实 tools/call 缺凭据错误契约")

    with tempfile.TemporaryDirectory(prefix="pinvou-obsidian-") as vault:
        env = {"OBSIDIAN_VAULT_PATH": vault}
        with RpcServer(MCP_ROOT / "obsidian", env) as rpc:
            made = content_json(rpc.call("tools/call", {"name": "create_note", "arguments": {"path": "测试/自动化", "content": "# 自动化\n工具商店契约"}}))
            assert made.get("type") == "obsidian_created" and Path(vault, "测试", "自动化.md").is_file()
            read = content_json(rpc.call("tools/call", {"name": "read_note", "arguments": {"path": "测试/自动化"}}))
            assert "工具商店契约" in read.get("content", "")
            found = content_json(rpc.call("tools/call", {"name": "search", "arguments": {"query": "工具商店"}}))
            assert found.get("count") == 1
            preview = content_json(rpc.call("tools/call", {"name": "delete_note", "arguments": {"path": "测试/自动化"}}))
            assert preview.get("type") == "confirm_required" and Path(vault, "测试", "自动化.md").exists()
            deleted = content_json(rpc.call("tools/call", {"name": "delete_note", "arguments": {"path": "测试/自动化", "confirm": True}}))
            assert deleted.get("type") == "obsidian_deleted" and not Path(vault, "测试", "自动化.md").exists()
    print("✅ obsidian: 创建/读取/搜索/人在环删除全旅程")

    with tempfile.TemporaryDirectory(prefix="pinvou-artifacts-") as artifacts:
        env = {"PINVOU3_SESSION_ARTIFACTS": artifacts}
        with RpcServer(MCP_ROOT / "pptx", env) as rpc:
            result = content_json(rpc.call("tools/call", {"name": "make_pptx", "arguments": {
                "filename": "自动化测试", "theme": "business-blue", "slides": [
                    {"layout": "cover", "title": "自动化测试", "subtitle": "工具商店"},
                    {"layout": "bullets", "title": "结论", "bullets": ["安装可用", "产物可编辑"]},
                ]
            }}))
            pptx_path = Path(result["path"])
            assert result.get("ok") is True and result.get("slides") == 2
            assert pptx_path.parent == Path(artifacts) and zipfile.is_zipfile(pptx_path)

        with RpcServer(MCP_ROOT / "gongwen", env) as rpc:
            result = content_json(rpc.call("tools/call", {"name": "make_gongwen", "arguments": {
                "filename": "自动化测试通知", "doc_type": "通知", "issuer": "测试中心",
                "title": "测试中心关于开展自动化测试的通知", "recipient": "各部门",
                "body": "为提升产品质量，现将有关事项通知如下。\n一、开展工具安装测试。\n二、验证产物可以正常打开。",
                "signer": "测试中心", "date": "2026年7月14日",
            }}))
            docx_path = Path(result["path"])
            assert result.get("ok") is True and result["validate"]["ok"] is True
            assert docx_path.parent == Path(artifacts) and zipfile.is_zipfile(docx_path)
    print("✅ pptx/gongwen: 真实 tools/call 生成可打开的 OOXML 产物")

    weather = manifests["weather"]
    assert weather["secret_env"] == [{"key": "AMAP_KEY", "provider": "amap", "required": True}]
    assert weather["config_fields"] == [{
        "key": "AMAP_KEY",
        "label": "高德 Web 服务 API Key",
        "required": True,
        "target": "env",
        "secret": True,
    }]
    print("✅ weather: 用户自填高德 Web 服务 Key 清单契约")

    iwencai = manifests["iwencai"]
    assert iwencai["secret_env"] == [{"key": "IWENCAI_API_KEY", "provider": "iwencai", "required": True}]
    assert iwencai["config_fields"] == [{
        "key": "IWENCAI_API_KEY",
        "label": "问财 API Key",
        "required": True,
        "target": "env",
        "secret": True,
    }]
    print("✅ iwencai: 用户自填问财 API Key 清单契约")

    wecom_bot = manifests["wecom-bot"]
    assert wecom_bot["secret_env"] == [{"key": "WECOM_BOT_KEY", "provider": "wecom-bot", "required": True}]
    assert wecom_bot["config_fields"] == [{
        "key": "WECOM_BOT_KEY",
        "label": "群机器人 Webhook Key",
        "required": True,
        "target": "env",
        "secret": True,
    }]
    print("✅ wecom-bot: 用户自填群机器人 Webhook Key 清单契约")

    qcc = manifests["qcc"]
    assert qcc.get("secret_headers") in (None, [])
    assert qcc["config_fields"] == []
    assert qcc["servers"] == [{
        "name": "qcc-company",
        "url": "https://agent.qcc.com/mcp/company/stream",
        "scopes": ["mcp:tools"],
        "oauth_resource": "https://agent.qcc.com/mcp/company/stream",
    }]
    print("✅ qcc: 唯一 qcc-company 远程端点 + OAuth scope/resource 清单契约")

    yuandian = manifests["yuandian-mcp"]
    assert yuandian["mcp_tools"] == [] and not yuandian["command"]
    assert yuandian["servers"] == [{
        "name": "yuandian_mcp",
        "url": "https://open.chineselaw.com/mcp",
        "scopes": ["mcp"],
        "oauth_resource": "https://open.chineselaw.com/mcp",
    }]
    print("✅ yuandian-mcp: 唯一远程端点 + OAuth scope/resource 清单契约")

    canva = manifests["canva-mcp"]
    assert canva["mcp_tools"] == [] and not canva["command"]
    assert canva["servers"] == [{
        "name": "canva_mcp",
        "url": "https://mcp.canva.cn/mcp",
        "scopes": [
            "profile:read",
            "design:meta:read",
            "design:content:write",
            "design:content:read",
            "folder:read",
            "folder:write",
            "brandtemplate:content:read",
            "brandtemplate:meta:read",
            "brandtemplate:content:write",
            "comment:write",
            "comment:read",
            "asset:read",
            "asset:write",
            "brandkit:read",
            "help:answers:read",
            "help:answers:write",
        ],
        "oauth_resource": "https://mcp.canva.cn/mcp",
    }]
    assert "validate_on_install" not in canva
    assert "secret_headers" not in canva
    assert "config_fields" not in canva
    print("✅ canva-mcp: 唯一远程端点 + OAuth scope/resource 清单契约")

    patsnap = manifests["patsnap-search"]
    assert patsnap["mcp_tools"] == ["patsnap_search", "patsnap_fetch"] and not patsnap["command"]
    assert patsnap["servers"] == [{
        "name": "patsnap-search",
        "url": "https://connect.zhihuiya.com/2b0355/logic-mcp",
    }]
    assert patsnap["validate_on_install"] is True
    assert patsnap["secret_headers"] == [{
        "header": "Authorization",
        "scheme": "Bearer",
        "source_key": "PATSNAP_API_KEY",
        "provider": "patsnap",
        "required": True,
    }]
    assert patsnap["config_fields"] == [{
        "key": "PATSNAP_API_KEY",
        "label": "智慧芽 API Key",
        "required": True,
        "target": "bearer",
        "secret": True,
    }]
    print("✅ patsnap-search: 唯一远程端点 + bearer secret + 安装校验契约")

    tdoc = manifests["tencent-docs"]
    assert tdoc["mcp_tools"] == [] and not tdoc["command"]
    assert tdoc["servers"] == [
        {"name": "tencent-docs", "url": "https://docs.qq.com/openapi/mcp"},
        {"name": "tdoc-slide", "url": "https://docs.qq.com/api/v6/slide/mcp"},
        {"name": "tdoc-doc", "url": "https://docs.qq.com/api/v6/doc/mcp"},
        {"name": "tdoc-sheet", "url": "https://docs.qq.com/api/v6/sheet/mcp"},
    ]
    assert tdoc["validate_on_install"] is True
    assert tdoc["secret_headers"] == [{
        "header": "Authorization",
        "scheme": "",
        "source_key": "TENCENT_DOCS_TOKEN",
        "provider": "tencent-docs",
        "required": True,
    }]
    assert tdoc["config_fields"] == [{
        "key": "TENCENT_DOCS_TOKEN",
        "label": "腾讯文档 Token",
        "required": True,
        "target": "bearer",
        "secret": True,
    }]
    print("✅ tencent-docs: 四官方远程端点 + 无 scheme 原始 Token 注入 + 安装校验契约")

    print("\n✅ ALL MCP SERVER CONTRACTS PASS")


if __name__ == "__main__":
    main()
