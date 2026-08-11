"""Run a Python MCP server with its managed dependency directory."""

from __future__ import annotations

import runpy
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) < 3:
        raise SystemExit("usage: python_dependency_runner.py <site-packages> <server.py> [args...]")

    site_packages = sys.argv[1]
    server_script = sys.argv[2]
    # Match `python server.py`: local sibling modules remain importable, while the
    # managed dependency directory is available without mutating global Python.
    server_directory = str(Path(server_script).resolve().parent)
    sys.path[:0] = [server_directory, site_packages]
    sys.argv = [server_script, *sys.argv[3:]]
    runpy.run_path(server_script, run_name="__main__")


if __name__ == "__main__":
    main()
