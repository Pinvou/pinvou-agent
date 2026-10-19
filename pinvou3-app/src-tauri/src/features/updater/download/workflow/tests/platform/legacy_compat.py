"""Opt-in actual old/new production executables; no network or OS changes.

Creates a retained job with the independently supplied pre-extraction binary,
then uses the new binary to read it, rescan existing bytes despite a damaged
journal, and recover after exact source restoration. Owned fixtures are retained.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def command(args):
    result = subprocess.run(list(map(str, args)), capture_output=True, timeout=60)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--old-exe", required=True, type=Path)
    parser.add_argument("--old-sha256", required=True)
    parser.add_argument("--new-exe", required=True, type=Path)
    parser.add_argument("--proof", required=True, type=Path)
    args = parser.parse_args()
    if os.name != "nt":
        raise SystemExit("LEGACY_JOB_COMPAT_UNSUPPORTED")
    old_sha=hashlib.sha256(args.old_exe.read_bytes()).hexdigest()
    new_sha=hashlib.sha256(args.new_exe.read_bytes()).hexdigest()
    assert old_sha==args.old_sha256 and old_sha!=new_sha, "independently pinned original executable required"
    crate = Path(__file__).resolve().parents[2]
    repository = crate.parents[6]
    fixture = command(["node", crate / "tests/fixtures.mjs", repository / "specs/upgrade/v1/reference/signatures.mjs", "abc"])
    assert fixture.returncode == 0, "fixed public signing fixture failed"
    data = json.loads(fixture.stdout)
    owned = Path(tempfile.mkdtemp(prefix="pinvou-t33-legacy-job-"))
    for key in ("root", "package", "expected"):
        (owned / (key + ".json")).write_bytes(data[key].encode())
    (owned / "import.json").write_bytes(json.dumps(data["imported"], separators=(",", ":")).encode())
    job = owned / "original-job"
    created = command([args.old_exe, "create", job, owned / "root.json", data["rootSha256"],
                       owned / "package.json", owned / "expected.json", owned / "import.json"])
    assert created.returncode == 0 and not created.stderr, "old executable must create the original job"
    pin = json.loads(created.stdout)["manifestSha256"]
    (job / "attempt-00000000000000000001.bin").write_bytes(b"abc")
    originals = {p.name: p.read_bytes() for p in job.iterdir() if p.is_file()}

    def status(exe, operation, *extra):
        result = command([exe, operation, job, pin, *extra])
        assert result.returncode == 0 and not result.stderr, "actual compatibility operation failed"
        return json.loads(result.stdout)

    old_status = status(args.old_exe, "status")
    new_status = status(args.new_exe, "status")
    assert new_status == old_status
    old_cache = status(args.old_exe, "check-retained", "1")
    new_cache = status(args.new_exe, "check-retained", "1")
    assert new_cache == old_cache and new_cache["observedBytesMatch"] is True
    assert all((job / name).read_bytes() == raw for name, raw in originals.items())
    (job / "journal.jsonl").write_bytes(b"null\n")
    failed = command([args.new_exe, "status", job, pin])
    assert failed.returncode != 0 and not failed.stdout
    assert failed.stderr.strip() == b"DOWNLOAD_JOB_JOURNAL_DAMAGED"
    assert status(args.new_exe, "check-retained", "1") == old_cache
    assert (job / "journal.jsonl").read_bytes() == b"null\n"
    (job / "journal.jsonl").write_bytes(originals["journal.jsonl"])
    assert status(args.new_exe, "status") == old_status
    assert all((job / name).read_bytes() == raw for name, raw in originals.items())
    proof = {
        "oldExecutableSHA256": old_sha,
        "newExecutableSHA256": new_sha,
        "manifestSHA256": pin,
        "originalFiles": {name: hashlib.sha256(raw).hexdigest() for name, raw in originals.items()},
        "oldCreatedJobNewStatusExact": True, "oldCachedBytesNewScanExact": True,
        "damagedJournalRefusedUnhealed": True, "retainedCacheStillObserved": True,
        "originalSourceRestored": True, "allChildrenWaited": True,
    }
    args.proof.write_text(json.dumps(proof, indent=2), encoding="utf-8")
    print("LEGACY_JOB_COMPAT_PASS")


if __name__ == "__main__":
    main()
