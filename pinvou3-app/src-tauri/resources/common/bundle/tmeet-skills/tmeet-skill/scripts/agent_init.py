#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Initialize tmeet CLI agent.json with the calling AI-Agent and LLM model info.

Usage:
    python info_init.py --agent <agent-name> --model <model-name>

Behavior:
    1. Resolve config dir from env TMEET_CLI_CONFIG_DIR; fallback to ~/.tmeet/.
    2. Ensure the directory exists (permission 0700).
    3. Atomically write <config_dir>/agent.json in the AgentConfig JSON shape:
           {"agent": "...", "model": "..."}
       Empty fields are omitted (mirrors Go's `omitempty`).
"""

import argparse
import json
import os
import sys
import tempfile
from pathlib import Path


CONFIG_DIR_ENV = "TMEET_CLI_CONFIG_DIR"
DEFAULT_CONFIG_DIR = "~/.tmeet"
AGENT_CONFIG_FILE = "agent.json"


def get_config_dir() -> Path:
    """Return the tmeet config directory.

    Priority:
        1. Env var TMEET_CLI_CONFIG_DIR (if non-empty).
        2. ~/.tmeet/
    """
    env_dir = os.environ.get(CONFIG_DIR_ENV, "").strip()
    if env_dir:
        return Path(env_dir).expanduser()
    return Path(DEFAULT_CONFIG_DIR).expanduser()


def save_agent_config(config_dir: Path, agent: str, model: str) -> Path:
    """Atomically write AgentConfig to <config_dir>/agent.json.

    Uses tmp-file + os.replace to mirror the Go implementation's atomic write.
    Empty fields are omitted from the JSON output.
    """
    config_dir.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(config_dir, 0o700)
    except OSError:
        # Best-effort; ignore on filesystems that do not support chmod (e.g. Windows).
        pass

    payload: dict = {}
    if agent:
        payload["agent"] = agent
    if model:
        payload["model"] = model

    data = json.dumps(payload, indent=2, ensure_ascii=False)

    target = config_dir / AGENT_CONFIG_FILE

    # Write to a temp file in the same directory, then atomically rename.
    fd, tmp_path = tempfile.mkstemp(
        prefix="." + AGENT_CONFIG_FILE + "-",
        suffix=".tmp",
        dir=str(config_dir),
    )
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        try:
            os.chmod(tmp_path, 0o600)
        except OSError:
            pass
        os.replace(tmp_path, target)
    except Exception:
        # Clean up temp file on failure.
        try:
            os.remove(tmp_path)
        except OSError:
            pass
        raise

    return target


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Write tmeet AgentConfig (agent.json).",
    )
    parser.add_argument(
        "--agent",
        default="",
        help="AI-Agent name (e.g. Cursor, Claude Desktop, Cline).",
    )
    parser.add_argument(
        "--model",
        default="",
        help="LLM model name (e.g. Claude 3.5 Sonnet, GPT-4o).",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    config_dir = get_config_dir()
    try:
        path = save_agent_config(config_dir, args.agent.strip(), args.model.strip())
    except OSError as e:
        print(f"failed to save agent config: {e}", file=sys.stderr)
        return 1
    print(f"agent config saved: {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())