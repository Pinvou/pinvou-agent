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
  sha256 of "<from_session>|<kind>|<task_id>|<idempotency_key>" when a key is
  given (a key requires from_session, so the namespace is never global; same
  namespace rule as send_message_to_session, contract §6: two sessions reusing
  one key cannot clobber each other's pending request; retries overwrite the
  same file) or a random uuid otherwise. Delivery is at-least-once: a success
  marker suppresses a replay, a failure marker lets a retry re-apply, and a
  crash in the watcher between apply and marker can re-apply once (documented
  in the watcher module);
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

# Task ids are AutomationManager-allocated UUIDs, but the watcher feeds them
# back into storage paths, so the same charset/length discipline as session
# ids applies (anti-empty / anti-traversal; `\Z` anchors the true end).
TASK_ID_RE = re.compile(r"^[A-Za-z0-9_-]+\Z")
MAX_TASK_ID_LEN = 128

# Request kinds behind the create/update/delete tools (the spool record's
# `kind` field; the watcher maps each to its domain entry point).
REQUEST_KINDS = ("create", "update", "delete")

# Task field caps (mirrored by the Rust watcher, which re-checks because the
# spool directory is user-writable). The prompt cap matches the messaging
# channel's 32k body cap — a task prompt becomes the recurring user turn.
MAX_NAME_CHARS = 200
MAX_PROMPT_CHARS = 32 * 1024
MAX_MODEL_ID_CHARS = 200
# Bounded like every other field: an oversize-but-valid-shape rrule must be a
# validation error here, not a spool file that the watcher's byte cap turns
# into silent poison.
MAX_RRULE_CHARS = 256
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
            "Create a Pinvou scheduled task (write operation, applied immediately — "
            "there is no per-call confirmation dialog; the Scheduled Tasks panel "
            "and the audit log are the review surface). "
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
                "idempotency_key": {
                    "type": "string",
                    "description": "(optional) Opaque key (max 128 chars) making retries safe: resending with the same key replaces the pending request instead of creating a duplicate task. Requires from_session so the key is scoped to one sender; omit both when the sender is unknown.",
                },
            },
            "required": ["name", "prompt", "rrule"],
        },
    },
    {
        "name": "read_scheduled_task",
        "description": (
            "Read one Pinvou scheduled task's full detail by id, including its prompt "
            "(read-only). Use it to inspect a task before updating it; use "
            "list_scheduled_tasks to discover ids."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task's id (from list_scheduled_tasks).",
                },
            },
            "required": ["task_id"],
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
    {
        "name": "update_scheduled_task",
        "description": (
            "Update an existing Pinvou scheduled task by id (write operation, applied "
            "immediately — there is no per-call confirmation dialog; the Scheduled "
            "Tasks panel and the audit log are the review surface). Provide only the "
            "fields to change: name, prompt, rrule (same product subset as "
            "create_scheduled_task), model_id, or paused. Read the task first when "
            "the user asks to change a task they describe by name but you only have "
            "list data. Returns the updated task's id and name once the app confirms, "
            "or delivery:'pending' when confirmation has not landed within a few "
            "seconds."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task's id to update (from list_scheduled_tasks/read_scheduled_task).",
                },
                "name": {
                    "type": "string",
                    "description": "(optional) New task name (max 200 chars).",
                },
                "prompt": {
                    "type": "string",
                    "description": "(optional) New per-run instruction (max 32k chars).",
                },
                "rrule": {
                    "type": "string",
                    "description": "(optional) New recurrence, same product subset as create (HOURLY/WEEKLY/ONCE).",
                },
                "model_id": {
                    "type": "string",
                    "description": "(optional) New saved-model id to pin every run to.",
                },
                "paused": {
                    "type": "boolean",
                    "description": "(optional) true pauses the task, false resumes it.",
                },
                "from_session": {
                    "type": "string",
                    "description": "(optional) Your own session's sessionId, for the audit trail.",
                },
                "idempotency_key": {
                    "type": "string",
                    "description": "(optional) Opaque key (max 128 chars) making retries safe.",
                },
            },
            "required": ["task_id"],
        },
    },
    {
        "name": "delete_scheduled_task",
        "description": (
            "Permanently delete a Pinvou scheduled task by id and archive its run "
            "history (destructive, applied immediately). The task stops scheduling "
            "immediately and is removed from the Scheduled Tasks panel. Confirm with "
            "the user before calling; runs already in flight are cancelled. Returns "
            "the deleted task's id and name once the app confirms."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task's id to delete.",
                },
                "from_session": {
                    "type": "string",
                    "description": "(optional) Your own session's sessionId, for the audit trail.",
                },
                "idempotency_key": {
                    "type": "string",
                    "description": "(optional) Opaque key (max 128 chars) making retries safe.",
                },
            },
            "required": ["task_id"],
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


# NOTE: the small request/response helpers in this file (full_tool_name,
# load_tool_features, _coerce_int, _parse_bool_arg, _send, _result, _error,
# _text_content) are byte-identical to session-reader/server.py's. They are
# deliberately kept per-package self-contained so each builtin server stays
# independently reviewable and shippable; extract a shared _builtin_common.py
# (shipped via McpPackageSpec.files) only if a third server repeats them.
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
        key = key.strip()
        if key in parts:
            # Duplicate keys are ambiguous (this dict is last-wins while the
            # Rust watcher's find is first-wins): reject instead of silently
            # picking one side's schedule.
            return None, "invalid rrule: duplicate field '%s'" % key
        parts[key] = value.strip()
    return parts, None


def _parse_int_value(key, value, minimum, maximum):
    text = str(value)
    # ASCII digits only: Python's int() also accepts full-width digits
    # ('１０') and PEP 515 underscores ('1_0'), which the Rust watcher's u32
    # parse rejects — such a record would validate here and poison there.
    if not (text.isascii() and text.isdigit()):
        return None, "invalid rrule: %s must be an integer, got '%s'" % (key, text[:32])
    number = int(text)
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


def validate_task_id(task_id):
    """Charset + length validation for a target task id (update/delete); the
    id flows back into storage paths on the watcher side, so anti-traversal
    rules apply here too."""
    if not task_id or len(task_id) > MAX_TASK_ID_LEN or not TASK_ID_RE.match(task_id):
        return "invalid task_id: %r" % (
            task_id[:64] + "..." if len(task_id) > 64 else task_id,)
    return None


def read_scheduled_task(automations_dir, task_id):
    """Reads one task's full detail from the AutomationManager store
    (read-only). Unlike list_scheduled_tasks this projects the prompt: the
    caller addresses a specific task the user owns, and the prompt is the
    main thing to inspect before an update. Returns (payload, error)."""
    task_id = str(task_id or "").strip()
    id_error = validate_task_id(task_id)
    if id_error:
        return None, id_error
    path = os.path.join(automations_dir, "automations", "%s.json" % task_id)
    try:
        with open(path, "r", encoding="utf-8") as handle:
            record = json.load(handle)
    except (OSError, ValueError, RecursionError):
        # Deliberately no raw exception text: OSError messages embed absolute
        # local paths, which must not leak into tool responses.
        return None, "not found: no scheduled task with id %s" % task_id
    if not isinstance(record, dict):
        return None, "not found: no scheduled task with id %s" % task_id
    status = record.get("status")
    return {
        "id": record.get("id") if isinstance(record.get("id"), str) else task_id,
        "name": str(record.get("name") or ""),
        "prompt": str(record.get("prompt") or ""),
        "rrule": str(record.get("rrule") or ""),
        "status": str(status) if isinstance(status, str) else "",
        "nextRunAt": (
            str(record.get("next_run_at"))
            if isinstance(record.get("next_run_at"), str) else None
        ),
        "lastRunAt": (
            str(record.get("last_run_at"))
            if isinstance(record.get("last_run_at"), str) else None
        ),
        "model": (
            str(record.get("model"))
            if isinstance(record.get("model"), str) else None
        ),
        "createdAt": (
            str(record.get("created_at"))
            if isinstance(record.get("created_at"), str) else None
        ),
        "updatedAt": (
            str(record.get("updated_at"))
            if isinstance(record.get("updated_at"), str) else None
        ),
    }, None


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
                   from_session, idempotency_key, kind="create",
                   task_id=None):
    """Spool record shape — the app-side Rust watcher
    (features/scheduled/creation_requests.rs) re-validates this schema before
    touching anything; additive fields only (contract §4.4). `kind` selects
    the watcher operation ("create" | "update" | "delete"; create records
    written before the field existed stay valid via the watcher's default).
    `task_id` is the target task for update/delete, None for create."""
    return {
        "schema_version": 1,
        "kind": kind,
        "id": spool_id,
        "task_id": task_id,
        "name": name,
        "prompt": prompt,
        "rrule": rrule,
        "model_id": model_id,
        "paused": paused,
        "from_session": from_session,
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
                          paused=False, from_session=None,
                          idempotency_key=None, automations_dir=None):
    """Validates a creation request and spools it for the app-side watcher,
    then waits briefly for the creation result marker. Returns
    (payload, error); nothing else is written.

    Idempotency (contract §6): with an idempotency_key the spool file name is
    the sha256 of "<from_session>|<kind>|<task_id>|<key>", so a retried call
    replaces its own pending request and can never clobber another session's
    (nor a different operation that happens to reuse the key).
    """
    return schedule_task_request(
        requests_dir,
        "create",
        automations_dir=automations_dir,
        name=name,
        prompt=prompt,
        rrule=rrule,
        model_id=model_id,
        paused=paused,
        from_session=from_session,
        idempotency_key=idempotency_key,
    )


def schedule_task_request(requests_dir, kind, automations_dir=None, name=None,
                          prompt=None, rrule=None, model_id=None, paused=None,
                          task_id=None, from_session=None,
                          idempotency_key=None):
    """Kind-aware request path behind the create/update/delete tools: validate
    → spool → wait for the watcher's result marker. Returns (payload, error);
    nothing else is written. `paused` is tri-state here (None = not provided,
    meaningful for update); create callers pass False explicitly."""
    kind = str(kind or "").strip()
    if kind not in REQUEST_KINDS:
        return None, "invalid request kind: %s" % kind[:32]
    name = str(name).strip() if name is not None else None
    prompt = str(prompt).strip() if prompt is not None else None
    rrule = str(rrule).strip() if rrule is not None else None
    model_id = str(model_id or "").strip() or None
    from_session = str(from_session or "").strip() or None
    idempotency_key = str(idempotency_key or "").strip() or None
    if paused is not None:
        paused = _parse_bool_arg(paused, default=None)
        if paused is None:
            return None, "invalid paused: must be a boolean"

    if kind == "create":
        if task_id is not None:
            return None, "invalid task_id: create requests must not target an existing task"
        if not name:
            return None, "invalid name: the task name is empty"
        if not prompt:
            return None, "invalid prompt: the task prompt is empty"
        if not rrule:
            return None, "invalid rrule: the recurrence is empty"
    elif kind in ("update", "delete"):
        task_id = str(task_id or "").strip()
        id_error = validate_task_id(task_id)
        if id_error:
            return None, id_error
        if kind == "update":
            if all(field is None for field in (name, prompt, rrule, model_id, paused)):
                return None, (
                    "invalid update: provide at least one field to change "
                    "(name, prompt, rrule, model_id, paused)"
                )
        else:
            for field_name, field in (("name", name), ("prompt", prompt),
                                      ("rrule", rrule), ("model_id", model_id)):
                if field:
                    return None, (
                        "invalid %s: delete takes no extra fields" % field_name
                    )
            if paused is not None:
                return None, "invalid paused: delete takes no extra fields"

    if name is not None:
        if not name:
            return None, "invalid name: the task name is empty"
        if len(name) > MAX_NAME_CHARS:
            return None, "invalid name: exceeds the %d character limit" % MAX_NAME_CHARS
    if prompt is not None:
        if not prompt:
            return None, "invalid prompt: the task prompt is empty"
        if len(prompt) > MAX_PROMPT_CHARS:
            return None, "invalid prompt: exceeds the %d character limit" % MAX_PROMPT_CHARS
    if model_id is not None and len(model_id) > MAX_MODEL_ID_CHARS:
        return None, "invalid model_id: exceeds %d characters" % MAX_MODEL_ID_CHARS
    if idempotency_key is not None and len(idempotency_key) > MAX_IDEMPOTENCY_KEY_CHARS:
        return None, "invalid idempotency_key: exceeds %d characters" % MAX_IDEMPOTENCY_KEY_CHARS
    if idempotency_key is not None and from_session is None:
        # Without a sender the key's namespace would degrade to global: two
        # unattributed senders reusing one key would clobber each other's
        # pending request.
        return None, (
            "invalid idempotency_key: requires from_session so the key is "
            "scoped to one sender; omit idempotency_key when the sender is unknown"
        )
    if from_session is not None:
        error = validate_sender_session_id(from_session)
        if error:
            return None, error
    if rrule is not None:
        if len(rrule) > MAX_RRULE_CHARS:
            return None, "invalid rrule: exceeds the %d character limit" % MAX_RRULE_CHARS
        rrule, error = validate_rrule(rrule)
        if error:
            return None, error

    # Fail fast on an unknown target instead of spooling into a guaranteed
    # failure (the watcher re-checks against the live store anyway).
    if kind in ("update", "delete") and automations_dir is not None:
        target = os.path.join(automations_dir, "automations", "%s.json" % task_id)
        try:
            if not os.path.isfile(target):
                return None, "not found: no scheduled task with id %s" % task_id
        except OSError:
            return None, "scheduled task store is not readable"

    spool_dir = os.path.join(requests_dir, "spool")
    try:
        os.makedirs(spool_dir, exist_ok=True)
    except OSError:
        # Deliberately no raw OSError text: it embeds absolute host paths.
        return None, "task request queue is not writable"
    if idempotency_key is not None:
        # from_session is guaranteed non-None here by the validation above.
        spool_id = hashlib.sha256(
            ("%s|%s|%s|%s" % (from_session, kind, task_id or "",
                              idempotency_key)).encode("utf-8")
        ).hexdigest()
    else:
        spool_id = uuid.uuid4().hex
    target = os.path.join(spool_dir, "%s.json" % spool_id)
    done_marker = os.path.join(spool_dir, ".done", "%s.json" % spool_id)
    # A pre-existing spool file (still queued / retrying) or a pre-existing
    # result marker (already completed) both mean this key was seen before:
    # say so instead of reporting a fresh apply.
    duplicate = os.path.exists(target) or os.path.exists(done_marker)
    payload = _spool_payload(spool_id, name, prompt, rrule, model_id, paused,
                             from_session, idempotency_key,
                             kind=kind, task_id=task_id if kind != "create" else None)
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

    deadline = time.monotonic() + RESULT_WAIT_SECONDS
    while time.monotonic() < deadline:
        marker, marker_error = _read_result_marker(done_marker)
        if marker is not None:
            if marker.get("ok"):
                result = {
                    "ok": True,
                    "kind": kind,
                    "taskId": marker.get("task_id"),
                    "taskName": marker.get("task_name") or name or task_id,
                    "duplicate": duplicate,
                }
                if kind == "create":
                    result["note"] = (
                        "The scheduled task has been created and is visible in the "
                        "Scheduled Tasks panel."
                        if not duplicate else
                        "A request with the same idempotency key was already "
                        "processed; returning its recorded result — no second task "
                        "was created."
                    )
                elif kind == "update":
                    result["note"] = (
                        "The scheduled task has been updated and the change is "
                        "visible in the Scheduled Tasks panel."
                        if not duplicate else
                        "A request with the same idempotency key was already "
                        "processed; returning its recorded result."
                    )
                else:
                    result["deleted"] = True
                    result["note"] = (
                        "The scheduled task has been deleted; its run history is "
                        "archived and it no longer schedules."
                        if not duplicate else
                        "A request with the same idempotency key was already "
                        "processed; returning its recorded result."
                    )
                return result, None
            return None, str(marker.get("error") or "scheduled task request failed")
        # marker_error only means "not there yet / unreadable" — keep polling.
        time.sleep(RESULT_POLL_INTERVAL_SECONDS)
    pending = {
        "ok": True,
        "kind": kind,
        "taskId": None,
        "taskName": name or task_id,
        "delivery": "pending",
        "duplicate": duplicate,
    }
    if kind == "create":
        pending["note"] = (
            "The creation request is queued; the app has not confirmed the result "
            "within a few seconds. Tell the user the task '%s' is being created and "
            "they can check the Scheduled Tasks panel." % name
        )
    elif kind == "update":
        pending["note"] = (
            "The update request is queued; the app has not confirmed the result "
            "within a few seconds. Tell the user the change to '%s' is being "
            "applied and they can check the Scheduled Tasks panel." % (name or task_id)
        )
    else:
        pending["note"] = (
            "The delete request is queued; the app has not confirmed the result "
            "within a few seconds. Tell the user '%s' is being deleted and they "
            "can check the Scheduled Tasks panel." % task_id
        )
    return pending, None


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
            idempotency_key=args.get("idempotency_key"),
            automations_dir=automations_dir,
        )
    elif name == "read_scheduled_task":
        payload, error = read_scheduled_task(
            automations_dir,
            task_id=args.get("task_id"),
        )
    elif name in ("update_scheduled_task", "delete_scheduled_task"):
        payload, error = schedule_task_request(
            requests_dir,
            "update" if name == "update_scheduled_task" else "delete",
            automations_dir=automations_dir,
            name=args.get("name"),
            prompt=args.get("prompt"),
            rrule=args.get("rrule"),
            model_id=args.get("model_id"),
            paused=args.get("paused"),
            task_id=args.get("task_id"),
            from_session=args.get("from_session"),
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
            "serverInfo": {"name": "pinvou3-app-automations", "version": "1.1.0"},
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
