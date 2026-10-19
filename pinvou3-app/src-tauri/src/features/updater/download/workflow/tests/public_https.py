"""Explicit opt-in public HTTPS smoke of the unmodified production executable.

No test CA, trust installation, fake fetch, external Python dependency or secret
is used. Harness waits are not product download deadlines. Owned files remain
for inspection; this script neither deletes nor repairs a previous job.
"""
import argparse
import hashlib
import json
from pathlib import Path
import socket
import ssl
import subprocess
import tempfile
import time
import urllib.request

SOURCE = "https://raw.githubusercontent.com/rust-lang/rust/1.89.0/LICENSE-MIT"
HOST = "raw.githubusercontent.com"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-public-https", action="store_true", required=True)
    parser.add_argument("--exe", type=Path, required=True)
    args = parser.parse_args()
    if not args.allow_public_https:
        raise RuntimeError("explicit network consent required")
    executable = args.exe.resolve(strict=True)
    binary_sha = hashlib.sha256(executable.read_bytes()).hexdigest()
    crate = Path(__file__).resolve().parents[1]
    reference = crate.parents[6] / "specs/upgrade/v1/reference/signatures.mjs"
    started = time.monotonic()
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({}), NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl.create_default_context()),
    )
    with opener.open(SOURCE, timeout=30) as response:
        assert response.status == 200
        payload = response.read(65_537)
    assert 0 < len(payload) <= 65_536
    text = payload.decode("utf-8")
    addresses = sorted({row[4][0] for row in socket.getaddrinfo(HOST, 443, type=socket.SOCK_STREAM)})
    assert addresses
    root = Path(tempfile.mkdtemp(prefix="pinvou-job-public-"))

    def child(command):
        # subprocess.run kills and Waits its owned child on timeout. Captured
        # input/diagnostics are never copied to this harness's error messages.
        result = subprocess.run(command, capture_output=True, timeout=120)
        return result

    fixture = child(["node", str(crate / "tests/fixtures.mjs"), str(reference), text, SOURCE])
    assert fixture.returncode == 0, "signed fixture generation failed"
    value = json.loads(fixture.stdout)
    for name in ("root", "package", "expected"):
        (root / (name + ".json")).write_text(value[name], encoding="utf-8")
    (root / "import.json").write_text(json.dumps(value["imported"]), encoding="utf-8")
    (root / "network.json").write_text(json.dumps({"host": HOST, "port": 443, "addresses": addresses}), encoding="utf-8")
    job = root / "job"
    result = child([str(executable), "create", str(job), str(root / "root.json"), value["rootSha256"],
                    str(root / "package.json"), str(root / "expected.json"), str(root / "import.json")])
    assert result.returncode == 0, "production create failed"
    pin = json.loads(result.stdout)["manifestSha256"]

    def invoke(action, *extra):
        return child([str(executable), action, str(job), pin, *map(str, extra)])

    result = invoke("run", root / "import.json", root / "network.json")
    assert result.returncode == 0, "production HTTPS transfer failed"
    lines = [json.loads(line) for line in result.stdout.splitlines()]
    assert lines[0]["kind"] == "started"
    facts = lines[-1]
    assert facts["recordedPhase"] == "bytesMatch" and facts["observedBytesMatch"] is True
    assert facts["actualLength"] == len(payload) and facts["attempt"] == 1
    for flag in ("nativeAuthenticated", "currentQualificationChecked", "verifiedExecutionObject"):
        assert facts[flag] is False
    stored = job / "attempt-00000000000000000001.bin"
    assert stored.read_bytes() == payload
    assert json.loads(invoke("check").stdout)["observedBytesMatch"] is True
    # An invalid operator network document would fail a network action. Reusing
    # complete bytes instead re-scans locally and leaves the attempt unchanged.
    (root / "network.json").write_text('{"host":"","port":0,"addresses":[]}', encoding="utf-8")
    result = invoke("run", root / "import.json", root / "network.json")
    assert result.returncode == 0
    assert json.loads(result.stdout)["attempt"] == 1
    damaged = bytearray(payload)
    damaged[0] ^= 1
    stored.write_bytes(damaged)
    result = invoke("check")
    assert result.returncode != 0 and result.stderr.strip() == b"DOWNLOAD_JOB_BYTES_MISMATCH"
    assert stored.read_bytes() == damaged
    stored.write_bytes(payload)  # Fixture-only restoration, never tool repair.
    journal = job / "journal.jsonl"
    journal.write_bytes(b"null\n")
    result = invoke("check-retained", "1")
    assert result.returncode == 0 and json.loads(result.stdout)["recordedPhase"] == "journalNotConsulted"
    assert journal.read_bytes() == b"null\n"
    assert hashlib.sha256(executable.read_bytes()).hexdigest() == binary_sha
    print(json.dumps({"productionBinarySha256": binary_sha, "source": SOURCE,
                      "bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest(),
                      "transferAndRecheck": "PASS", "pollutionRefusalAndRetainedScan": "PASS",
                      "allChildrenWaited": True, "wallSeconds": round(time.monotonic() - started, 3)}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        # URLs/credentials, private local paths and process arguments stay out
        # of diagnostics. A failed smoke is retained as failure, never skipped.
        print(json.dumps({"publicHttpsSmoke": "FAIL", "errorClass": type(error).__name__}))
        raise SystemExit(1)
