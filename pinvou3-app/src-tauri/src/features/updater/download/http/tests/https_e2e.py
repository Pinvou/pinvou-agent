"""Run the real HTTPS transport matrix with an isolated in-memory TLS peer."""
import argparse
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading


def read_line(pipe, timeout):
    result = queue.Queue()
    def read():
        result.put(pipe.readline())
    threading.Thread(target=read, daemon=True).start()
    try:
        line = result.get(timeout=timeout)
    except queue.Empty:
        raise RuntimeError("isolated peer did not respond") from None
    if not line:
        raise RuntimeError("isolated peer exited unexpectedly")
    return json.loads(line)


def verify(records):
    assert records["proxyHits"] == 0, "system proxy was used"
    assert records["redirectHits"] == 0, "redirect was followed"
    requests = records["requests"]
    assert requests, "no HTTPS requests observed"
    assert sum(item["path"] == "/cache-header-wait" for item in requests) == 1, "header cancellation never reached the peer"
    for request in requests:
        assert request["path"] not in ("/unapproved-pin", "/denied")
        assert request["method"] == "GET"
        assert request["acceptEncoding"] == "identity"
        assert not request["hasAuthorization"]
        assert not request["hasCookie"]
        assert not request["hasProxyAuthorization"]
    partials = [item for item in requests if item["path"] == "/partial"]
    assert {item["range"] for item in partials} == {"bytes=4-", "bytes=7-"}
    assert all(item["ifRange"] == '"original"' for item in partials)
    assert any(item["path"] == "/cookie" for item in requests)
    print(f"HTTPS wire assertions passed ({len(requests)} requests)")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--go", default=shutil.which("go"))
    args = parser.parse_args()
    if not args.go:
        parser.error("Go is required for the isolated HTTPS fixture")
    crate = Path(__file__).resolve().parents[1]
    subprocess.run(["cargo", "test", "--locked", "--no-run"], cwd=crate, check=True)
    hidden = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    # TemporaryDirectory owns this exact directory; no unrelated process or
    # filesystem path is selected for termination or cleanup.
    with tempfile.TemporaryDirectory(prefix="pinvou-file-https-") as directory:
        temporary = Path(directory).resolve()
        peer_path = temporary / ("tls-peer.exe" if os.name == "nt" else "tls-peer")
        go_env = os.environ.copy()
        for key in ("GOOS", "GOARCH", "GOFLAGS"):
            go_env.pop(key, None)
        go_env.update(GOENV="off", GOPROXY="off", GOTOOLCHAIN="local", CGO_ENABLED="0")
        subprocess.run([args.go, "build", "-o", str(peer_path),
                        str(crate / "tests/fixtures/tls-server.go")],
                       env=go_env, check=True, creationflags=hidden)
        peer = subprocess.Popen([str(peer_path)], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, text=True,
                                creationflags=hidden)
        try:
            fixture = read_line(peer.stdout, 30)
            public_info = temporary / "public-fixture.json"
            public_info.write_text(json.dumps(fixture), encoding="utf-8")
            child_env = os.environ.copy()
            child_env["PINVOU_UPGRADE_TLS_FIXTURE"] = str(public_info)
            proxy = "http://fixture:synthetic@" + fixture["proxyAddress"]
            for key in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"):
                child_env[key] = child_env[key.lower()] = proxy
            child_env["NO_PROXY"] = child_env["no_proxy"] = ""
            outcome = subprocess.run(
                ["cargo", "test", "--locked", "--lib", "--", "--include-ignored",
                 "--test-threads=1"], cwd=crate, env=child_env,
                creationflags=hidden, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, text=True, encoding="utf-8")
            print(outcome.stdout, end="", flush=True)
            peer.stdin.write("\n")
            peer.stdin.flush()
            records = read_line(peer.stdout, 30)
            peer.wait(timeout=10)
            outcome.check_returncode()
            verify(records)
        finally:
            if peer.poll() is None:
                peer.terminate()
                try:
                    peer.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    peer.kill()
                    peer.wait(timeout=10)
            peer.stdin.close()
            peer.stdout.close()


if __name__ == "__main__":
    main()
