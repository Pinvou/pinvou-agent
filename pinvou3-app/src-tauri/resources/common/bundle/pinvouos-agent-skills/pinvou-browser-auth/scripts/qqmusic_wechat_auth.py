#!/usr/bin/env python3
"""Bounded QQ Music WeChat authorization broker for PinvouOS Linux.

Public commands are ``start``, ``status`` and ``cancel``.  ``start`` detaches a
worker which owns a dedicated headed Firefox instance and its WebDriver BiDi
session.  The worker never serializes QR URLs or cookie values. Authorization
requires either three facts from the current flow (QQ callback, changed auth
cookie, and authenticated UI) or a still-authenticated dedicated profile with
a prior three-fact verification marker.
"""

from __future__ import annotations

import argparse
import base64
import contextlib
import hashlib
import json
import os
import re
import secrets
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from collections.abc import Callable, Iterator
from typing import Any

try:
    import fcntl
except ImportError:  # pragma: no cover - exercised by the fail-closed platform gate.
    fcntl = None  # type: ignore[assignment]


os.umask(0o077)

RUNTIME_DIR_NAME = "pinvou-browser-auth"
STATE_VERSION = 1
DEFAULT_TTL_SECONDS = 300
MAX_TTL_SECONDS = 300
START_READY_TIMEOUT_SECONDS = 90
START_LAUNCH_GRACE_SECONDS = 15
POLL_SECONDS = 0.25
PROCESS_CLEANUP_GUARD_SECONDS = 10
PROCESS_ACTIVE_LIFETIME_SECONDS = MAX_TTL_SECONDS - PROCESS_CLEANUP_GUARD_SECONDS
MAX_WS_MESSAGE_BYTES = 2 * 1024 * 1024
JOB_RE = re.compile(r"^[0-9a-f]{24}$")
UNIT_RE = re.compile(r"^pinvou-browser-auth-[0-9a-f]{24}\.service$")
PUBLIC_STATUSES = {"waiting", "authorized", "expired", "failed", "cancelled"}
INTERNAL_TERMINAL_STATUSES = {"authorized", "expired", "failed", "cancelled"}
EVIDENCE_KEYS = (
    "qr_ready",
    "scanned",
    "callback_seen",
    "cookie_signal",
    "user_dom",
    "prior_verified",
)
AUTH_COOKIE_NAMES = {
    "qqmusic_key",
    "qm_keyst",
    "uin",
    "wxuin",
    "qqmusic_uin",
    "psrf_qqopenid",
    "psrf_qqunionid",
    "psrf_qqaccess_token",
    "psrf_musickey_createtime",
}
AUTH_NAME_PATTERN = re.compile(
    r"(?:^|_)(?:uin|key|skey|token|openid|access)(?:_|$)", re.I
)
TRACKING_COOKIE_PREFIXES = ("pgv_", "ts_", "fqm_", "yybsdk")

QQ_MUSIC_URL = "https://y.qq.com/"
ISOLATED_PYTHON = "/usr/bin/python3"
AUTH_MARKER_NAME = ".pinvou-browser-auth-verified.json"
AUTH_MARKER_MAX_AGE_SECONDS = 30 * 24 * 60 * 60
PROXY_ENV_KEYS = (
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
)

LOGIN_RECT_JS = r"""
(() => {
  const visible = (el) => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width >= 2 && r.height >= 2 && s.display !== 'none' &&
      s.visibility !== 'hidden' && Number(s.opacity || '1') > 0;
  };
  const el = [...document.querySelectorAll('.top_login__link,a,button')]
    .find((node) => visible(node) && (node.textContent || '').trim() === '登录');
  if (!el) return JSON.stringify({found:false});
  const r = el.getBoundingClientRect();
  return JSON.stringify({found:true,x:Math.round(r.left+r.width/2),y:Math.round(r.top+r.height/2)});
})()
"""

WECHAT_RECT_JS = r"""
(() => {
  const visible = (el) => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width >= 2 && r.height >= 2 && s.display !== 'none' &&
      s.visibility !== 'hidden' && Number(s.opacity || '1') > 0;
  };
  const el = [...document.querySelectorAll('a.login-box-tit__item,a,button')]
    .find((node) => visible(node) && (node.textContent || '').trim() === '微信登录');
  if (!el) return JSON.stringify({found:false});
  const r = el.getBoundingClientRect();
  return JSON.stringify({found:true,x:Math.round(r.left+r.width/2),y:Math.round(r.top+r.height/2)});
})()
"""

WECHAT_CURRENT_JS = r"""
(() => {
  const nodes = [...document.querySelectorAll('a.login-box-tit__item')];
  const el = nodes.find((node) => (node.textContent || '').trim() === '微信登录');
  if (!el) return false;
  const own = String(el.className || '').toLowerCase();
  const body = [...document.querySelectorAll('.login-box-bd__item')]
    .some((node) => /(?:^|\s)current(?:\s|$)/i.test(String(node.className || '')) &&
      /微信/.test(node.textContent || ''));
  return /(?:^|\s)current(?:\s|$)/.test(own) || body;
})()
"""

QR_STATE_JS = r"""
(() => {
  const text = (document.body && document.body.innerText) || '';
  let qr = null;
  for (const image of document.images) {
    try {
      const u = new URL(image.src);
      if (u.protocol === 'https:' && u.hostname === 'open.weixin.qq.com' &&
          (u.port === '' || u.port === '443') &&
          /^\/connect\/qrcode\/[A-Za-z0-9_-]+$/.test(u.pathname)) {
        qr = image;
        break;
      }
    } catch (_) {}
  }
  if (/二维码.*(?:失效|过期)|请刷新二维码/.test(text)) {
    return JSON.stringify({state:'expired'});
  }
  if (/扫描成功|已扫描|请在(?:手机|微信).*确认/.test(text)) {
    return JSON.stringify({state:'scanned'});
  }
  if (qr) {
    const r = qr.getBoundingClientRect();
    const s = getComputedStyle(qr);
    const visible = r.width >= 2 && r.height >= 2 && r.right > 0 && r.bottom > 0 &&
      r.left < innerWidth && r.top < innerHeight && s.display !== 'none' &&
      s.visibility !== 'hidden' && Number(s.opacity || '1') > 0;
    if (visible && qr.complete && qr.naturalWidth >= 200 && qr.naturalHeight >= 200) {
      return JSON.stringify({state:'ready',width:qr.naturalWidth,height:qr.naturalHeight});
    }
  }
  return JSON.stringify({state:'loading'});
})()
"""

USER_DOM_JS = r"""
(() => {
  const visible = (el) => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width >= 2 && r.height >= 2 && s.display !== 'none' &&
      s.visibility !== 'hidden' && Number(s.opacity || '1') > 0;
  };
  const root = document.querySelector('.top_login');
  if (!root) return false;
  const loginVisible = [...root.querySelectorAll('.top_login__link,a,button')]
    .some((node) => visible(node) && (node.textContent || '').trim() === '登录');
  const avatarVisible = [...root.querySelectorAll('.top_login__cover,img,[class*=avatar]')]
    .some((node) => visible(node));
  const text = (root.innerText || '').trim();
  const namedUser = !!text && text !== '登录' && !/^登录\s*$/.test(text);
  return !loginVisible && (avatarVisible || namedUser);
})()
"""


class BrokerError(RuntimeError):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


class ProcessDeadlineError(Exception):
    pass


def validate_job_id(job_id: str) -> str:
    if not JOB_RE.fullmatch(job_id or ""):
        raise BrokerError("invalid_job_id")
    return job_id


def secure_dir(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise BrokerError("unsafe_runtime_path")
    if info.st_uid != os.geteuid():
        raise BrokerError("runtime_owner_mismatch")
    os.chmod(path, 0o700)
    return path


def runtime_root() -> Path:
    if sys.platform != "linux":
        raise BrokerError("linux_required")
    raw = os.environ.get("XDG_RUNTIME_DIR", "")
    if not raw or not os.path.isabs(raw):
        raise BrokerError("xdg_runtime_unavailable")
    parent = Path(raw)
    if not parent.is_dir() or parent.lstat().st_uid != os.geteuid():
        raise BrokerError("xdg_runtime_unavailable")
    return secure_dir(parent / RUNTIME_DIR_NAME)


def state_path(root: Path, job_id: str) -> Path:
    return root / f"{validate_job_id(job_id)}.json"


def log_path(root: Path, job_id: str) -> Path:
    return root / f"{validate_job_id(job_id)}.log"


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    secure_dir(path.parent)
    fd, tmp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            fd = -1
            json.dump(
                value, handle, ensure_ascii=False, sort_keys=True, separators=(",", ":")
            )
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp_name, path)
        os.chmod(path, 0o600)
    finally:
        if fd >= 0:
            os.close(fd)
        try:
            os.unlink(tmp_name)
        except FileNotFoundError:
            pass


def read_json_file(path: Path) -> dict[str, Any]:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags)
    try:
        info = os.fstat(fd)
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_uid != os.geteuid()
            or info.st_size > 65536
        ):
            raise BrokerError("unsafe_state_file")
        with os.fdopen(fd, "r", encoding="utf-8") as handle:
            fd = -1
            value = json.load(handle)
    finally:
        if fd >= 0:
            os.close(fd)
    if not isinstance(value, dict) or value.get("version") != STATE_VERSION:
        raise BrokerError("invalid_state")
    return value


def load_state(root: Path, job_id: str) -> dict[str, Any]:
    try:
        return read_json_file(state_path(root, job_id))
    except FileNotFoundError as exc:
        raise BrokerError("job_not_found") from exc
    except BrokerError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError, TypeError, ValueError) as exc:
        raise BrokerError("invalid_state") from exc


def state_lock_path(root: Path, job_id: str) -> Path:
    return root / f".{validate_job_id(job_id)}.lock"


@contextlib.contextmanager
def exclusive_file_lock(path: Path) -> Iterator[None]:
    if fcntl is None:
        raise BrokerError("linux_required")
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid():
            raise BrokerError("unsafe_lock_file")
        os.fchmod(fd, 0o600)
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)


def save_state_unlocked(root: Path, state: dict[str, Any]) -> None:
    state["updated_at"] = int(time.time())
    atomic_json(state_path(root, str(state["job_id"])), state)


def create_state(root: Path, state: dict[str, Any]) -> None:
    job_id = str(state["job_id"])
    with exclusive_file_lock(state_lock_path(root, job_id)):
        if state_path(root, job_id).exists():
            raise BrokerError("job_exists")
        save_state_unlocked(root, state)


def mutate_state(
    root: Path,
    job_id: str,
    mutation: Callable[[dict[str, Any]], None],
) -> dict[str, Any]:
    with exclusive_file_lock(state_lock_path(root, job_id)):
        state = load_state(root, job_id)
        mutation(state)
        save_state_unlocked(root, state)
        return state


def record_evidence(root: Path, job_id: str, **facts: bool) -> dict[str, Any]:
    if any(key not in EVIDENCE_KEYS for key in facts):
        raise BrokerError("invalid_evidence")

    def apply(state: dict[str, Any]) -> None:
        evidence = state.get("evidence")
        if not isinstance(evidence, dict):
            raise BrokerError("invalid_state")
        if state.get("status") != "waiting":
            return
        for key, value in facts.items():
            if value:
                evidence[key] = True

    return mutate_state(root, job_id, apply)


def safe_log(root: Path, job_id: str, event: str) -> None:
    # Only fixed event names are accepted. Browser URLs, DOM text, cookie names and
    # exception strings never enter this log.
    if not re.fullmatch(r"[a-z0-9_]{1,48}", event):
        event = "invalid_event"
    path = log_path(root, job_id)
    flags = os.O_WRONLY | os.O_APPEND | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags, 0o600)
        try:
            os.fchmod(fd, 0o600)
            line = (
                json.dumps(
                    {"at": int(time.time()), "event": event}, separators=(",", ":")
                )
                + "\n"
            )
            os.write(fd, line.encode("utf-8"))
        finally:
            os.close(fd)
    except OSError:
        # State is authoritative; an optional fixed-name audit event must not
        # undo a verified transition or strand an owned browser.
        return


def public_result(state: dict[str, Any]) -> dict[str, Any]:
    status_value = state.get("status")
    status_text = status_value if status_value in PUBLIC_STATUSES else "failed"
    raw = state.get("evidence") if isinstance(state.get("evidence"), dict) else {}
    return {
        "job_id": str(state.get("job_id", "")),
        "status": status_text,
        "evidence": {key: bool(raw.get(key, False)) for key in EVIDENCE_KEYS},
    }


def new_state(
    job_id: str, ttl_seconds: int, launch_kind: str, unit: str | None
) -> dict[str, Any]:
    now = int(time.time())
    return {
        "version": STATE_VERSION,
        "job_id": job_id,
        "status": "waiting",
        "reason": "",
        "created_at": now,
        "updated_at": now,
        "deadline_at": now + min(ttl_seconds, PROCESS_ACTIVE_LIFETIME_SECONDS),
        "process_deadline_at": now + PROCESS_ACTIVE_LIFETIME_SECONDS,
        "launch_kind": launch_kind,
        "unit": unit,
        "cancel_requested": False,
        "worker_pid": None,
        "worker_pgid": None,
        "worker_start_ticks": None,
        "browser_pid": None,
        "browser_pgid": None,
        "browser_start_ticks": None,
        "port": None,
        "evidence": {key: False for key in EVIDENCE_KEYS},
    }


def effective_active_deadline(state: dict[str, Any]) -> float:
    return min(
        float(state.get("deadline_at", 0)),
        float(state.get("process_deadline_at", 0)),
    )


def bounded_monotonic_deadline(state: dict[str, Any], max_wait: float) -> float:
    remaining = max(0.0, effective_active_deadline(state) - time.time())
    return min(time.monotonic() + max_wait, time.monotonic() + remaining)


def process_start_ticks(pid: int) -> int | None:
    try:
        raw = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
        tail = raw[raw.rfind(")") + 2 :].split()
        return int(tail[19])
    except (OSError, ValueError, IndexError):
        return None


def process_identity_matches(pid: Any, start_ticks: Any) -> bool:
    return (
        isinstance(pid, int)
        and isinstance(start_ticks, int)
        and process_start_ticks(pid) == start_ticks
    )


def terminate_owned_group(
    pid: Any, pgid: Any, start_ticks: Any, grace: float = 5.0
) -> None:
    if not process_identity_matches(pid, start_ticks) or not isinstance(pgid, int):
        return
    try:
        if os.getpgid(pid) != pgid:
            return
        os.killpg(pgid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError, OSError):
        return
    deadline = time.monotonic() + grace
    while time.monotonic() < deadline and process_identity_matches(pid, start_ticks):
        time.sleep(0.05)
    if process_identity_matches(pid, start_ticks):
        try:
            os.killpg(pgid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError, OSError):
            pass


def active_path(root: Path) -> Path:
    return root / "active.json"


def active_job(root: Path) -> str | None:
    try:
        value = read_json_file(active_path(root))
    except (FileNotFoundError, BrokerError, json.JSONDecodeError):
        return None
    job_id = value.get("job_id")
    return job_id if isinstance(job_id, str) and JOB_RE.fullmatch(job_id) else None


def active_state_is_reusable(state: dict[str, Any]) -> bool:
    if bool(state.get("cancel_requested")):
        return False
    now = int(time.time())
    if state.get("status") == "waiting":
        if effective_active_deadline(state) <= now:
            return False
        worker_live = process_identity_matches(
            state.get("worker_pid"), state.get("worker_start_ticks")
        )
        qr_ready = bool(state.get("evidence", {}).get("qr_ready"))
        if not worker_live:
            return (
                not qr_ready
                and state.get("worker_pid") is None
                and now - int(state.get("created_at", 0)) <= START_LAUNCH_GRACE_SECONDS
            )
        if not qr_ready:
            return True
        return browser_state_is_live(state)
    if state.get("status") == "authorized":
        return (
            int(state.get("process_deadline_at", 0)) > now
            and process_identity_matches(
                state.get("worker_pid"), state.get("worker_start_ticks")
            )
            and browser_state_is_live(state)
        )
    return False


def browser_state_is_live(state: dict[str, Any]) -> bool:
    pid = state.get("browser_pid")
    pgid = state.get("browser_pgid")
    port = state.get("port")
    if not (
        process_identity_matches(pid, state.get("browser_start_ticks"))
        and isinstance(pid, int)
        and isinstance(pgid, int)
        and isinstance(port, int)
        and 1 <= port <= 65535
    ):
        return False
    try:
        return os.getpgid(pid) == pgid and listener_owned_by_group(port, pgid)
    except (ProcessLookupError, PermissionError, OSError):
        return False


def claim_active(root: Path, state: dict[str, Any]) -> str | None:
    job_id = validate_job_id(str(state["job_id"]))
    stale_state_to_clean: dict[str, Any] | None = None
    with exclusive_file_lock(root / ".active.lock"):
        existing = active_job(root)
        if existing:
            try:
                existing_state = load_state(root, existing)
                if active_state_is_reusable(existing_state):
                    return existing
                if existing_state.get("status") == "waiting":

                    def invalidate(state_to_invalidate: dict[str, Any]) -> None:
                        if state_to_invalidate.get("status") == "waiting":
                            state_to_invalidate["status"] = "failed"
                            state_to_invalidate["reason"] = "stale_active_job"
                            state_to_invalidate["cancel_requested"] = True

                    existing_state = mutate_state(root, existing, invalidate)
                stale_state_to_clean = existing_state
            except BrokerError:
                pass
        create_state(root, state)
        atomic_json(
            active_path(root),
            {"version": STATE_VERSION, "job_id": job_id},
        )
    if stale_state_to_clean is not None:
        terminate_state_processes(stale_state_to_clean)
    return None


def release_active(root: Path, job_id: str) -> None:
    with exclusive_file_lock(root / ".active.lock"):
        if active_job(root) != job_id:
            return
        try:
            active_path(root).unlink()
        except FileNotFoundError:
            pass


def firefox_profile_dir() -> Path:
    home = Path.home()
    snap_common = home / "snap" / "firefox" / "common"
    if snap_common.is_dir():
        return secure_dir(snap_common / "pinvou-browser-auth-profile")
    data_home = Path(os.environ.get("XDG_DATA_HOME", home / ".local" / "share"))
    return secure_dir(data_home / RUNTIME_DIR_NAME / "firefox-profile")


def auth_marker_valid(profile: Path) -> bool:
    try:
        marker = read_json_file(profile / AUTH_MARKER_NAME)
        verified_at = int(marker.get("verified_at", 0))
    except (
        FileNotFoundError,
        BrokerError,
        json.JSONDecodeError,
        TypeError,
        ValueError,
    ):
        return False
    now = int(time.time())
    return now - AUTH_MARKER_MAX_AGE_SECONDS <= verified_at <= now + 60


def write_auth_marker(profile: Path) -> None:
    # This carries no token, cookie, URL, account identifier, or QR material. It
    # records only that this dedicated profile previously passed all three facts.
    atomic_json(
        profile / AUTH_MARKER_NAME,
        {"version": STATE_VERSION, "verified_at": int(time.time())},
    )


def write_auth_marker_best_effort(profile: Path, root: Path, job_id: str) -> None:
    try:
        write_auth_marker(profile)
    except (OSError, BrokerError):
        safe_log(root, job_id, "marker_write_failed")


def pick_port() -> int:
    for port in (9444, 9333):
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            sock.bind(("127.0.0.1", port))
            return port
        except OSError:
            pass
        finally:
            sock.close()
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])
    finally:
        sock.close()


def loopback_listener_inodes(port: int) -> set[str]:
    inodes: set[str] = set()
    try:
        lines = Path("/proc/net/tcp").read_text(encoding="ascii").splitlines()[1:]
    except OSError:
        return inodes
    expected_port = f"{port:04X}"
    for line in lines:
        fields = line.split()
        if len(fields) < 10:
            continue
        address, separator, port_hex = fields[1].partition(":")
        if (
            separator
            and address == "0100007F"
            and port_hex.upper() == expected_port
            and fields[3] == "0A"
        ):
            inodes.add(fields[9])
    return inodes


def listener_owned_by_group(port: int, pgid: int) -> bool:
    listener_inodes = loopback_listener_inodes(port)
    if not listener_inodes:
        return False
    for proc_dir in Path("/proc").iterdir():
        if not proc_dir.name.isdigit():
            continue
        try:
            info = proc_dir.stat()
            pid = int(proc_dir.name)
            if info.st_uid != os.geteuid() or os.getpgid(pid) != pgid:
                continue
            for fd_path in (proc_dir / "fd").iterdir():
                try:
                    target = os.readlink(fd_path)
                except OSError:
                    continue
                if target.startswith("socket:[") and target[8:-1] in listener_inodes:
                    return True
        except (
            FileNotFoundError,
            PermissionError,
            ProcessLookupError,
            OSError,
            ValueError,
        ):
            continue
    return False


def wait_debugger(
    port: int,
    browser_pid: int,
    browser_pgid: int,
    browser_start_ticks: int,
    deadline: float,
) -> None:
    endpoint = f"http://127.0.0.1:{port}/json/list"
    direct_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    while time.monotonic() < deadline:
        if not process_identity_matches(browser_pid, browser_start_ticks):
            raise BrokerError("browser_closed")
        if not listener_owned_by_group(port, browser_pgid):
            time.sleep(0.1)
            continue
        try:
            with direct_opener.open(endpoint, timeout=1.0) as response:
                if response.status == 200 and listener_owned_by_group(
                    port, browser_pgid
                ):
                    return
        except urllib.error.HTTPError as exc:
            # Firefox 133+ removed CDP; a 404 proves Remote Agent is ready for BiDi.
            if exc.code == 404 and listener_owned_by_group(port, browser_pgid):
                return
        except (urllib.error.URLError, TimeoutError, OSError):
            pass
        time.sleep(0.1)
    raise BrokerError("browser_debugger_timeout")


class StdlibWebSocket:
    """Small RFC 6455 client for Firefox's loopback-only BiDi endpoint."""

    GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

    def __init__(self, port: int):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=5.0)
        self.buffer = bytearray()
        self.closed = False
        try:
            key = base64.b64encode(os.urandom(16)).decode("ascii")
            request = (
                "GET /session HTTP/1.1\r\n"
                f"Host: 127.0.0.1:{port}\r\n"
                "Upgrade: websocket\r\n"
                "Connection: Upgrade\r\n"
                f"Sec-WebSocket-Key: {key}\r\n"
                "Sec-WebSocket-Version: 13\r\n\r\n"
            )
            self.sock.sendall(request.encode("ascii"))
            response = self._read_http_headers(time.monotonic() + 5.0)
            self.validate_upgrade_response(response, key)
        except Exception:
            self.sock.close()
            raise

    @classmethod
    def validate_upgrade_response(cls, response: bytes, key: str) -> None:
        status_line, *header_lines = response.decode("iso-8859-1").split("\r\n")

        def forbidden_ctl(text: str) -> bool:
            return any(
                (ord(character) < 0x20 and character != "\t") or ord(character) == 0x7F
                for character in text
            )

        if forbidden_ctl(status_line) or not re.fullmatch(
            r"HTTP/1\.1 101(?: .*)?", status_line
        ):
            raise BrokerError("websocket_upgrade_failed")
        headers: dict[str, str] = {}
        for line in header_lines:
            if not line:
                continue
            if forbidden_ctl(line):
                raise BrokerError("websocket_upgrade_failed")
            name, separator, value = line.partition(":")
            lowered = name.strip().lower()
            if (
                not separator
                or name != name.strip()
                or not re.fullmatch(r"[!#$%&'*+.^_`|~0-9A-Za-z-]+", name)
                or lowered in headers
            ):
                raise BrokerError("websocket_upgrade_failed")
            headers[lowered] = value.strip()
        expected = base64.b64encode(
            hashlib.sha1((key + cls.GUID).encode("ascii")).digest()
        ).decode("ascii")
        if (
            headers.get("upgrade", "").lower() != "websocket"
            or "upgrade"
            not in {
                item.strip().lower()
                for item in headers.get("connection", "").split(",")
            }
            or not secrets.compare_digest(
                headers.get("sec-websocket-accept", ""), expected
            )
            or "sec-websocket-extensions" in headers
            or "sec-websocket-protocol" in headers
        ):
            raise BrokerError("websocket_upgrade_failed")

    def _read_http_headers(self, deadline: float) -> bytes:
        marker = b"\r\n\r\n"
        while marker not in self.buffer:
            if len(self.buffer) >= 16384:
                raise BrokerError("websocket_upgrade_failed")
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise BrokerError("bidi_timeout")
            self.sock.settimeout(remaining)
            try:
                chunk = self.sock.recv(16384 - len(self.buffer))
            except socket.timeout as exc:
                raise BrokerError("bidi_timeout") from exc
            if not chunk:
                raise BrokerError("bidi_closed")
            self.buffer.extend(chunk)
        split_at = self.buffer.index(marker) + len(marker)
        if split_at > 16384:
            raise BrokerError("websocket_upgrade_failed")
        result = bytes(self.buffer[:split_at])
        del self.buffer[:split_at]
        return result

    def _recv_into_buffer(self, deadline: float) -> None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise BrokerError("bidi_timeout")
        self.sock.settimeout(remaining)
        try:
            chunk = self.sock.recv(65536)
        except socket.timeout as exc:
            raise BrokerError("bidi_timeout") from exc
        if not chunk:
            raise BrokerError("bidi_closed")
        self.buffer.extend(chunk)

    def _read_exact(self, length: int, deadline: float) -> bytes:
        while len(self.buffer) < length:
            self._recv_into_buffer(deadline)
        result = bytes(self.buffer[:length])
        del self.buffer[:length]
        return result

    def _send_frame(self, opcode: int, payload: bytes, deadline: float) -> None:
        if self.closed:
            raise BrokerError("bidi_closed")
        if len(payload) > MAX_WS_MESSAGE_BYTES:
            raise BrokerError("websocket_message_too_large")
        first = 0x80 | opcode
        length = len(payload)
        if length < 126:
            header = bytes((first, 0x80 | length))
        elif length <= 0xFFFF:
            header = bytes((first, 0x80 | 126)) + struct.pack("!H", length)
        else:
            header = bytes((first, 0x80 | 127)) + struct.pack("!Q", length)
        mask = os.urandom(4)
        masked = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
        try:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise BrokerError("bidi_timeout")
            self.sock.settimeout(remaining)
            self.sock.sendall(header + mask + masked)
        except socket.timeout as exc:
            raise BrokerError("bidi_timeout") from exc
        except (BrokenPipeError, ConnectionError, OSError) as exc:
            raise BrokerError("bidi_closed") from exc

    def send_json(self, value: dict[str, Any], deadline: float) -> None:
        self._send_frame(
            0x1,
            json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode(
                "utf-8"
            ),
            deadline,
        )

    @staticmethod
    def _validate_close_payload(payload: bytes) -> None:
        if len(payload) == 1:
            raise BrokerError("websocket_protocol_error")
        if len(payload) < 2:
            return
        code = struct.unpack("!H", payload[:2])[0]
        if not (
            code in {1000, 1001, 1002, 1003, 1007, 1008, 1009, 1010, 1011}
            or 3000 <= code <= 4999
        ):
            raise BrokerError("websocket_protocol_error")
        try:
            payload[2:].decode("utf-8")
        except UnicodeDecodeError as exc:
            raise BrokerError("websocket_protocol_error") from exc

    def recv_json(self, deadline: float) -> dict[str, Any]:
        fragment_opcode: int | None = None
        message_bytes = bytearray()
        fragment_bytes = 0
        frame_count = 0
        while True:
            first, second = self._read_exact(2, deadline)
            fin = bool(first & 0x80)
            if first & 0x70:
                raise BrokerError("websocket_protocol_error")
            opcode = first & 0x0F
            masked = bool(second & 0x80)
            if masked:
                raise BrokerError("websocket_protocol_error")
            if opcode not in {0x0, 0x1, 0x2, 0x8, 0x9, 0xA}:
                raise BrokerError("websocket_protocol_error")
            if opcode == 0x0 and fragment_opcode is None:
                raise BrokerError("websocket_protocol_error")
            if opcode in {0x1, 0x2} and fragment_opcode is not None:
                raise BrokerError("websocket_protocol_error")
            length_code = second & 0x7F
            length = length_code
            if length_code == 126:
                length = struct.unpack("!H", self._read_exact(2, deadline))[0]
                if length < 126:
                    raise BrokerError("websocket_protocol_error")
            elif length_code == 127:
                length = struct.unpack("!Q", self._read_exact(8, deadline))[0]
                if length <= 0xFFFF or length & (1 << 63):
                    raise BrokerError("websocket_protocol_error")
            if length > MAX_WS_MESSAGE_BYTES:
                raise BrokerError("websocket_message_too_large")
            if opcode in {0x8, 0x9, 0xA} and (not fin or length_code >= 126):
                raise BrokerError("websocket_protocol_error")
            if (
                opcode == 0x0
                and fragment_opcode is not None
                and fragment_bytes + length > MAX_WS_MESSAGE_BYTES
            ):
                raise BrokerError("websocket_message_too_large")
            payload = self._read_exact(length, deadline)

            if opcode in (0x8, 0x9, 0xA):
                if opcode == 0x8:
                    self._validate_close_payload(payload)
                    self._send_frame(0x8, payload, deadline)
                    self.closed = True
                    try:
                        self.sock.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    self.sock.close()
                    raise BrokerError("bidi_closed")
                if opcode == 0x9:
                    self._send_frame(0xA, payload, deadline)
                continue

            if opcode in (0x1, 0x2):
                fragment_opcode = opcode
                message_bytes.extend(payload)
                fragment_bytes = len(payload)
            elif opcode == 0x0:
                message_bytes.extend(payload)
                fragment_bytes += len(payload)
            frame_count += 1
            if frame_count > 4096:
                raise BrokerError("websocket_protocol_error")
            if fragment_bytes > MAX_WS_MESSAGE_BYTES:
                raise BrokerError("websocket_message_too_large")
            if not fin:
                continue
            if fragment_opcode != 0x1:
                raise BrokerError("websocket_protocol_error")
            try:
                message = json.loads(message_bytes.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                raise BrokerError("websocket_protocol_error") from exc
            if not isinstance(message, dict):
                raise BrokerError("websocket_protocol_error")
            return message

    def close(self) -> None:
        if self.closed:
            return
        try:
            self._send_frame(0x8, struct.pack("!H", 1000), time.monotonic() + 1.0)
        except BrokerError:
            pass
        self.closed = True
        try:
            self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.sock.close()


class BidiClient:
    def __init__(self, port: int):
        self.ws = StdlibWebSocket(port)
        self.next_id = 0
        self.events: list[dict[str, Any]] = []

    def request(
        self, method: str, params: dict[str, Any], timeout: float = 5.0
    ) -> dict[str, Any]:
        self.next_id += 1
        request_id = self.next_id
        deadline = time.monotonic() + timeout
        self.ws.send_json(
            {"id": request_id, "method": method, "params": params}, deadline
        )
        while time.monotonic() < deadline:
            message = self.ws.recv_json(deadline)
            if message.get("id") == request_id:
                if message.get("type") == "error" or message.get("error"):
                    raise BrokerError("bidi_command_failed")
                return message
            if isinstance(message.get("method"), str):
                self.events.append(message)
                if len(self.events) > 1024:
                    del self.events[: len(self.events) - 1024]
        raise BrokerError("bidi_timeout")

    def take_events(self) -> list[dict[str, Any]]:
        events, self.events = self.events, []
        return events

    def close(self) -> None:
        try:
            self.ws.close()
        except Exception:
            pass


def bidi_value(reply: dict[str, Any]) -> Any:
    return reply.get("result", {}).get("result", {}).get("value")


def eval_json(client: BidiClient, context: str, expression: str) -> Any:
    reply = client.request(
        "script.evaluate",
        {
            "expression": expression,
            "target": {"context": context},
            "awaitPromise": False,
            "resultOwnership": "root",
        },
    )
    value = bidi_value(reply)
    return json.loads(value) if isinstance(value, str) else value


def flatten_contexts(reply: dict[str, Any]) -> list[tuple[str, str]]:
    result: list[tuple[str, str]] = []

    def walk(node: dict[str, Any]) -> None:
        context = node.get("context")
        if isinstance(context, str):
            result.append((context, str(node.get("url", ""))))
        for child in node.get("children") or []:
            if isinstance(child, dict):
                walk(child)

    for root in reply.get("result", {}).get("contexts", []):
        if isinstance(root, dict):
            walk(root)
    return result


def is_callback_url(raw_url: str) -> bool:
    try:
        parsed = urllib.parse.urlsplit(raw_url)
        port = parsed.port
    except ValueError:
        return False
    return (
        parsed.scheme == "https"
        and parsed.hostname == "y.qq.com"
        and port in (None, 443)
        and parsed.username is None
        and parsed.password is None
        and parsed.path == "/portal/wx_redirect.html"
    )


def events_show_callback(events: list[dict[str, Any]]) -> bool:
    for event in events:
        params = event.get("params") if isinstance(event.get("params"), dict) else {}
        urls = [params.get("url")]
        context_info = params.get("context")
        if isinstance(context_info, dict):
            urls.append(context_info.get("url"))
        if any(isinstance(url, str) and is_callback_url(url) for url in urls):
            return True
    return False


def pointer_click(
    client: BidiClient, context: str, x: int, y: int, source_id: str
) -> None:
    client.request(
        "input.performActions",
        {
            "context": context,
            "actions": [
                {
                    "type": "pointer",
                    "id": source_id,
                    "parameters": {"pointerType": "mouse"},
                    "actions": [
                        {"type": "pointerMove", "x": x, "y": y, "origin": "viewport"},
                        {"type": "pointerDown", "button": 0},
                        {"type": "pointerUp", "button": 0},
                    ],
                }
            ],
        },
    )
    try:
        client.request("input.releaseActions", {"context": context})
    except BrokerError:
        pass


def worker_cancelled(root: Path, job_id: str, signal_flag: list[bool]) -> bool:
    if signal_flag[0]:
        return True
    try:
        return bool(load_state(root, job_id).get("cancel_requested"))
    except BrokerError:
        return True


def wait_for_rect(
    client: BidiClient,
    context: str,
    expression: str,
    deadline: float,
    browser: subprocess.Popen[Any],
    cancelled: Callable[[], bool],
    error_code: str,
) -> tuple[int, int]:
    while time.monotonic() < deadline:
        if cancelled():
            raise BrokerError("cancelled")
        if browser.poll() is not None:
            raise BrokerError("browser_closed")
        try:
            value = eval_json(client, context, expression)
            if isinstance(value, dict) and value.get("found"):
                return int(value["x"]), int(value["y"])
        except (BrokerError, KeyError, TypeError, ValueError):
            pass
        time.sleep(POLL_SECONDS)
    raise BrokerError(error_code)


def cookie_snapshot(client: BidiClient, top_context: str) -> dict[tuple[str, str], str]:
    reply = client.request(
        "storage.getCookies",
        {"partition": {"type": "context", "context": top_context}},
    )
    snapshot: dict[tuple[str, str], str] = {}
    for cookie in reply.get("result", {}).get("cookies", []):
        if not isinstance(cookie, dict):
            continue
        domain = str(cookie.get("domain", "")).lower().lstrip(".")
        name = str(cookie.get("name", ""))
        if (domain == "qq.com" or domain.endswith(".qq.com")) and name:
            # Values remain memory-only and are used solely to reject unchanged stale
            # baseline cookies. They are never placed in state, logs, or output.
            snapshot[(domain, name)] = json.dumps(
                cookie.get("value"),
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            )
    return snapshot


def is_auth_cookie_name(name: str) -> bool:
    lowered = name.lower()
    if lowered in AUTH_COOKIE_NAMES:
        return True
    if lowered.startswith(TRACKING_COOKIE_PREFIXES):
        return False
    return bool(AUTH_NAME_PATTERN.search(lowered))


def current_has_auth_cookie(current: dict[tuple[str, str], str]) -> bool:
    return any(is_auth_cookie_name(name) for _, name in current)


def has_auth_cookie_signal(
    baseline: dict[tuple[str, str], str], current: dict[tuple[str, str], str]
) -> bool:
    for pair, value in current.items():
        if is_auth_cookie_name(pair[1]) and (
            pair not in baseline
            or not secrets.compare_digest(
                baseline[pair].encode("utf-8"), value.encode("utf-8")
            )
        ):
            return True
    return False


def authorization_complete(evidence: dict[str, Any]) -> bool:
    return bool(evidence.get("prior_verified")) or all(
        bool(evidence.get(key))
        for key in ("callback_seen", "cookie_signal", "user_dom")
    )


def transition_terminal(
    root: Path, job_id: str, status_text: str, reason: str
) -> dict[str, Any]:
    if status_text not in INTERNAL_TERMINAL_STATUSES:
        status_text = "failed"

    def apply(state: dict[str, Any]) -> None:
        if state.get("status") != "waiting":
            return
        if status_text == "authorized" and not authorization_complete(
            state.get("evidence", {})
        ):
            return
        state["status"] = status_text
        state["reason"] = reason

    return mutate_state(root, job_id, apply)


def fail_waiting_job(root: Path, job_id: str, reason: str) -> dict[str, Any]:
    def apply(state: dict[str, Any]) -> None:
        if state.get("status") == "waiting":
            state["status"] = "failed"
            state["reason"] = reason
            state["cancel_requested"] = True

    return mutate_state(root, job_id, apply)


def update_process_fields(root: Path, job_id: str, **fields: Any) -> dict[str, Any]:
    allowed = {
        "launch_kind",
        "unit",
        "worker_pid",
        "worker_pgid",
        "worker_start_ticks",
        "browser_pid",
        "browser_pgid",
        "browser_start_ticks",
        "port",
    }
    if any(key not in allowed for key in fields):
        raise BrokerError("invalid_process_field")

    def apply(state: dict[str, Any]) -> None:
        for key, value in fields.items():
            state[key] = value

    return mutate_state(root, job_id, apply)


def direct_process_environment() -> dict[str, str]:
    environment = dict(os.environ)
    for key in PROXY_ENV_KEYS:
        environment.pop(key, None)
    return environment


def spawn_owned_browser(
    root: Path,
    job_id: str,
    command: list[str],
    port: int,
) -> tuple[subprocess.Popen[Any], int, int, dict[str, Any]]:
    # Hold the job lock across the final gate, spawn, and PID publication. Thus
    # cancel either wins before spawn or observes enough identity to clean it.
    with exclusive_file_lock(state_lock_path(root, job_id)):
        state = load_state(root, job_id)
        if state.get("status") != "waiting" or bool(state.get("cancel_requested")):
            raise BrokerError("cancelled")
        process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            start_new_session=True,
            env=direct_process_environment(),
        )
        try:
            process_pgid = os.getpgid(process.pid)
            start_ticks = process_start_ticks(process.pid)
            if start_ticks is None:
                raise BrokerError("browser_closed")
        except Exception:
            try:
                process.terminate()
            except OSError:
                pass
            raise
        state["browser_pid"] = process.pid
        state["browser_pgid"] = process_pgid
        state["browser_start_ticks"] = start_ticks
        state["port"] = port
        save_state_unlocked(root, state)
        return process, process_pgid, start_ticks, state


def hold_authorized_browser(
    root: Path,
    job_id: str,
    browser: subprocess.Popen[Any],
    cancelled: Callable[[], bool],
) -> None:
    while True:
        state = load_state(root, job_id)
        if (
            state.get("status") != "authorized"
            or cancelled()
            or browser.poll() is not None
            or time.time() >= float(state.get("process_deadline_at", 0))
        ):
            return
        time.sleep(POLL_SECONDS)


def run_worker(job_id: str) -> int:
    root = runtime_root()
    signal_flag = [False]

    def request_stop(_signum: int, _frame: Any) -> None:
        signal_flag[0] = True

    def enforce_process_deadline(_signum: int, _frame: Any) -> None:
        # This is deliberately not BrokerError: narrow DOM/BiDi recovery blocks
        # catch BrokerError and must never swallow the one-shot hard deadline.
        raise ProcessDeadlineError

    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    signal.signal(signal.SIGALRM, enforce_process_deadline)

    state = update_process_fields(
        root,
        job_id,
        worker_pid=os.getpid(),
        worker_pgid=os.getpgid(0),
        worker_start_ticks=process_start_ticks(os.getpid()),
    )
    safe_log(root, job_id, "worker_started")

    browser: subprocess.Popen[Any] | None = None
    client: BidiClient | None = None
    profile: Path | None = None
    terminal_status = "failed"
    terminal_reason = "worker_failed"
    authorization_verified = False
    try:
        remaining_lifetime = float(state.get("process_deadline_at", 0)) - time.time()
        if remaining_lifetime <= 0:
            raise BrokerError("process_deadline")
        signal.setitimer(signal.ITIMER_REAL, remaining_lifetime)
        if sys.platform != "linux":
            raise BrokerError("linux_required")
        if not os.environ.get("DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
            raise BrokerError("display_unavailable")
        firefox = shutil.which("firefox")
        if not firefox:
            raise BrokerError("firefox_missing")

        port = pick_port()
        profile = firefox_profile_dir()
        browser, browser_pgid, browser_start_ticks, state = spawn_owned_browser(
            root,
            job_id,
            [
                firefox,
                "--new-instance",
                "--profile",
                str(profile),
                f"--remote-debugging-port={port}",
                "about:blank",
            ],
            port,
        )
        safe_log(root, job_id, "browser_started")

        def cancelled() -> bool:
            return worker_cancelled(root, job_id, signal_flag)

        wait_debugger(
            port,
            browser.pid,
            browser_pgid,
            browser_start_ticks,
            bounded_monotonic_deadline(state, 20.0),
        )
        if cancelled():
            raise BrokerError("cancelled")
        if not listener_owned_by_group(port, browser_pgid):
            raise BrokerError("browser_listener_unowned")

        client = BidiClient(port)
        if not process_identity_matches(
            browser.pid, browser_start_ticks
        ) or not listener_owned_by_group(port, browser_pgid):
            raise BrokerError("browser_listener_unowned")
        client.request("session.new", {"capabilities": {}})
        tree = client.request("browsingContext.getTree", {"maxDepth": 5})
        contexts = flatten_contexts(tree)
        if not contexts:
            raise BrokerError("browser_context_missing")
        top_context = contexts[0][0]
        client.request(
            "session.subscribe",
            {
                "events": [
                    "browsingContext.contextCreated",
                    "browsingContext.contextDestroyed",
                    "browsingContext.navigationStarted",
                    "browsingContext.domContentLoaded",
                    "browsingContext.load",
                ]
            },
        )
        client.request(
            "browsingContext.navigate",
            {"context": top_context, "url": QQ_MUSIC_URL, "wait": "none"},
        )

        login_xy: tuple[int, int] | None = None
        baseline_cookies: dict[tuple[str, str], str] = {}
        warm_unverified_since: float | None = None
        entry_deadline = bounded_monotonic_deadline(state, 20.0)
        while time.monotonic() < entry_deadline:
            if cancelled():
                raise BrokerError("cancelled")
            if browser.poll() is not None:
                raise BrokerError("browser_closed")
            try:
                baseline_cookies = cookie_snapshot(client, top_context)
                warm_user_dom = bool(eval_json(client, top_context, USER_DOM_JS))
                warm_cookie = current_has_auth_cookie(baseline_cookies)
                if warm_user_dom and warm_cookie and auth_marker_valid(profile):
                    state = record_evidence(
                        root,
                        job_id,
                        user_dom=True,
                        prior_verified=True,
                    )
                    state = transition_terminal(
                        root, job_id, "authorized", "verified_warm"
                    )
                    if state.get("status") == "authorized":
                        authorization_verified = True
                        terminal_status, terminal_reason = "authorized", "verified_warm"
                        write_auth_marker_best_effort(profile, root, job_id)
                        safe_log(root, job_id, "warm_authorization_verified")
                        break
                if warm_user_dom and warm_cookie:
                    if warm_unverified_since is None:
                        warm_unverified_since = time.monotonic()
                    elif time.monotonic() - warm_unverified_since >= 1.0:
                        raise BrokerError("warm_profile_unverified")
                rect = eval_json(client, top_context, LOGIN_RECT_JS)
                if isinstance(rect, dict) and rect.get("found"):
                    login_xy = (int(rect["x"]), int(rect["y"]))
                    break
            except (KeyError, TypeError, ValueError):
                pass
            time.sleep(POLL_SECONDS)
        if not authorization_verified:
            if login_xy is None:
                raise BrokerError("login_entry_timeout")
            pointer_click(client, top_context, *login_xy, "pinvou-login")
            wx_xy = wait_for_rect(
                client,
                top_context,
                WECHAT_RECT_JS,
                bounded_monotonic_deadline(state, 15.0),
                browser,
                cancelled,
                "wechat_tab_timeout",
            )
            pointer_click(client, top_context, *wx_xy, "pinvou-wechat")
            safe_log(root, job_id, "wechat_tab_clicked")

        if authorization_verified:
            hold_authorized_browser(root, job_id, browser, cancelled)
            return 0

        callback_seen = False
        qr_deadline = min(effective_active_deadline(state), time.time() + 20.0)
        qr_ready = False
        while time.time() < qr_deadline and not qr_ready:
            if cancelled():
                raise BrokerError("cancelled")
            if browser.poll() is not None:
                raise BrokerError("browser_closed")
            callback_seen = callback_seen or events_show_callback(client.take_events())
            tree = client.request("browsingContext.getTree", {"maxDepth": 5})
            for context, url in flatten_contexts(tree):
                callback_seen = callback_seen or is_callback_url(url)
                try:
                    qr_state = eval_json(client, context, QR_STATE_JS)
                except BrokerError:
                    continue
                if not isinstance(qr_state, dict):
                    continue
                if qr_state.get("state") == "expired":
                    terminal_status, terminal_reason = "expired", "challenge_expired"
                    raise BrokerError("challenge_expired")
                if qr_state.get("state") == "scanned":
                    record_evidence(root, job_id, scanned=True)
                if qr_state.get("state") == "ready":
                    qr_ready = True
                    break
            if not qr_ready:
                time.sleep(POLL_SECONDS)
        if not qr_ready:
            raise BrokerError("qr_ready_timeout")

        current = eval_json(client, top_context, WECHAT_CURRENT_JS)
        if not bool(current):
            raise BrokerError("wechat_tab_not_current")
        state = record_evidence(
            root,
            job_id,
            qr_ready=True,
            callback_seen=callback_seen,
        )
        safe_log(root, job_id, "qr_ready")

        reloaded_for_user_dom = False
        while time.time() < effective_active_deadline(state):
            if cancelled():
                raise BrokerError("cancelled")
            if browser.poll() is not None:
                raise BrokerError("browser_closed")

            state = load_state(root, job_id)
            evidence = state["evidence"]
            callback_fact = bool(evidence.get("callback_seen"))
            scanned_fact = False
            if events_show_callback(client.take_events()):
                callback_fact = True

            tree = client.request("browsingContext.getTree", {"maxDepth": 5})
            contexts = flatten_contexts(tree)
            for context, url in contexts:
                if is_callback_url(url):
                    callback_fact = True
                try:
                    qr_state = eval_json(client, context, QR_STATE_JS)
                except BrokerError:
                    continue
                if isinstance(qr_state, dict):
                    if qr_state.get("state") == "scanned":
                        scanned_fact = True
                    elif qr_state.get("state") == "expired" and not callback_fact:
                        terminal_status, terminal_reason = (
                            "expired",
                            "challenge_expired",
                        )
                        raise BrokerError("challenge_expired")

            cookie_fact = False
            user_dom_fact = False
            if callback_fact:
                try:
                    current_cookies = cookie_snapshot(client, top_context)
                    cookie_fact = has_auth_cookie_signal(
                        baseline_cookies, current_cookies
                    )
                except BrokerError:
                    pass
                try:
                    user_dom_fact = bool(eval_json(client, top_context, USER_DOM_JS))
                except BrokerError:
                    pass
                if cookie_fact and not user_dom_fact and not reloaded_for_user_dom:
                    try:
                        client.request(
                            "browsingContext.navigate",
                            {
                                "context": top_context,
                                "url": QQ_MUSIC_URL,
                                "wait": "none",
                            },
                        )
                        reloaded_for_user_dom = True
                    except BrokerError:
                        pass

            state = record_evidence(
                root,
                job_id,
                callback_seen=callback_fact,
                scanned=scanned_fact,
                cookie_signal=cookie_fact,
                user_dom=user_dom_fact,
            )
            if authorization_complete(state["evidence"]):
                terminal_status, terminal_reason = "authorized", "verified"
                state = transition_terminal(
                    root, job_id, terminal_status, terminal_reason
                )
                if state.get("status") == "authorized":
                    authorization_verified = True
                    if profile is not None:
                        write_auth_marker_best_effort(profile, root, job_id)
                    safe_log(root, job_id, "authorization_verified")
                    hold_authorized_browser(root, job_id, browser, cancelled)
                    return 0
                raise BrokerError("cancelled")
            time.sleep(POLL_SECONDS)
        else:
            terminal_status, terminal_reason = "expired", "ttl_expired"

    except ProcessDeadlineError:
        terminal_status, terminal_reason = "expired", "process_deadline"
        safe_log(root, job_id, "worker_terminal")
    except BrokerError as exc:
        if exc.code == "cancelled":
            terminal_status, terminal_reason = "cancelled", "cancelled"
        elif exc.code == "challenge_expired":
            terminal_status, terminal_reason = "expired", exc.code
        else:
            terminal_status, terminal_reason = "failed", exc.code
        safe_log(root, job_id, "worker_terminal")
    except Exception:
        terminal_status, terminal_reason = "failed", "unexpected_failure"
        safe_log(root, job_id, "worker_terminal")
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0.0)
        if client is not None:
            try:
                client.request("session.end", {}, timeout=3.0)
            except BrokerError:
                pass
            client.close()
        try:
            transition_terminal(root, job_id, terminal_status, terminal_reason)
            cleanup_state = load_state(root, job_id)
            terminate_owned_group(
                cleanup_state.get("browser_pid"),
                cleanup_state.get("browser_pgid"),
                cleanup_state.get("browser_start_ticks"),
            )
            update_process_fields(
                root,
                job_id,
                worker_pid=None,
                worker_pgid=None,
                worker_start_ticks=None,
                browser_pid=None,
                browser_pgid=None,
                browser_start_ticks=None,
                port=None,
            )
        except BrokerError:
            pass
        release_active(root, job_id)
        safe_log(root, job_id, "worker_stopped")
    try:
        final_status = load_state(root, job_id).get("status")
    except BrokerError:
        final_status = "failed"
    return 0 if final_status in {"authorized", "cancelled"} else 1


def systemd_environment_args() -> list[str]:
    args: list[str] = []
    for key in (
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "XAUTHORITY",
        "LANG",
        "LC_ALL",
    ):
        value = os.environ.get(key)
        if value:
            args.append(f"--setenv={key}={value}")
    return args


def require_worker_launch_allowed(state: dict[str, Any]) -> None:
    if state.get("status") != "waiting" or bool(state.get("cancel_requested")):
        raise BrokerError("cancelled")


def launch_worker(
    root: Path,
    state: dict[str, Any],
    after_create_for_test: Callable[[str], None] | None = None,
) -> None:
    job_id = str(state["job_id"])
    script = str(Path(__file__).resolve())
    unit = f"pinvou-browser-auth-{job_id}.service"
    if not Path(ISOLATED_PYTHON).is_file():
        raise BrokerError("isolated_python_missing")

    # The job lock is also used by cancel/status mutations. Keep it across both
    # creation and ownership publication so cancel either wins the initial gate
    # or observes a complete unit/PGID identity that it can clean precisely.
    with exclusive_file_lock(state_lock_path(root, job_id)):
        state = load_state(root, job_id)
        require_worker_launch_allowed(state)
        state["launch_kind"] = "systemd"
        state["unit"] = unit
        save_state_unlocked(root, state)

        systemd_run = shutil.which("systemd-run")
        if systemd_run:
            command = [
                systemd_run,
                "--user",
                "--quiet",
                "--collect",
                f"--unit={unit[:-8]}",
                "--property=Type=exec",
                "--property=KillMode=control-group",
                "--property=TimeoutStopSec=10s",
                f"--property=RuntimeMaxSec={PROCESS_ACTIVE_LIFETIME_SECONDS}s",
                "--property=UnsetEnvironment=" + " ".join(PROXY_ENV_KEYS),
                "--property=StandardOutput=null",
                "--property=StandardError=null",
                *systemd_environment_args(),
                ISOLATED_PYTHON,
                "-I",
                script,
                "_worker",
                "--job-id",
                job_id,
            ]
            try:
                result = subprocess.run(
                    command,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=10,
                    check=False,
                )
                if result.returncode == 0:
                    if after_create_for_test is not None:
                        after_create_for_test("systemd")
                    require_worker_launch_allowed(state)
                    save_state_unlocked(root, state)
                    safe_log(root, job_id, "launched_systemd")
                    return
            except (subprocess.TimeoutExpired, OSError):
                pass

        stop_systemd_unit(unit)
        require_worker_launch_allowed(state)
        state["launch_kind"] = "pgid"
        state["unit"] = None
        save_state_unlocked(root, state)

        process: subprocess.Popen[Any] | None = None
        process_pgid: int | None = None
        start_ticks: int | None = None
        try:
            process = subprocess.Popen(
                [ISOLATED_PYTHON, "-I", script, "_worker", "--job-id", job_id],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                close_fds=True,
                start_new_session=True,
                env=direct_process_environment(),
            )
            process_pgid = os.getpgid(process.pid)
            start_ticks = process_start_ticks(process.pid)
            if start_ticks is None:
                raise BrokerError("worker_closed")
            if after_create_for_test is not None:
                after_create_for_test("pgid")
            require_worker_launch_allowed(state)
            state["worker_pid"] = process.pid
            state["worker_pgid"] = process_pgid
            state["worker_start_ticks"] = start_ticks
            save_state_unlocked(root, state)
            safe_log(root, job_id, "launched_pgid")
        except Exception:
            if (
                process is not None
                and process_pgid is not None
                and start_ticks is not None
            ):
                terminate_owned_group(process.pid, process_pgid, start_ticks)
            elif process is not None:
                try:
                    process.terminate()
                except OSError:
                    pass
            raise


def command_start(ttl_seconds: int) -> dict[str, Any]:
    if sys.platform != "linux":
        raise BrokerError("linux_required")
    if ttl_seconds < 1 or ttl_seconds > MAX_TTL_SECONDS:
        raise BrokerError("invalid_ttl")
    root = runtime_root()
    job_id = secrets.token_hex(12)
    state = new_state(job_id, ttl_seconds, "pending", None)
    existing = claim_active(root, state)
    if existing:
        return wait_until_qr_ready(root, existing)
    try:
        launch_worker(root, state)
    except Exception:
        transition_terminal(root, job_id, "failed", "worker_launch_failed")
        release_active(root, job_id)
    return wait_until_qr_ready(root, job_id)


def wait_until_qr_ready(root: Path, job_id: str) -> dict[str, Any]:
    state = load_state(root, job_id)
    deadline = min(
        effective_active_deadline(state),
        time.time() + START_READY_TIMEOUT_SECONDS,
    )
    while time.time() < deadline:
        state = load_state(root, job_id)
        if state.get("status") != "waiting":
            return public_result(state)
        if not active_state_is_reusable(state):
            state = fail_waiting_job(root, job_id, "stale_active_job")
            terminate_state_processes(state)
            release_active(root, job_id)
            return public_result(state)
        if bool(state.get("evidence", {}).get("qr_ready")):
            return public_result(state)
        time.sleep(0.1)
    timed_out = [False]

    def fail_if_unready(state: dict[str, Any]) -> None:
        if state.get("status") == "waiting" and not bool(
            state.get("evidence", {}).get("qr_ready")
        ):
            state["status"] = "failed"
            state["reason"] = "start_ready_timeout"
            state["cancel_requested"] = True
            timed_out[0] = True

    state = mutate_state(root, job_id, fail_if_unready)
    if timed_out[0]:
        terminate_state_processes(state)
        release_active(root, job_id)
    return public_result(load_state(root, job_id))


def stop_systemd_unit(unit: Any) -> None:
    if not isinstance(unit, str) or not UNIT_RE.fullmatch(unit):
        return
    systemctl = shutil.which("systemctl")
    if not systemctl:
        return
    try:
        subprocess.run(
            [systemctl, "--user", "stop", unit],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=12,
            check=False,
        )
    except (subprocess.TimeoutExpired, OSError):
        pass


def terminate_state_processes(state: dict[str, Any]) -> None:
    stop_systemd_unit(state.get("unit"))
    terminate_owned_group(
        state.get("browser_pid"),
        state.get("browser_pgid"),
        state.get("browser_start_ticks"),
    )
    terminate_owned_group(
        state.get("worker_pid"),
        state.get("worker_pgid"),
        state.get("worker_start_ticks"),
    )


def command_status(job_id: str) -> dict[str, Any]:
    root = runtime_root()
    validate_job_id(job_id)
    cleanup_required = [False]

    def refresh_liveness(state: dict[str, Any]) -> None:
        if state.get(
            "status"
        ) == "waiting" and time.time() >= effective_active_deadline(state):
            state["status"] = "expired"
            state["reason"] = "ttl_expired"
            state["cancel_requested"] = True
            cleanup_required[0] = True
        elif state.get("status") == "authorized" and not active_state_is_reusable(
            state
        ):
            state["status"] = "expired"
            state["reason"] = "authorized_handoff_expired"
            state["cancel_requested"] = True
            cleanup_required[0] = True

    # active_state_is_reusable probes the exact worker/browser identities and
    # browser-owned listener while this state lock is held. A stale authorized
    # handoff is therefore persisted as expired before cleanup or publication.
    state = mutate_state(root, job_id, refresh_liveness)
    if state.get("status") == "waiting" and not active_state_is_reusable(state):
        state = fail_waiting_job(root, job_id, "stale_active_job")
        cleanup_required[0] = True
    if cleanup_required[0]:
        terminate_state_processes(state)
        release_active(root, job_id)
    return public_result(load_state(root, job_id))


def command_cancel(job_id: str) -> dict[str, Any]:
    root = runtime_root()
    validate_job_id(job_id)

    def cancel(state: dict[str, Any]) -> None:
        if state.get("status") in {"waiting", "authorized"}:
            state["cancel_requested"] = True
            state["status"] = "cancelled"
            state["reason"] = "cancelled"

    state = mutate_state(root, job_id, cancel)
    terminate_state_processes(state)
    release_active(root, job_id)
    safe_log(root, job_id, "cancelled")
    return public_result(state)


def expect_broker_error(call: Callable[[], Any], code: str) -> None:
    try:
        call()
    except BrokerError as exc:
        assert exc.code == code, (exc.code, code)
    else:
        raise AssertionError(f"expected {code}")


def test_websocket_peer() -> tuple[StdlibWebSocket, socket.socket]:
    client_socket, peer = socket.socketpair()
    client_socket.settimeout(1.0)
    peer.settimeout(1.0)
    websocket = object.__new__(StdlibWebSocket)
    websocket.sock = client_socket
    websocket.buffer = bytearray()
    websocket.closed = False
    return websocket, peer


def socket_read_exact(sock: socket.socket, length: int) -> bytes:
    value = bytearray()
    while len(value) < length:
        chunk = sock.recv(length - len(value))
        if not chunk:
            raise AssertionError("unexpected socket EOF")
        value.extend(chunk)
    return bytes(value)


def read_masked_client_frame(sock: socket.socket) -> tuple[int, bytes]:
    first, second = socket_read_exact(sock, 2)
    assert first & 0x80
    assert second & 0x80
    length_code = second & 0x7F
    if length_code == 126:
        length = struct.unpack("!H", socket_read_exact(sock, 2))[0]
    elif length_code == 127:
        length = struct.unpack("!Q", socket_read_exact(sock, 8))[0]
    else:
        length = length_code
    mask = socket_read_exact(sock, 4)
    payload = socket_read_exact(sock, length)
    return first & 0x0F, bytes(
        byte ^ mask[index % 4] for index, byte in enumerate(payload)
    )


def server_frame(
    opcode: int, payload: bytes, *, fin: bool = True, masked: bool = False
) -> bytes:
    first = (0x80 if fin else 0) | opcode
    length = len(payload)
    mask_bit = 0x80 if masked else 0
    if length < 126:
        header = bytes((first, mask_bit | length))
    elif length <= 0xFFFF:
        header = bytes((first, mask_bit | 126)) + struct.pack("!H", length)
    else:
        header = bytes((first, mask_bit | 127)) + struct.pack("!Q", length)
    if not masked:
        return header + payload
    mask = b"test"
    transformed = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
    return header + mask + transformed


def run_websocket_self_test() -> None:
    for length, expected_code in ((125, 125), (126, 126), (65536, 127)):
        websocket, peer = test_websocket_peer()
        try:
            payload = b"x" * length
            websocket._send_frame(0x1, payload, time.monotonic() + 1.0)
            first, second = socket_read_exact(peer, 2)
            assert first == 0x81 and second & 0x80
            assert second & 0x7F == expected_code
            if expected_code == 126:
                assert struct.unpack("!H", socket_read_exact(peer, 2))[0] == length
            elif expected_code == 127:
                assert struct.unpack("!Q", socket_read_exact(peer, 8))[0] == length
            mask = socket_read_exact(peer, 4)
            masked = socket_read_exact(peer, length)
            assert (
                bytes(byte ^ mask[index % 4] for index, byte in enumerate(masked))
                == payload
            )
        finally:
            websocket.sock.close()
            peer.close()

    websocket, peer = test_websocket_peer()
    try:
        peer.sendall(
            server_frame(0x1, b'{"id":', fin=False)
            + server_frame(0x9, b"p")
            + server_frame(0x0, b"1}")
        )
        assert websocket.recv_json(time.monotonic() + 1.0) == {"id": 1}
        assert read_masked_client_frame(peer) == (0xA, b"p")
    finally:
        websocket.sock.close()
        peer.close()

    websocket, peer = test_websocket_peer()
    try:
        peer.sendall(server_frame(0x1, b"{}", masked=True))
        expect_broker_error(
            lambda: websocket.recv_json(time.monotonic() + 1.0),
            "websocket_protocol_error",
        )
    finally:
        websocket.sock.close()
        peer.close()

    websocket, peer = test_websocket_peer()
    try:
        peer.sendall(b"\x81\x7e\x00\x01x")
        expect_broker_error(
            lambda: websocket.recv_json(time.monotonic() + 1.0),
            "websocket_protocol_error",
        )
    finally:
        websocket.sock.close()
        peer.close()

    websocket, peer = test_websocket_peer()
    try:
        peer.sendall(
            server_frame(0x1, b"{", fin=False)
            + b"\x80\x7f"
            + struct.pack("!Q", MAX_WS_MESSAGE_BYTES)
        )
        expect_broker_error(
            lambda: websocket.recv_json(time.monotonic() + 1.0),
            "websocket_message_too_large",
        )
    finally:
        websocket.sock.close()
        peer.close()

    websocket, peer = test_websocket_peer()
    try:
        close_payload = struct.pack("!H", 1000) + b"done"
        peer.sendall(server_frame(0x8, close_payload))
        expect_broker_error(
            lambda: websocket.recv_json(time.monotonic() + 1.0), "bidi_closed"
        )
        assert read_masked_client_frame(peer) == (0x8, close_payload)
    finally:
        peer.close()

    websocket, peer = test_websocket_peer()
    try:
        started = time.monotonic()
        expect_broker_error(
            lambda: websocket.recv_json(time.monotonic() + 0.05), "bidi_timeout"
        )
        assert time.monotonic() - started < 0.5
    finally:
        websocket.sock.close()
        peer.close()

    key = "dGhlIHNhbXBsZSBub25jZQ=="
    accept = base64.b64encode(
        hashlib.sha1((key + StdlibWebSocket.GUID).encode("ascii")).digest()
    ).decode("ascii")
    valid = (
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Accept: {accept}\r\n\r\n"
    ).encode("ascii")
    StdlibWebSocket.validate_upgrade_response(valid, key)
    for invalid in (
        valid.replace(b"HTTP/1.1", b"HTTP/1.0"),
        valid.replace(
            b"Sec-WebSocket-Accept:",
            f"Sec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Accept:".encode("ascii"),
        ),
        valid.replace(b"\r\n\r\n", b"\r\nSec-WebSocket-Extensions: x\r\n\r\n"),
        valid.replace(b"Upgrade:", b" Upgrade:"),
        valid.replace(b"Switching Protocols", b"Switching\x00Protocols"),
        valid.replace(b"Connection: Upgrade", b"Connection: Upgrade\x7f"),
    ):
        expect_broker_error(
            lambda invalid=invalid: StdlibWebSocket.validate_upgrade_response(
                invalid, key
            ),
            "websocket_upgrade_failed",
        )

    websocket, peer = test_websocket_peer()
    try:
        peer.sendall(b"HTTP/1.1 101\r\nX: " + b"a" * 20000 + b"\r\n\r\n")
        expect_broker_error(
            lambda: websocket._read_http_headers(time.monotonic() + 1.0),
            "websocket_upgrade_failed",
        )
    finally:
        websocket.sock.close()
        peer.close()


def assert_job_lock_held(root: Path, job_id: str) -> None:
    assert fcntl is not None
    path = state_lock_path(root, job_id)
    fd = os.open(path, os.O_RDWR | getattr(os, "O_NOFOLLOW", 0))
    acquired = False
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            acquired = True
        except BlockingIOError:
            return
    finally:
        if acquired:
            fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)
    raise AssertionError("worker creation was not serialized by the job lock")


def run_worker_launch_lock_self_test(root: Path) -> None:
    global terminate_state_processes

    original_which = shutil.which
    original_run = subprocess.run
    original_popen = subprocess.Popen
    original_terminate_state_processes = terminate_state_processes
    cleaned_states: list[dict[str, Any]] = []

    def capture_cleanup(state: dict[str, Any]) -> None:
        cleaned_states.append(dict(state))

    terminate_state_processes = capture_cleanup
    try:
        systemd_job = "e" * 24
        systemd_state = new_state(systemd_job, 60, "pending", None)
        create_state(root, systemd_state)

        def systemd_hook(kind: str) -> None:
            assert kind == "systemd"
            assert_job_lock_held(root, systemd_job)
            observed = load_state(root, systemd_job)
            assert observed["unit"] == f"pinvou-browser-auth-{systemd_job}.service"

        def fake_run(command: list[str], **_kwargs: Any) -> Any:
            return subprocess.CompletedProcess(command, 0)

        try:
            shutil.which = lambda executable: (
                "/test/systemd-run" if executable == "systemd-run" else None
            )
            subprocess.run = fake_run
            launch_worker(root, systemd_state, systemd_hook)
        finally:
            shutil.which = original_which
            subprocess.run = original_run

        assert command_cancel(systemd_job)["status"] == "cancelled"
        assert cleaned_states[-1]["unit"] == (
            f"pinvou-browser-auth-{systemd_job}.service"
        )

        fallback_job = "f" * 24
        fallback_state = new_state(fallback_job, 60, "pending", None)
        create_state(root, fallback_state)

        class FakeProcess:
            pid = os.getpid()

            def terminate(self) -> None:
                return

        def fallback_hook(kind: str) -> None:
            assert kind == "pgid"
            assert_job_lock_held(root, fallback_job)
            observed = load_state(root, fallback_job)
            assert observed["worker_pid"] is None

        try:
            shutil.which = lambda _executable: None
            subprocess.Popen = lambda *_args, **_kwargs: FakeProcess()
            launch_worker(root, fallback_state, fallback_hook)
        finally:
            shutil.which = original_which
            subprocess.Popen = original_popen

        published = load_state(root, fallback_job)
        assert published["worker_pid"] == os.getpid()
        assert published["worker_pgid"] == os.getpgid(0)
        assert published["worker_start_ticks"] == process_start_ticks(os.getpid())
        assert command_cancel(fallback_job)["status"] == "cancelled"
        assert cleaned_states[-1]["worker_pid"] == os.getpid()
        assert cleaned_states[-1]["worker_pgid"] == os.getpgid(0)
    finally:
        shutil.which = original_which
        subprocess.run = original_run
        subprocess.Popen = original_popen
        terminate_state_processes = original_terminate_state_processes


def run_authorized_status_self_test(root: Path) -> None:
    global listener_owned_by_group, terminate_state_processes

    original_listener_owned_by_group = listener_owned_by_group
    original_terminate_state_processes = terminate_state_processes
    cleaned_states: list[dict[str, Any]] = []

    def authorized_state(job_id: str) -> dict[str, Any]:
        state = new_state(job_id, 60, "pgid", None)
        pid = os.getpid()
        start_ticks = process_start_ticks(pid)
        assert start_ticks is not None
        state["status"] = "authorized"
        state["worker_pid"] = pid
        state["worker_pgid"] = os.getpgid(pid)
        state["worker_start_ticks"] = start_ticks
        state["browser_pid"] = pid
        state["browser_pgid"] = os.getpgid(pid)
        state["browser_start_ticks"] = start_ticks
        state["port"] = 43123
        state["evidence"]["prior_verified"] = True
        return state

    def listener_is_owned(_port: int, _pgid: int) -> bool:
        return True

    def capture_cleanup(state: dict[str, Any]) -> None:
        cleaned_states.append(dict(state))

    try:
        listener_owned_by_group = listener_is_owned
        terminate_state_processes = capture_cleanup

        live_job = "1" * 24
        create_state(root, authorized_state(live_job))
        assert command_status(live_job)["status"] == "authorized"
        assert cleaned_states == []

        stale_job = "2" * 24
        stale = authorized_state(stale_job)
        stale["worker_start_ticks"] = int(stale["worker_start_ticks"]) + 1
        create_state(root, stale)
        assert command_status(stale_job)["status"] == "expired"
        persisted = load_state(root, stale_job)
        assert persisted["status"] == "expired"
        assert persisted["reason"] == "authorized_handoff_expired"
        assert persisted["cancel_requested"] is True
        assert cleaned_states[-1]["job_id"] == stale_job
    finally:
        listener_owned_by_group = original_listener_owned_by_group
        terminate_state_processes = original_terminate_state_processes


def run_self_test() -> dict[str, Any]:
    assert authorization_complete(
        {"callback_seen": True, "cookie_signal": True, "user_dom": True}
    )
    assert not authorization_complete(
        {
            "callback_seen": True,
            "cookie_signal": False,
            "user_dom": True,
            "scanned": True,
        }
    )
    assert authorization_complete({"prior_verified": True})
    baseline = {("y.qq.com", "ts_uid"): "old", ("y.qq.com", "qqmusic_key"): "stale"}
    assert has_auth_cookie_signal(
        baseline, baseline | {("y.qq.com", "qqmusic_key"): "fresh"}
    )
    assert has_auth_cookie_signal(
        {("y.qq.com", "qqmusic_key"): "旧"},
        {("y.qq.com", "qqmusic_key"): "新"},
    )
    assert not has_auth_cookie_signal(baseline, dict(baseline))
    assert not has_auth_cookie_signal(
        baseline, baseline | {("y.qq.com", "pgv_pvid"): "tracking"}
    )
    assert is_callback_url("https://y.qq.com/portal/wx_redirect.html?login")
    assert is_callback_url("https://y.qq.com:443/portal/wx_redirect.html")
    assert not is_callback_url("https://y.qq.com/")
    assert not is_callback_url("https://y.qq.com/portal/wx_redirect.html.evil?login")
    assert not is_callback_url("http://y.qq.com/portal/wx_redirect.html")
    assert not is_callback_url("https://y.qq.com:444/portal/wx_redirect.html")
    assert not is_callback_url("https://evil@y.qq.com/portal/wx_redirect.html")

    run_websocket_self_test()

    prior = os.environ.get("XDG_RUNTIME_DIR")
    with tempfile.TemporaryDirectory() as tmp:
        os.chmod(tmp, 0o700)
        os.environ["XDG_RUNTIME_DIR"] = tmp
        root = runtime_root()
        job_id = "a" * 24
        state = new_state(job_id, 60, "selftest", None)
        assert (
            state["process_deadline_at"]
            - state["created_at"]
            + PROCESS_CLEANUP_GUARD_SECONDS
            == MAX_TTL_SECONDS
        )
        max_budget_state = new_state("d" * 24, MAX_TTL_SECONDS, "pgid", None)
        assert (
            effective_active_deadline(max_budget_state)
            - max_budget_state["created_at"]
            + PROCESS_CLEANUP_GUARD_SECONDS
            == MAX_TTL_SECONDS
        )
        create_state(root, state)
        assert stat.S_IMODE(root.stat().st_mode) == 0o700
        assert stat.S_IMODE(state_path(root, job_id).stat().st_mode) == 0o600
        result = public_result(load_state(root, job_id))
        assert set(result) == {"job_id", "status", "evidence"}
        assert set(result["evidence"]) == set(EVIDENCE_KEYS)
        assert all(isinstance(value, bool) for value in result["evidence"].values())
        live_worker = dict(state)
        live_worker["worker_pid"] = os.getpid()
        live_worker["worker_start_ticks"] = process_start_ticks(os.getpid())
        assert active_state_is_reusable(live_worker)
        stale_worker = dict(live_worker)
        stale_worker["worker_start_ticks"] = int(live_worker["worker_start_ticks"]) + 1
        assert not active_state_is_reusable(stale_worker)
        missing_qr_browser = dict(live_worker)
        missing_qr_browser["evidence"] = dict(live_worker["evidence"], qr_ready=True)
        assert not active_state_is_reusable(missing_qr_browser)
        cancelled = command_cancel(job_id)
        assert cancelled["status"] == "cancelled"
        assert command_status(job_id)["status"] == "cancelled"

        fake_qr_job = "b" * 24
        fake_qr = new_state(fake_qr_job, 60, "selftest", None)
        fake_qr["evidence"]["qr_ready"] = True
        create_state(root, fake_qr)
        assert wait_until_qr_ready(root, fake_qr_job)["status"] == "failed"

        stale_launch_job = "c" * 24
        stale_launch = new_state(stale_launch_job, 60, "selftest", None)
        stale_launch["created_at"] -= START_LAUNCH_GRACE_SECONDS + 1
        create_state(root, stale_launch)
        assert wait_until_qr_ready(root, stale_launch_job)["status"] == "failed"
        run_worker_launch_lock_self_test(root)
        run_authorized_status_self_test(root)
    if prior is None:
        os.environ.pop("XDG_RUNTIME_DIR", None)
    else:
        os.environ["XDG_RUNTIME_DIR"] = prior
    return {"ok": True}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="QQ Music WeChat browser authorization broker"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    start = subparsers.add_parser("start")
    start.add_argument("--ttl-seconds", type=int, default=DEFAULT_TTL_SECONDS)
    for command in ("status", "cancel", "_worker"):
        item = subparsers.add_parser(command)
        item.add_argument("--job-id", required=True)
    subparsers.add_parser("_self-test")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        if args.command == "start":
            result = command_start(args.ttl_seconds)
        elif args.command == "status":
            result = command_status(args.job_id)
        elif args.command == "cancel":
            result = command_cancel(args.job_id)
        elif args.command == "_worker":
            return run_worker(validate_job_id(args.job_id))
        else:
            result = run_self_test()
    except Exception:
        # Public errors deliberately expose no host data, URLs, cookies or process details.
        result = {
            "job_id": getattr(args, "job_id", ""),
            "status": "failed",
            "evidence": {key: False for key in EVIDENCE_KEYS},
        }
    print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
    if args.command == "cancel":
        return 0
    return 0 if result.get("status") != "failed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
