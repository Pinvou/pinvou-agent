"""Credential-isolating pass-through for real configured model routes.

Real credentials stay in this trusted host process. The agent namespace receives
only a temporary proxy capability. No model responses are generated or mocked.
"""

import copy
import hmac
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import select
import socket
import socketserver
import subprocess
import threading
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPHandler, HTTPRedirectHandler, HTTPSHandler, Request, build_opener


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def resolve_key(cli, home_template, model_id):
    env = dict(os.environ, PINVOU3_HOME=str(home_template))
    result = subprocess.run([str(cli), "--output", "json", "models", "show", model_id, "--reveal-key"],
                            env=env, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError("configured model credential resolution failed")
    result = json.loads(result.stdout)
    key = result.get("api_key")
    if not key:
        raise RuntimeError("configured model credential is unavailable")
    return key


def scrub(value):
    if isinstance(value, dict):
        return {k: ("" if k in {"api_key", "password", "access_token", "refresh_token", "secret"} or k.endswith("_api_key")
                    else scrub(v)) for k, v in value.items()
                if k not in {"credential_ref", "credential_state", "credential_action", "has_secret"}}
    if isinstance(value, list):
        return [scrub(v) for v in value]
    return value


class ModelProxy:
    def __init__(self, cli, home_template, model_id, socket_path=None):
        config = json.loads((home_template / "settings.json").read_text())
        profile = next(p for p in config["advanced"]["saved_models"] if p["id"] == model_id)
        self._key = resolve_key(cli, home_template, model_id)
        self._capability = secrets.token_urlsafe(32)
        base_url = profile["base_url"].rstrip("/")
        if urlsplit(base_url).scheme not in {"http", "https"}:
            raise ValueError("unsupported configured model transport")
        owner = self
        self.wire_shapes = []

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_args):
                pass

            def do_GET(self):
                self.forward()

            def do_POST(self):
                self.forward()

            def forward(self):
                auth = self.headers.get("Authorization", "")
                if not hmac.compare_digest(auth, "Bearer " + owner._capability):
                    self.send_error(403)
                    return
                path = urlsplit(self.path).path
                if path not in {"/v1/chat/completions", "/v1/models", "/v1/responses"}:
                    self.send_error(404)
                    return
                body = self.rfile.read(int(self.headers.get("Content-Length", "0"))) if self.command == "POST" else None
                if body:
                    try:
                        payload = json.loads(body)
                        public_parameters = {"model", "stream", "temperature", "top_p", "top_k", "max_tokens",
                                             "max_completion_tokens", "reasoning_effort", "thinking", "enable_thinking",
                                             "chat_template_kwargs", "presence_penalty", "frequency_penalty", "seed",
                                             "tool_choice", "parallel_tool_calls"}
                        owner.wire_shapes.append({"path": path, "model": payload.get("model"),
                                                  "parameters": {k: v for k, v in payload.items() if k in public_parameters}})
                    except ValueError:
                        pass
                headers = {k: v for k, v in self.headers.items() if k.lower() not in {
                    "host", "authorization", "connection", "content-length", "transfer-encoding", "accept-encoding"}}
                headers["Authorization"] = "Bearer " + owner._key
                headers["Accept-Encoding"] = "identity"
                request = Request(base_url + path[len("/v1"):], data=body, headers=headers, method=self.command)
                finished = threading.Event()
                connections = []
                responses = []

                def connection_factory(kind):
                    def create(*args, **kwargs):
                        connection = kind(*args, **kwargs)
                        connections.append(connection)
                        return connection
                    return create

                class ForwardHTTP(HTTPHandler):
                    def http_open(self, req):
                        return self.do_open(connection_factory(http.client.HTTPConnection), req)

                class ForwardHTTPS(HTTPSHandler):
                    def https_open(self, req):
                        return self.do_open(connection_factory(http.client.HTTPSConnection), req, context=self._context)

                def cancel_on_disconnect():
                    disconnected = False
                    while not finished.wait(.1):
                        if not disconnected:
                            try:
                                ready, _, _ = select.select([self.connection], [], [], 0)
                                disconnected = bool(ready) and not self.connection.recv(1, socket.MSG_PEEK)
                            except OSError:
                                disconnected = True
                        if disconnected:
                            for response in list(responses):
                                stream = getattr(response, "fp", None)
                                raw = getattr(stream, "raw", None)
                                active_socket = getattr(raw, "_sock", None)
                                if active_socket is not None:
                                    try:
                                        active_socket.shutdown(socket.SHUT_RDWR)
                                    except OSError:
                                        pass
                            for connection in list(connections):
                                if connection.sock is not None:
                                    try:
                                        connection.sock.shutdown(socket.SHUT_RDWR)
                                    except OSError:
                                        pass
                                connection.close()

                threading.Thread(target=cancel_on_disconnect, daemon=True).start()
                try:
                    try:
                        response = build_opener(NoRedirect(), ForwardHTTP(), ForwardHTTPS()).open(request, timeout=240)
                    except HTTPError as error:
                        response = error
                    responses.append(response)
                    with response:
                        self.send_response_only(response.code)
                        for name, value in response.headers.items():
                            if name.lower() not in {"connection", "content-length", "transfer-encoding", "authorization", "proxy-authorization"}:
                                self.send_header(name, value)
                        self.send_header("Transfer-Encoding", "chunked")
                        self.end_headers()
                        reader = getattr(response, "read1", response.read)
                        while chunk := reader(65536):
                            self.wfile.write(f"{len(chunk):x}\r\n".encode() + chunk + b"\r\n")
                            self.wfile.flush()
                        self.wfile.write(b"0\r\n\r\n")
                        self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    return
                except (OSError, URLError):
                    self.close_connection = True
                    try:
                        self.send_error(502, "configured model transport unavailable")
                    except OSError:
                        pass
                finally:
                    finished.set()

        self.socket_path = socket_path
        if socket_path is not None:
            if socket_path.exists():
                if not socket_path.is_socket():
                    raise RuntimeError("proxy socket path is not a socket")
                socket_path.unlink()

            class UnixHTTPServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
                daemon_threads = True

            self.server = UnixHTTPServer(str(socket_path), Handler)
            socket_path.chmod(0o600)
            local_endpoint = "http://127.0.0.1:18000/v1"
        else:
            self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            local_endpoint = f"http://127.0.0.1:{self.server.server_port}/v1"
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        # Clone routing/sampling fields but replace credentials and transport.
        # The original key and endpoint are never persisted into agent-visible state.
        clean = scrub(copy.deepcopy(config))
        selected = scrub(copy.deepcopy(profile))
        selected["base_url"] = local_endpoint
        selected["api_key"] = self._capability
        clean["advanced"]["saved_models"] = [selected]
        clean["advanced"]["active_model_id"] = model_id
        clean["advanced"]["custom_base_url"] = local_endpoint
        self.settings = clean

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        if self.socket_path is not None:
            self.socket_path.unlink(missing_ok=True)
