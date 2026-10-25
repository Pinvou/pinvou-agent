"""Real fixed-origin control POST/Root GET and rejection tests; no production endpoint I/O."""
import argparse
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading


def read_line(pipe):
    result = queue.Queue()
    threading.Thread(target=lambda: result.put(pipe.readline()), daemon=True).start()
    try:
        line = result.get(timeout=30)
    except queue.Empty:
        raise RuntimeError("isolated peer did not respond") from None
    if not line:
        raise RuntimeError("isolated peer exited")
    return json.loads(line)


def verify(records, cases, address):
    assert records["ProxyHits"] == 0
    assert records["RedirectHits"] == 0
    requests = [item for item in records["Requests"] if item["Method"] == "POST"]
    gets = [item for item in records["Requests"] if item["Method"] == "GET"]
    assert len(requests) == len(cases) + 44, len(requests)  # baseline + original negative/restoration census
    assert len(gets) == 17, len(gets)
    assert len(records["Requests"]) == len(requests) + len(gets)
    first = requests[:len(cases)]
    for observed, expected in zip(first, cases):
        assert observed["Path"] == expected["path"]
        assert observed["Body"] == expected["request"], "canonical POST bytes changed"
    for item in requests + gets:
        assert item["Method"] in ("POST", "GET")
        assert item["Host"] == "update.pinvou.com:" + address.rsplit(":", 1)[1]
        assert item["AcceptEncoding"] == "identity"
        assert item["ContentType"] == ("application/json" if item["Method"] == "POST" else "")
        assert item["CacheControl"] == "no-store"
        assert not item["HasAuthorization"]
        assert not item["HasCookie"]
        assert not item["HasProxyAuthorization"]
        assert "?" not in item["Path"]
    modes = [json.loads(item["Body"]).get("transactionId") for item in requests[len(cases):]]
    assert modes.count("case-restore") == 20
    assert modes.count("case-cancel") == 1
    assert modes.count("case-connection-drop") == 1, "connection loss was automatically resubmitted"
    assert modes.count("case-progress") == 1
    for item in gets:
        assert item["Body"] == ""
    assert [item["Path"] for item in gets[:3]] == [
        "/v1/upgrade/roots/pinvou/1.json", "/v1/upgrade/roots/pinvou/2.json", "/v1/upgrade/roots/pinvou/3.json"]
    assert sum(item["Path"] == "/v1/upgrade/roots/connection-drop/1.json" for item in gets) == 1
    print(f"Control HTTPS wire assertions passed ({len(requests)} POSTs, all {len(cases)} routes; {len(gets)} Root GETs)")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--go", default=shutil.which("go"))
    args = parser.parse_args()
    if not args.go:
        parser.error("Go is required")
    crate = Path(__file__).resolve().parents[1]
    repo = crate.parents[6]
    reference = repo / "specs/upgrade/v1/reference/canonical-json.mjs"
    cases_raw = subprocess.check_output(["node", str(crate / "tests/wire-oracle.mjs"), str(reference)])
    cases = json.loads(cases_raw)
    assert len(cases) == 27
    subprocess.run(["cargo", "test", "--locked", "--no-run"], cwd=crate, check=True)
    hidden = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    with tempfile.TemporaryDirectory(prefix="pinvou-control-https-") as directory:
        temporary = Path(directory).resolve()
        cases_path = temporary / "wire-cases.json"
        cases_path.write_bytes(cases_raw)
        vectors = json.loads((repo / "specs/upgrade/v1/vectors/time/anchor.json").read_text(encoding="utf-8"))
        root_path = temporary / "root-wire-cases.json"
        root_path.write_text(json.dumps({"1": vectors["cases"][0]["trustedRootBytes"],
            "2": vectors["cases"][0]["rootSuccessors"][0]}), encoding="utf-8")
        peer_path = temporary / ("tls-peer.exe" if os.name == "nt" else "tls-peer")
        go_env = os.environ.copy()
        for key in ("GOOS", "GOARCH", "GOFLAGS"):
            go_env.pop(key, None)
        go_env.update(GOENV="off", GOPROXY="off", GOTOOLCHAIN="local", CGO_ENABLED="0")
        subprocess.run([args.go, "build", "-o", str(peer_path), str(crate / "tests/fixtures/tls-server.go")],
                       env=go_env, check=True, creationflags=hidden)
        peer = subprocess.Popen([str(peer_path), str(cases_path), str(temporary / "cancel-seen"), str(root_path)], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, text=True, creationflags=hidden)
        try:
            fixture = read_line(peer.stdout)
            fixture["wireCasesPath"] = str(cases_path)
            fixture_path = temporary / "public-fixture.json"
            fixture_path.write_text(json.dumps(fixture), encoding="utf-8")
            child_env = os.environ.copy()
            child_env["PINVOU_CONTROL_TLS_FIXTURE"] = str(fixture_path)
            proxy = "http://fixture:synthetic@" + fixture["proxyAddress"]
            for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"):
                child_env[name] = child_env[name.lower()] = proxy
            child_env["NO_PROXY"] = child_env["no_proxy"] = ""
            result = subprocess.run(["cargo", "test", "--locked", "--lib", "--", "--include-ignored", "--test-threads=1"],
                                    cwd=crate, env=child_env, creationflags=hidden,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8")
            print(result.stdout, end="", flush=True)
            peer.stdin.write("\n"); peer.stdin.flush()
            records = read_line(peer.stdout)
            peer.wait(timeout=10)
            result.check_returncode()
            verify(records, cases, fixture["address"])
        finally:
            if peer.poll() is None:
                peer.terminate()
                try:
                    peer.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    peer.kill(); peer.wait(timeout=10)
            peer.stdin.close(); peer.stdout.close()


if __name__ == "__main__":
    main()
