#!/usr/bin/python3
"""Fixed, cgroup-bounded memory loader used only by the MegaBook E2E harness."""

import json
import os
import pathlib
import signal
import stat
import sys
import time

GIB = 1024 * 1024 * 1024
EXPECTED_LIMITS = {
    "memory.high": str(4 * GIB),
    "memory.max": str(8 * GIB),
    "memory.swap.max": str(2 * GIB),
    "memory.oom.group": "1",
}
TARGETS = {"high": 5 * GIB, "max": 9 * GIB}
GO_PAYLOADS = {
    mode: f"schema=pinvou-memory-e2e-go-v1\nmode={mode}\n".encode("ascii")
    for mode in TARGETS
}


def fail(message: str) -> "None":
    raise SystemExit(f"pinvou-memory-e2e-loader: {message}")


def fixed_app_cgroup() -> tuple[str, pathlib.Path]:
    unified = None
    for line in pathlib.Path("/proc/self/cgroup").read_text(encoding="utf-8").splitlines():
        fields = line.split(":", 2)
        if len(fields) == 3 and fields[0] == "0" and fields[1] == "":
            unified = fields[2]
            break
    if not unified or not unified.startswith("/") or not unified.endswith("/pinvou3-app.service"):
        fail("not running inside the fixed pinvou3-app.service cgroup")
    relative = pathlib.PurePosixPath(unified.removeprefix("/"))
    if any(part in {"", ".", ".."} for part in relative.parts):
        fail("app cgroup path is not normalized")
    cgroup = pathlib.Path("/sys/fs/cgroup").joinpath(*relative.parts)
    if not cgroup.is_dir() or cgroup.is_symlink():
        fail("fixed app cgroup directory is unavailable")
    for name, expected in EXPECTED_LIMITS.items():
        if cgroup.joinpath(name).read_text(encoding="ascii").strip() != expected:
            fail(f"fixed app cgroup {name} does not match the MegaBook profile")
    return unified, cgroup


def runtime_directory() -> pathlib.Path:
    runtime = pathlib.Path(f"/run/user/{os.geteuid()}")
    metadata = runtime.stat(follow_symlinks=False)
    if (
        not runtime.is_dir()
        or runtime.is_symlink()
        or metadata.st_uid != os.geteuid()
        or stat.S_IMODE(metadata.st_mode) & 0o022
    ):
        fail("fixed user runtime directory is not trusted")
    directory = runtime / "pinvou-megabook-e2e"
    directory_metadata = directory.stat(follow_symlinks=False)
    if (
        not directory.is_dir()
        or directory.is_symlink()
        or directory_metadata.st_uid != os.geteuid()
        or stat.S_IMODE(directory_metadata.st_mode) != 0o700
    ):
        fail("fixed E2E runtime directory is not trusted")
    return directory


def claim_once(directory: pathlib.Path, mode: str) -> bool:
    marker = directory / f"once-{mode}.marker"
    expected = f"schema=pinvou-memory-e2e-once-v1\nmode={mode}\n".encode("ascii")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW
    try:
        descriptor = os.open(marker, flags, 0o600)
    except FileExistsError:
        metadata = marker.stat(follow_symlinks=False)
        if (
            not marker.is_file()
            or marker.is_symlink()
            or metadata.st_uid != os.geteuid()
            or stat.S_IMODE(metadata.st_mode) != 0o600
            or marker.read_bytes() != expected
        ):
            fail("one-shot marker exists but is not trusted")
        return False
    try:
        os.write(descriptor, expected)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    directory_descriptor = os.open(directory, os.O_RDONLY | os.O_CLOEXEC)
    try:
        os.fsync(directory_descriptor)
    finally:
        os.close(directory_descriptor)
    return True


def write_evidence(path: pathlib.Path, mode: str, cgroup: str) -> None:
    temporary = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW
    descriptor = os.open(temporary, flags, 0o600)
    try:
        payload = json.dumps(
            {"schema": 1, "mode": mode, "pid": os.getpid(), "cgroup": cgroup},
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8") + b"\n"
        os.write(descriptor, payload)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    try:
        os.link(temporary, path, follow_symlinks=False)
    except FileExistsError:
        temporary.unlink(missing_ok=True)
        fail("loader evidence target already exists")
    temporary.unlink()
    directory_descriptor = os.open(path.parent, os.O_RDONLY | os.O_CLOEXEC)
    try:
        os.fsync(directory_descriptor)
    finally:
        os.close(directory_descriptor)


def wait_for_go(directory: pathlib.Path, mode: str) -> None:
    marker = directory / f"go-{mode}.marker"
    expected = GO_PAYLOADS[mode]
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        try:
            metadata = marker.stat(follow_symlinks=False)
        except FileNotFoundError:
            time.sleep(0.1)
            continue
        if (
            not marker.is_file()
            or marker.is_symlink()
            or metadata.st_uid != os.geteuid()
            or stat.S_IMODE(metadata.st_mode) != 0o600
            or marker.read_bytes() != expected
        ):
            fail("go marker exists but is not trusted")
        return
    fail("timed out waiting for the fixed go marker")


def allocate(mode: str) -> None:
    cgroup_text, _ = fixed_app_cgroup()
    directory = runtime_directory()
    write_evidence(directory / f"loader-{mode}.json", mode, cgroup_text)
    # The harness releases this fixed gate only after systemd/cgroup policy, the real WebKit child,
    # Supervisor isolation, and a trusted Resource ledger baseline have all been verified.
    wait_for_go(directory, mode)
    fixed_app_cgroup()
    blocks: list[bytearray] = []
    allocated = 0
    block_size = 64 * 1024 * 1024
    while allocated < TARGETS[mode]:
        block = bytearray(block_size)
        for offset in range(0, len(block), 4096):
            block[offset] = 1
        blocks.append(block)
        allocated += len(block)
    time.sleep(180)


def main() -> None:
    if len(sys.argv) != 2 or sys.argv[1] not in TARGETS:
        fail("usage: memory-loader.py <high|max>")
    mode = sys.argv[1]
    fixed_app_cgroup()
    directory = runtime_directory()
    if not claim_once(directory, mode):
        return
    child = os.fork()
    if child != 0:
        return
    os.setsid()
    signal.signal(signal.SIGHUP, signal.SIG_DFL)
    allocate(mode)


if __name__ == "__main__":
    main()
