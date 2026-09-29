#!/usr/bin/env python3
"""app_automations — scheduled task creation + listing MCP server for pinvou3 (stdlib only, zero third-party dependencies).

Form: a preset marketplace package named app-automations in the plugin center
(tool store), installed by default; at install time the package contents are
released to ~/.pinvou3/bundles/app-automations/mcp/ and this script is launched
with that directory as cwd.

Goal (docs/app-automations-定时任务创建工具-设计与验收.md): in ANY session the
model can create a Pinvou scheduled task directly, landing in the exact same
store the Scheduled Tasks panel uses (ScheduledTaskState::create_task →
~/.pinvou3/automations, forced YOLO, per-task workspace, model sidecar).

Create semantics (create_scheduled_task, contract §5 L1):
- This server NEVER writes the automation store (the panel and the scheduler
  own it; concurrent whole-file writers would clobber each other). It
  validates the request and spools it:
  ~/.pinvou3/task-requests/spool/<name>.json — the name is the sender-scoped
  sha256 of "<from_session>|<idempotency_key>" when a key is given (same
  namespace rule as send_message_to_session, contract §6: two sessions reusing
  one key cannot clobber each other's pending request; retries overwrite the
  same file, so a retried tool call cannot duplicate a task) or a random uuid
  otherwise;
- An app-side Rust watcher (features/scheduled/creation_requests.rs) drains the
  spool, re-validates it (the spool directory is user-writable, server-side
  checks are not trusted), and creates the task through the panel's own domain
  function. On success it writes a result marker
  spool/.done/<spool-id>.json = {"ok":true,"task_id","task_name"}, deletes the
  spool file, appends the audit record, and emits the panel refresh event;
- Short synchronous wait: after spooling, this process polls the result marker
  for up to RESULT_WAIT_SECONDS. Marker hit → the task ids are returned so the
  model can tell the user "created X" and the UI can link it; timeout → an
  explicit delivery:"pending" payload (NOT an error) — the watcher may be
  busy/paused and the request is still queued.

rrule scope (deliberately stricter than the domain layer): only the product
subset FREQ=HOURLY (INTERVAL hours) / FREQ=WEEKLY (BYDAY+BYHOUR+BYMINUTE) /
FREQ=ONCE (local AT) is accepted; CRON and everything minute-granular are
rejected (same product constraint as SCHEDULED_TASK_CHAT_PROMPT in
features/scheduled/tasks.rs). The domain parser (CodeWhale
automation_manager::parse_rrule) additionally supports CRON — that is
intentionally unreachable through this tool.

list semantics (list_scheduled_tasks, L0): reads only
~/.pinvou3/automations/automations/<id>.json (the AutomationManager store,
one file per task) and projects id/name/rrule/status/nextRunAt/model. The
prompt is NEVER included — the list is for de-duplication before creating,
not for exporting task bodies. Corrupt/torn files (concurrent watcher/panel
writes) are skipped, never fatal.

Storage format source of truth: AutomationRecord in CodeWhale
crates/tui/src/automation_manager.rs (verified 2026-09; drift defense:
unknown fields are ignored by projection, unparseable files are skipped).

Protocol: newline-delimited JSON-RPC 2.0 over stdio (aligned with the
foundation mcp.rs stdio transport: one JSON message per line + '\n', read via
read_line). protocolVersion 2024-11-05.

Directory resolution: --task-requests-dir / --automations-dir explicit
overrides (mainly for tests) > PINVOU3_HOME env var (dev/test fallback) >
~/.pinvou3/... Production never sets PINVOU3_HOME and falls back to HOME; the
foundation's child_env sanitize passes HOME/USERPROFILE through but not
PINVOU3_HOME, so the test/dev-side relocation never affects engine-spawned
instances — which is exactly why the explicit override arguments exist.

Feature-switch fallback (docs/builtin-toolset-contract.md §3.3): once a
feature-level switch turns a feature off, its dedicated tools are removed from
the registry and the model cannot see them; but stale contexts (old sessions
resumed after the contract was injected before the switch-off) can still send
tool calls, which must then get a structured feature_disabled error naming the
alternative action, not a generic not_found. Tool↔feature is many-to-many
union semantics: the sibling manifest.json's tool_features maps full tool
names (mcp_<server>_<tool>) to the features they serve, and a tool counts as
disabled only when **all** listed features appear in the disabled_features of
~/.pinvou3/marketplace/builtin_features.json; tools with no registered
features (mapping missing/empty) are not gated. The state file is re-read on
every call (switches can change at runtime while this process is long-lived);
the manifest is read once at startup (ships with the package, immutable at
runtime); a missing/corrupt state file tolerantly means "all enabled". The
engine's env sanitize does not pass custom env vars through, so switch state
can only be read from the file.
"""
import argparse
import datetime
import hashlib
import io
import json
import os
import re
import sys
import tempfile
import time
import uuid
from pathlib import Path

# Windows defaults stdout to GBK; the MCP protocol requires UTF-8. stdin is
# intentionally NOT rewrapped: the main loop reads sys.stdin.buffer as raw
# bytes and decodes tolerantly so a single non-UTF-8 byte cannot kill the
# process (errors="replace" below).
if sys.platform == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")

PROTOCOL_VERSION = "2024-11-05"

# Same rules as validate_session_id in
# pinvou3-app/src-tauri/src/features/sessions/validators.rs:
# [A-Za-z0-9_-]+, anti-empty / anti-traversal. `\Z` (not `$`) anchors at the
# true end of the string, matching the Rust validator (a trailing "\n" must
# not pass).
SESSION_ID_RE = re.compile(r"^[A-Za-z0-9_-]+\Z")

MAX_SESSION_ID_LEN = 128
MAX_SENDER_TITLE_CHARS = 200

# Task field caps (mirrored by the Rust watcher, which re-checks because the
# spool directory is user-writable). The prompt cap matches the messaging
# channel's 32k body cap — a task prompt becomes the recurring user turn.
MAX_NAME_CHARS = 200
MAX_PROMPT_CHARS = 32 * 1024
MAX_MODEL_ID_CHARS = 200
MAX_IDEMPOTENCY_KEY_CHARS = 128

DEFAULT_LIST_LIMIT = 20
MAX_LIST_LIMIT = 100

# Short synchronous wait: after spooling, poll for the watcher's result
# marker. Long enough to cover a normal create (a local JSON write), short
# enough that a dead watcher cannot stall the model's turn.
RESULT_WAIT_SECONDS = 5.0
RESULT_POLL_INTERVAL_SECONDS = 0.2

# Server key (identical to the marketplace package id), used to compose the
# registry full tool name mcp_<server>_<tool>.
SERVER_KEY = "app-automations"

TOOL_DEFS = [
    {
        "name": "create_scheduled_task",
        "description": (
            "Create a Pinvou scheduled task (write operation, requires user approval). "
            "Use this when the user asks for a recurring or one-shot automated job, e.g. "
            "'run an AI news digest every day at 8:30' or 'remind me once on June 1st at 9:30'. "
            "Each run starts its own conversation in a task-dedicated workspace. Call "
            "list_scheduled_tasks first to avoid duplicating an existing task. rrule must use "
            "the product subset only: FREQ=HOURLY;INTERVAL=N;BYHOUR=h;BYMINUTE=m (every N hours), "
            "FREQ=WEEKLY;BYDAY=MO,...;BYHOUR=h;BYMINUTE=m (weekly on given days; all seven days "
            "for daily), or FREQ=ONCE;AT=YYYY-MM-DDTHH:MM (local time, future, no timezone "
            "suffix, runs once then pauses). Minute-granular frequencies (every N minutes) and "
            "CRON are NOT supported — offer the user every-N-hours/daily/weekly instead. "
            "Returns taskId/taskName once the app confirms creation, or delivery:'pending' "
            "when confirmation has not landed within a few seconds (the task is still queued)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short human-readable task name shown in the Scheduled Tasks panel (max 200 chars).",
                },
                "prompt": {
                    "type": "string",
                    "description": "What each run should do — written as the recurring instruction for that run's model (max 32k chars).",
                },
                "rrule": {
                    "type": "string",
                    "description": (
                        "Recurrence, product subset only: 'FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30' "
                        "(every 6h from 08:30), 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30' "
                        "(daily 08:30), 'FREQ=ONCE;AT=2027-06-01T09:30' (one-shot, local time)."
                    ),
                },
                "model_id": {
                    "type": "string",
                    "description": "(optional) Exact saved-model id from this app's model settings to pin every run to. Omit to use the app's fallback model.",
                },
                "paused": {
                    "type": "boolean",
                    "description": "(optional) Create the task paused (scheduled but not active); default false.",
                },
                "from_session": {
                    "type": "string",
                    "description": "(optional) Your own session's sessionId, for the creation audit trail. Omit if unknown.",
                },
                "from_title": {
                    "type": "string",
                    "description": "(optional) Your own session's title, for the audit trail. Omit if unknown.",
                },
                "idempotency_key": {
                    "type": "string",
                    "description": "(optional) Opaque key (max 128 chars) making retries safe: resending with the same key replaces the pending request instead of creating a duplicate task.",
                },
            },
            "required": ["name", "prompt", "rrule"],
        },
    },
    {
        "name": "list_scheduled_tasks",
        "description": (
            "List the user's existing Pinvou scheduled tasks (read-only; id, name, rrule, "
            "status, nextRunAt, model). Call this before create_scheduled_task to avoid "
            "duplicating a task the user already has. Task prompts are never included."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "(optional) Max entries to return, default 20, max 100.",
                },
            },
        },
    },
]

# ---------------------------------------------------------------------------
# Pure-function area (kept separate from the stdio protocol layer;
# scripts/tests/test_app_automations_server.py tests these directly)
# ---------------------------------------------------------------------------


def resolve_requests_dir(argv=None):
    """--task-requests-dir > PINVOU3_HOME/task-requests > ~/.pinvou3/task-requests (create spool root; tests use the explicit override)."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--task-requests-dir", default=None)
    args, _ = parser.parse_known_args(argv)
    if args.task_requests_dir:
        return args.task_requests_dir
    home = os.environ.get("PINVOU3_HOME")
    if home:
        return os.path.join(home, "task-requests")
    return os.path.join(os.path.expanduser("~"), ".pinvou3", "task-requests")


def resolve_automations_dir(argv=None):
    """--automations-dir > PINVOU3_HOME/automations > ~/.pinvou3/automations (list source: the AutomationManager store root)."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--automations-dir", default=None)
    args, _ = parser.parse_known_args(argv)
    if args.automations_dir:
        return args.automations_dir
    home = os.environ.get("PINVOU3_HOME")
    if home:
        return os.path.join(home, "automations")
    return os.path.join(os.path.expanduser("~"), ".pinvou3", "automations")


def full_tool_name(tool_name):
    """Local tool name -> registry full name mcp_<server>_<tool> (matches the Rust-side registration convention)."""
    return "mcp_%s_%s" % (SERVER_KEY, tool_name)


def load_tool_features(manifest_path=None):
    """Reads tool_features (full tool name -> feature list) from manifest.json.

    A missing/corrupt file or missing field all yield {} (no feature gating).
    Reading once at startup is enough — the manifest ships with the package
    and never changes at runtime.
    """
    path = (Path(manifest_path) if manifest_path
            else Path(__file__).with_name("manifest.json"))
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    mapping = data.get("tool_features") if isinstance(data, dict) else None
    if not isinstance(mapping, dict):
        return {}
    return {
        str(name): [str(feature) for feature in features]
        for name, features in mapping.items()
        if isinstance(features, list)
    }


def load_disabled_features(base_dir):
    """Reads disabled_features from builtin_features.json and returns a set.

    Missing/corrupt file = all enabled (tolerant; never rejects calls because
    of it). The state file is located relative to the base directory:
    <pinvou3_home>/marketplace/builtin_features.json (base_dir is
    task-requests, whose parent is the pinvou3 home — the same derivation the
    session-reader server applies to its sessions dir). It must be re-read on
    every call — switches can change at runtime while this process is
    long-lived.
    """
    path = Path(base_dir).parent / "marketplace" / "builtin_features.json"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return set()
    disabled = data.get("disabled_features") if isinstance(data, dict) else None
    if not isinstance(disabled, list):
        return set()
    return {str(feature) for feature in disabled}


def feature_gate_error(tool_name, base_dir, tool_features):
    """Contract §3.3 fallback: returns a feature_disabled payload when every feature the tool depends on is off, else None.

    Many-to-many union semantics: a tool counts as disabled only when the
    whole feature list mapped in tool_features appears in disabled_features;
    tools with no registered features (mapping missing/empty) are not gated.
    """
    features = tool_features.get(full_tool_name(tool_name))
    if not features:
        return None
    disabled = load_disabled_features(base_dir)
    if not all(feature in disabled for feature in features):
        return None
    names = ", ".join(features)
    return {
        "ok": False,
        "code": "feature_disabled",
        "error": (
            "This tool is unavailable: the feature(s) %s are disabled, and this "
            "tool requires all of them to be enabled." % names
        ),
        "alternative": (
            "Tell the user the feature(s) %s are turned off and can be re-enabled "
            "in Settings, or answer using information already available in this "
            "conversation." % names
        ),
    }


def _coerce_int(value, default, minimum, maximum):
    try:
        number = int(value)
    except (TypeError, ValueError):
        return default
    return max(minimum, min(maximum, number))


def _parse_bool_arg(value, default=False):
    """Strict boolean coercion for tool arguments: only a real bool or the
    strings "true"/"false" count — `bool("false")` would silently read as
    True, so anything unrecognized falls back to the default."""
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        lowered = value.strip().lower()
        if lowered == "true":
            return True
        if lowered == "false":
            return False
    return default


def _utc_now_rfc3339():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# --- rrule validation (product subset; stricter than the domain on purpose) ---

WEEKDAY_TOKENS = ("MO", "TU", "WE", "TH", "FR", "SA", "SU")

# ONCE AT: exactly the local wall-clock form the product documents,
# YYYY-MM-DDTHH:MM — no seconds, no 'Z', no UTC offset (B4: a timezone suffix
# or a past moment is rejected with an actionable message).
ONCE_AT_RE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}\Z")

# Isolated session prefixes (contract §4.3/§5, case-insensitive) rejected as
# the audit sender: sched- sessions are subsystem-owned (Scheduled Tasks,
# unattended by design), eval_ is benchmark-private, aux- marks auxiliary
# side-chats.
ISOLATED_SENDER_PREFIXES = ("sched-", "eval_", "aux-")


def normalize_rrule(rrule):
    """trim + uppercase (the domain stores the uppercase form; B8)."""
    return str(rrule or "").strip().upper()


def _parse_rrule_segments(rrule):
    """Splits 'K=V;K=V' into an ordered dict; empty segments are skipped (same
    as the domain parser) and a segment without '=' is a hard error. Returns
    (parts, error)."""
    parts = {}
    for raw in str(rrule).split(";"):
        item = raw.strip()
        if not item:
            continue
        key, sep, value = item.partition("=")
        if not sep:
            return None, "invalid rrule: segment '%s' must look like KEY=VALUE" % item[:64]
        parts[key.strip()] = value.strip()
    return parts, None


def _parse_int_value(key, value, minimum, maximum):
    try:
        number = int(value)
    except (TypeError, ValueError):
        return None, "invalid rrule: %s must be an integer, got '%s'" % (key, value[:32])
    if number < minimum or number > maximum:
        return None, "invalid rrule: %s must be between %d and %d, got %d" % (
            key, minimum, maximum, number)
    return number, None


def _parse_byday(value):
    tokens = [token.strip() for token in str(value).split(",") if token.strip()]
    if not tokens:
        return None, "invalid rrule: BYDAY cannot be empty"
    for token in tokens:
        if token not in WEEKDAY_TOKENS:
            return None, (
                "invalid rrule: BYDAY must be a comma-separated list of %s, got '%s'"
                % ("/".join(WEEKDAY_TOKENS), token[:32])
            )
    return tokens, None


def validate_rrule(rrule):
    """Validates one rrule against the product subset and returns the
    normalized (uppercase) form or an error string.

    Accepts exactly:
    - FREQ=HOURLY  [INTERVAL>=1h] [BYDAY] [BYHOUR 0-23] [BYMINUTE 0-59]
    - FREQ=WEEKLY  BYDAY (required) BYHOUR (required) BYMINUTE (required)
    - FREQ=ONCE    AT=<local YYYY-MM-DDTHH:MM, strictly in the future>
    Rejects CRON and every minute-granular frequency with guidance toward the
    supported shapes (product constraint; the domain parser's CRON support is
    intentionally unreachable here).
    """
    normalized = normalize_rrule(rrule)
    if not normalized:
        return None, "invalid rrule: the recurrence is empty"
    parts, error = _parse_rrule_segments(normalized)
    if error:
        return None, error
    freq = parts.get("FREQ")
    if not freq:
        return None, "invalid rrule: FREQ is required (HOURLY, WEEKLY, or ONCE)"
    if freq == "MINUTELY" or freq == "SECONDLY" or freq == "DAILY":
        return None, (
            "invalid rrule: FREQ=%s is not supported. Supported: HOURLY (every N hours), "
            "WEEKLY (specific weekdays = daily when all seven are listed), and ONCE "
            "(one-shot). Offer the user every-N-hours, a daily/weekly time, or a one-shot "
            "instead of minute-granular schedules" % freq
        )
    if freq == "CRON":
        return None, (
            "invalid rrule: CRON expressions are not supported by this tool. Express the "
            "schedule as FREQ=HOURLY;INTERVAL=N;BYHOUR=h;BYMINUTE=m, "
            "FREQ=WEEKLY;BYDAY=...;BYHOUR=h;BYMINUTE=m, or FREQ=ONCE;AT=YYYY-MM-DDTHH:MM"
        )
    if freq not in ("HOURLY", "WEEKLY", "ONCE"):
        return None, (
            "invalid rrule: unsupported FREQ '%s'. Supported: HOURLY, WEEKLY, ONCE"
            % freq[:32]
        )

    if freq == "HOURLY":
        allowed = {"FREQ", "INTERVAL", "BYDAY", "BYHOUR", "BYMINUTE"}
        unknown = sorted(set(parts) - allowed)
        if unknown:
            return None, "invalid rrule: unsupported field(s) %s for FREQ=HOURLY. Allowed: %s" % (
                ", ".join(unknown), ",".join(sorted(allowed)))
        if "INTERVAL" in parts:
            _, error = _parse_int_value("INTERVAL", parts["INTERVAL"], 1, 24 * 30)
            if error:
                return None, error
        if "BYHOUR" in parts:
            _, error = _parse_int_value("BYHOUR", parts["BYHOUR"], 0, 23)
            if error:
                return None, error
        if "BYMINUTE" in parts:
            _, error = _parse_int_value("BYMINUTE", parts["BYMINUTE"], 0, 59)
            if error:
                return None, error
        if "BYDAY" in parts:
            _, error = _parse_byday(parts["BYDAY"])
            if error:
                return None, error
        return normalized, None

    if freq == "WEEKLY":
        allowed = {"FREQ", "BYDAY", "BYHOUR", "BYMINUTE"}
        unknown = sorted(set(parts) - allowed)
        if unknown:
            return None, "invalid rrule: unsupported field(s) %s for FREQ=WEEKLY. Allowed: %s" % (
                ", ".join(unknown), ",".join(sorted(allowed)))
        if "BYDAY" not in parts:
            return None, "invalid rrule: FREQ=WEEKLY requires BYDAY (e.g. BYDAY=MO,WE)"
        _, error = _parse_byday(parts["BYDAY"])
        if error:
            return None, error
        if "BYHOUR" not in parts:
            return None, "invalid rrule: FREQ=WEEKLY requires BYHOUR (0-23)"
        _, error = _parse_int_value("BYHOUR", parts["BYHOUR"], 0, 23)
        if error:
            return None, error
        if "BYMINUTE" not in parts:
            return None, "invalid rrule: FREQ=WEEKLY requires BYMINUTE (0-59)"
        _, error = _parse_int_value("BYMINUTE", parts["BYMINUTE"], 0, 59)
        if error:
            return None, error
        return normalized, None

    # ONCE
    allowed = {"FREQ", "AT"}
    unknown = sorted(set(parts) - allowed)
    if unknown:
        return None, "invalid rrule: unsupported field(s) %s for FREQ=ONCE. Allowed: FREQ,AT" % (
            ", ".join(unknown))
    raw_at = parts.get("AT")
    if not raw_at:
        return None, "invalid rrule: FREQ=ONCE requires AT (local YYYY-MM-DDTHH:MM, future)"
    if not ONCE_AT_RE.match(raw_at):
        return None, (
            "invalid rrule: ONCE AT must be the local time 'YYYY-MM-DDTHH:MM' without "
            "seconds or a timezone suffix (no 'Z', no '+08:00'), got '%s'" % raw_at[:64]
        )
    try:
        at = datetime.datetime.strptime(raw_at, "%Y-%m-%dT%H:%M")
    except ValueError:
        return None, "invalid rrule: ONCE AT '%s' is not a real calendar minute" % raw_at[:64]
    now = datetime.datetime.now()
    if at <= now:
        return None, (
            "invalid rrule: ONCE AT '%s' is not in the future. Confirm a later local "
            "time with the user instead of creating a task that can never run" % raw_at
        )
    return normalized, None


def validate_sender_session_id(session_id):
    """Charset + length validation for the audit-trail sender id, plus the
    isolated-prefix rejection (contract §4.3/§5, case-insensitive): a sched-
    session is unattended by design and must never be the requester (the
    recursion shield lives in the execpolicy Ask rule; this is defense in
    depth, re-checked by the Rust watcher). Existence is deliberately NOT
    probed here: the field is model-supplied, unauthenticated, and used only
    to locate the audit root."""
    if not session_id or len(session_id) > MAX_SESSION_ID_LEN or not SESSION_ID_RE.match(session_id):
        return "invalid from_session: %r" % (
            session_id[:64] + "..." if len(session_id) > 64 else session_id,)
    if session_id.lower().startswith(ISOLATED_SENDER_PREFIXES):
        return "session %s cannot request task creation" % session_id
    return None


def _spool_payload(spool_id, name, prompt, rrule, model_id, paused,
                   from_session, from_title, idempotency_key):
    """Spool record shape — the app-side Rust watcher
    (features/scheduled/creation_requests.rs) re-validates this schema before
    creating anything; additive fields only (contract §4.4)."""
    return {
        "schema_version": 1,
        "id": spool_id,
        "name": name,
        "prompt": prompt,
        "rrule": rrule,
        "model_id": model_id,
        "paused": paused,
        "from_session": from_session,
        "from_title": from_title,
        "created_at": _utc_now_rfc3339(),
        "idempotency_key": idempotency_key,
    }


def _read_result_marker(path):
    """Reads one .done result marker; returns (marker, error). A corrupt or
    unreadable marker is reported as an error so the caller keeps waiting
    instead of surfacing garbage (a torn marker cannot happen — the watcher
    writes atomically — but a hostile one must not crash the server)."""
    try:
        with open(path, "r", encoding="utf-8") as handle:
            marker = json.load(handle)
    except (OSError, ValueError, RecursionError):
        return None, "creation result marker is unreadable"
    if not isinstance(marker, dict):
        return None, "creation result marker is malformed"
    return marker, None


def create_scheduled_task(requests_dir, name, prompt, rrule, model_id=None,
                          paused=False, from_session=None, from_title=None,
                          idempotency_key=None):
    """Validates a creation request and spools it for the app-side watcher,
    then waits briefly for the creation result marker. Returns
    (payload, error); nothing else is written.

    Idempotency (contract §6): with an idempotency_key the spool file name is
    the sender-scoped sha256 of "<from_session>|<key>", so a retried call
    replaces its own pending request (same-session dedup) and can never
    clobber another session's.
    """
    name = str(name or "").strip()
    prompt = str(prompt or "").strip()
    rrule = str(rrule or "").strip()
    model_id = str(model_id or "").strip() or None
    from_session = str(from_session or "").strip() or None
    from_title = str(from_title or "").strip() or None
    idempotency_key = str(idempotency_key or "").strip() or None
    paused = _parse_bool_arg(paused)

    if not name:
        return None, "invalid name: the task name is empty"
    if len(name) > MAX_NAME_CHARS:
        return None, "invalid name: exceeds the %d character limit" % MAX_NAME_CHARS
    if not prompt:
        return None, "invalid prompt: the task prompt is empty"
    if len(prompt) > MAX_PROMPT_CHARS:
        return None, "invalid prompt: exceeds the %d character limit" % MAX_PROMPT_CHARS
    if model_id is not None and len(model_id) > MAX_MODEL_ID_CHARS:
        return None, "invalid model_id: exceeds %d characters" % MAX_MODEL_ID_CHARS
    if idempotency_key is not None and len(idempotency_key) > MAX_IDEMPOTENCY_KEY_CHARS:
        return None, "invalid idempotency_key: exceeds %d characters" % MAX_IDEMPOTENCY_KEY_CHARS
    if from_session is not None:
        error = validate_sender_session_id(from_session)
        if error:
            return None, error
    if from_title is not None and len(from_title) > MAX_SENDER_TITLE_CHARS:
        return None, "invalid from_title: exceeds %d characters" % MAX_SENDER_TITLE_CHARS
    normalized_rrule, error = validate_rrule(rrule)
    if error:
        return None, error

    spool_dir = os.path.join(requests_dir, "spool")
    try:
        os.makedirs(spool_dir, exist_ok=True)
    except OSError:
        # Deliberately no raw OSError text: it embeds absolute host paths.
        return None, "task request queue is not writable"
    if idempotency_key is not None:
        spool_id = hashlib.sha256(
            ("%s|%s" % (from_session or "", idempotency_key)).encode("utf-8")
        ).hexdigest()
    else:
        spool_id = uuid.uuid4().hex
    target = os.path.join(spool_dir, "%s.json" % spool_id)
    duplicate = os.path.exists(target)
    payload = _spool_payload(spool_id, name, prompt, normalized_rrule, model_id,
                             paused, from_session, from_title, idempotency_key)
    try:
        # Atomic write (tmp + rename): the watcher must never observe a torn file.
        fd, tmp = tempfile.mkstemp(dir=spool_dir, suffix=".tmp")
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                json.dump(payload, handle, ensure_ascii=False)
            os.replace(tmp, target)
        except BaseException:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise
    except OSError:
        return None, "task request queue is not writable"

    done_marker = os.path.join(spool_dir, ".done", "%s.json" % spool_id)
    deadline = time.monotonic() + RESULT_WAIT_SECONDS
    while time.monotonic() < deadline:
        marker, marker_error = _read_result_marker(done_marker)
        if marker is not None:
            if marker.get("ok"):
                return {
                    "ok": True,
                    "taskId": marker.get("task_id"),
                    "taskName": marker.get("task_name") or name,
                    "duplicate": duplicate,
                    "note": (
                        "The scheduled task has been created and is visible in the "
                        "Scheduled Tasks panel."
                        if not duplicate else
                        "A request with the same idempotency key was already queued; "
                        "no second task was created."
                    ),
                }, None
            return None, str(marker.get("error") or "scheduled task creation failed")
        # marker_error only means "not there yet / unreadable" — keep polling.
        time.sleep(RESULT_POLL_INTERVAL_SECONDS)
    return {
        "ok": True,
        "taskId": None,
        "taskName": name,
        "delivery": "pending",
        "duplicate": duplicate,
        "note": (
            "The creation request is queued; the app has not confirmed the result "
            "within a few seconds. Tell the user the task '%s' is being created and "
            "they can check the Scheduled Tasks panel." % name
        ),
    }, None


def list_scheduled_tasks(automations_dir, limit=DEFAULT_LIST_LIMIT):
    """Reads the AutomationManager store and projects the model-safe fields.
    Returns (payload, error). The prompt field is deliberately never projected
    (the list is for de-duplication, not body export). A corrupt/torn file —
    possible while the panel or the watcher writes concurrently — is skipped,
    never fatal (C5)."""
    limit = _coerce_int(limit, DEFAULT_LIST_LIMIT, 1, MAX_LIST_LIMIT)
    tasks_dir = os.path.join(automations_dir, "automations")
    try:
        names = os.listdir(tasks_dir)
    except OSError:
        # No store yet = no tasks have ever been created; an empty answer (not
        # an error) keeps the de-dup flow working on a fresh install.
        return {"tasks": [], "total": 0}, None
    entries = []
    for name in sorted(names):
        if not name.endswith(".json"):
            continue
        path = os.path.join(tasks_dir, name)
        try:
            with open(path, "r", encoding="utf-8") as handle:
                record = json.load(handle)
        except (OSError, ValueError, RecursionError):
            continue
        if not isinstance(record, dict):
            continue
        task_id = record.get("id")
        if not isinstance(task_id, str) or not task_id:
            continue
        status = record.get("status")
        entries.append({
            "id": task_id,
            "name": str(record.get("name") or ""),
            "rrule": str(record.get("rrule") or ""),
            "status": str(status) if isinstance(status, str) else "",
            "nextRunAt": (
                str(record.get("next_run_at"))
                if isinstance(record.get("next_run_at"), str) else None
            ),
            "model": (
                str(record.get("model"))
                if isinstance(record.get("model"), str) else None
            ),
        })
    entries.sort(key=lambda item: item["name"])
    return {"tasks": entries[:limit], "total": len(entries)}, None


# ---------------------------------------------------------------------------
# stdio protocol layer (aligned with session-reader server.py)
# ---------------------------------------------------------------------------


def _send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _result(req_id, result):
    _send({"jsonrpc": "2.0", "id": req_id, "result": result})


def _error(req_id, code, message):
    _send({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}})


def _text_content(payload, is_error=False):
    return {
        "content": [{"type": "text", "text": json.dumps(payload, ensure_ascii=False)}],
        "isError": is_error,
    }


def _handle_call(req_id, params, requests_dir, automations_dir, tool_features):
    name = (params or {}).get("name")
    args = (params or {}).get("arguments") or {}
    # Contract §3.3 fallback: a tool call from stale context gets a structured
    # feature_disabled when all its features are off.
    gate = feature_gate_error(name, requests_dir, tool_features)
    if gate is not None:
        _result(req_id, _text_content(gate, is_error=True))
        return
    if name == "create_scheduled_task":
        payload, error = create_scheduled_task(
            requests_dir,
            name=args.get("name"),
            prompt=args.get("prompt"),
            rrule=args.get("rrule"),
            model_id=args.get("model_id"),
            paused=args.get("paused"),
            from_session=args.get("from_session"),
            from_title=args.get("from_title"),
            idempotency_key=args.get("idempotency_key"),
        )
    elif name == "list_scheduled_tasks":
        payload, error = list_scheduled_tasks(
            automations_dir,
            limit=args.get("limit", DEFAULT_LIST_LIMIT),
        )
    else:
        # Unknown tool name: -32602 (invalid params) — the method itself is
        # tools/call; the tool name is a parameter of it.
        _error(req_id, -32602, "unknown tool: %s" % name)
        return
    if error is not None:
        _result(req_id, _text_content({"ok": False, "error": error}, is_error=True))
    else:
        payload["ok"] = True
        _result(req_id, _text_content(payload))


def _handle(msg, requests_dir, automations_dir, tool_features):
    method = msg.get("method")
    req_id = msg.get("id")

    # Notifications (no id): initialized etc. — never answered.
    if req_id is None:
        return

    if method == "initialize":
        _result(req_id, {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pinvou3-app-automations", "version": "1.0.0"},
        })
    elif method == "ping":
        # MCP convention: keepalive ping answers with an empty result.
        _result(req_id, {})
    elif method == "tools/list":
        _result(req_id, {"tools": TOOL_DEFS})
    elif method == "tools/call":
        _handle_call(req_id, msg.get("params"), requests_dir, automations_dir, tool_features)
    else:
        _error(req_id, -32601, "method not found: %s" % method)


def main():
    requests_dir = resolve_requests_dir()
    automations_dir = resolve_automations_dir()
    tool_features = load_tool_features()
    # Read raw bytes and decode tolerantly: a single non-UTF-8 byte on stdin
    # becomes U+FFFD (the line then fails JSON parsing and is skipped) instead
    # of raising UnicodeDecodeError and killing the long-lived process.
    for raw in sys.stdin.buffer:
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue  # skip the bad line, never crash
        try:
            _handle(msg, requests_dir, automations_dir, tool_features)
        except Exception as e:
            rid = msg.get("id") if isinstance(msg, dict) else None
            if rid is not None:
                # Never interpolate the raw exception: its message can embed
                # absolute host paths (including the username), which must not
                # leak into model context. The exception type name is enough
                # to correlate with server-side logs.
                _error(rid, -32603, "internal error (%s)" % type(e).__name__)


if __name__ == "__main__":
    main()
