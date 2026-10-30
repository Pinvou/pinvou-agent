"""Opt-in OS isolation acceptance plus provider-free proxy contract checks."""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http.client
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import select
import threading
import unittest
from unittest.mock import patch
from urllib.request import Request, urlopen


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).resolve().parents[1] / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


sandbox = load("eval_namespace", "prompt_eval_sandbox.py")
proxy_module = load("eval_proxy", "prompt_eval_model_proxy.py")


class NamespaceAcceptanceTests(unittest.TestCase):
    def test_platform_dynamic_loader_view_is_read_only_when_present(self):
        original_exists = Path.exists
        for present in [False, True]:
            def exists(path):
                return present if str(path) == "/lib64" else original_exists(path)
            with patch.object(Path, "exists", exists), patch.object(sandbox.shutil, "which", return_value="/usr/bin/bwrap"):
                args = sandbox.base_args()
            binds = [args[index:index + 3] for index, value in enumerate(args) if value == "--ro-bind"]
            self.assertEqual(["--ro-bind", "/lib64", "/lib64"] in binds, present)

    def test_declared_roots_include_spaces_unicode_and_nested_selections(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("primary", "secondary", "third root", "四号 项目", "third root/nested", "unselected"):
                (root / name).mkdir(parents=True, exist_ok=True)
            request = {"workspace": "/work/trial/primary", "workspace_roots": [
                f"/work/trial/{name}" for name in ("secondary", "third root", "四号 项目", "third root/nested", "secondary")
            ]}
            mounts = sandbox.workspace_mounts(request, root)
            self.assertEqual({visible for _, visible in mounts}, {
                "/work/trial/primary", "/work/trial/secondary", "/work/trial/third root", "/work/trial/四号 项目"})
            self.assertNotIn(root / "unselected", [path for path, _ in mounts])

    def test_workspace_mounts_reject_controller_escape_runtime_and_missing_roots(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "primary").mkdir()
            (root / "home").mkdir()
            (root / "link").symlink_to(root / "primary", target_is_directory=True)
            for value in ("/work/trial", "/work/trial/../secret", "/etc", "/work/trial/home", "/work/trial/missing", "/work/trial/link", 3):
                with self.subTest(value=value), self.assertRaises(ValueError):
                    sandbox.workspace_mounts({"workspace_roots": [value]}, root)
            with self.assertRaises(ValueError):
                sandbox.workspace_mounts({"workspace_roots": "/work/trial/primary"}, root)

    @unittest.skipUnless(os.environ.get("PINVOU_EVAL_NS_SELFTEST") == "1", "opt-in Linux namespace acceptance")
    def test_supplemental_alias_oracle_rejects_json_only_copy(self):
        import sys
        with patch.dict(sys.modules, {"prompt_eval_sandbox": sandbox}):
            extra = load("extra_alias", "prompt_eval_extra_alias.py")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config_merge.py"
            path.write_text("def clone(v):\n if isinstance(v,dict):return {k:clone(x) for k,x in v.items()}\n if isinstance(v,list):return [clone(x) for x in v]\n return v\ndef merge_config(a,b):\n r=clone(a)\n for k,v in b.items():\n  r[k]=merge_config(r[k],v) if k in r and isinstance(r[k],dict) and isinstance(v,dict) else clone(v)\n return r\n")
            self.assertFalse(extra.check(path))
            path.write_text("import copy\ndef merge_config(a,b):\n r=copy.deepcopy(a)\n for k,v in b.items():\n  r[k]=merge_config(r[k],v) if k in r and isinstance(r[k],dict) and isinstance(v,dict) else copy.deepcopy(v)\n return r\n")
            self.assertTrue(extra.check(path))

    @unittest.skipUnless(os.environ.get("PINVOU_EVAL_NS_SELFTEST") == "1", "opt-in Linux namespace acceptance")
    def test_real_namespace_hides_controller_neighbors_and_user_state(self):
        sandbox.self_test()

    @unittest.skipUnless(os.environ.get("PINVOU_EVAL_NS_SELFTEST") == "1", "opt-in Linux namespace acceptance")
    def test_generated_code_cannot_write_its_project_during_verification(self):
        import sys
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory)
            result = sandbox.VerifierSubprocess.run([sys.executable, "-c",
                "from pathlib import Path; Path('forbidden.txt').write_text('bad')"],
                cwd=project, capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((project / "forbidden.txt").exists())


class ProxyContractTests(unittest.TestCase):
    def test_client_cancellation_closes_streaming_upstream(self):
        cancelled = threading.Event()

        class Upstream(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(b"data: hello\n\n")
                self.wfile.flush()
                readable, _, _ = select.select([self.connection], [], [], 3)
                if readable and self.connection.recv(1) == b"":
                    cancelled.set()

        upstream = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        threading.Thread(target=upstream.serve_forever, daemon=True).start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                home = Path(directory)
                (home / "settings.json").write_text(json.dumps({"advanced": {"saved_models": [
                    {"id": "fixture", "model": "fixture-model", "base_url": f"http://127.0.0.1:{upstream.server_port}/v1"}]}}))
                with patch.object(proxy_module, "resolve_key", return_value="unit-only-credential"):
                    proxy = proxy_module.ModelProxy(Path("unused"), home, "fixture")
                try:
                    connection = http.client.HTTPConnection("127.0.0.1", proxy.server.server_port, timeout=4)
                    connection.request("POST", "/v1/chat/completions", b"{}", {
                        "Authorization": "Bearer " + proxy.settings["advanced"]["saved_models"][0]["api_key"]})
                    response = connection.getresponse()
                    self.assertEqual(response.read(1), b"d")
                    response.close()
                    connection.close()
                    self.assertTrue(cancelled.wait(2), "upstream generation must not survive a cancelled trial")
                finally:
                    proxy.close()
        finally:
            upstream.shutdown()
            upstream.server_close()

    def test_forwarding_preserves_body_and_keeps_real_auth_out_of_agent_settings(self):
        observed = {}

        class Upstream(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                observed["auth"] = self.headers.get("Authorization")
                observed["headers"] = {k.lower(): v for k, v in self.headers.items()}
                observed["body"] = self.rfile.read(int(self.headers["Content-Length"]))
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"result":"from-upstream"}')

        upstream = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        threading.Thread(target=upstream.serve_forever, daemon=True).start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                home = Path(directory)
                config = {"advanced": {"saved_models": [{"id": "fixture", "model": "fixture-model",
                           "base_url": f"http://127.0.0.1:{upstream.server_port}/v1",
                           "vendor": "glm", "credential_ref": {"service": "fixture", "account": "fixture"}}]}}
                (home / "settings.json").write_text(json.dumps(config))
                with patch.object(proxy_module, "resolve_key", return_value="synthetic-unit-credential"):
                    proxy = proxy_module.ModelProxy(Path("unused"), home, "fixture")
                try:
                    selected = proxy.settings["advanced"]["saved_models"][0]
                    self.assertNotIn("credential_ref", selected)
                    self.assertNotIn("synthetic-unit-credential", json.dumps(proxy.settings))
                    self.assertEqual(selected["vendor"], "glm")
                    body = b'{"model":"fixture-model","messages":[{"role":"user","content":"hello"}],"stream":false}'
                    request = Request(selected["base_url"] + "/chat/completions", data=body,
                                      headers={"Authorization": "Bearer " + selected["api_key"], "Content-Type": "application/json",
                                               "Cookie": "synthetic-cookie", "X-Api-Key": "synthetic-other-key",
                                               "Proxy-Authorization": "synthetic-proxy-key", "Anthropic-Version": "2023-06-01"})
                    with urlopen(request, timeout=5) as response:
                        self.assertEqual(response.read(), b'{"result":"from-upstream"}')
                    self.assertEqual(observed["body"], body)
                    self.assertEqual(observed["auth"], "Bearer synthetic-unit-credential")
                    for secret_header in ["cookie", "x-api-key", "proxy-authorization"]:
                        self.assertNotIn(secret_header, observed["headers"])
                    self.assertEqual(observed["headers"]["anthropic-version"], "2023-06-01")
                    self.assertNotIn("messages", proxy.wire_shapes[0]["parameters"])
                finally:
                    proxy.close()
        finally:
            upstream.shutdown()
            upstream.server_close()


if __name__ == "__main__":
    unittest.main()
