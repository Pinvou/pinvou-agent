"""Run a Python MCP server with its managed dependency directory.

The parent command invokes this file with ``-I -S -B`` so ambient user-site,
PYTHONPATH, sitecustomize code, and bytecode writes cannot participate in the
managed server runtime.
"""

from __future__ import annotations

import runpy
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) < 3:
        raise SystemExit("usage: python_dependency_runner.py <site-packages> <server.py> [args...]")

    site_packages = sys.argv[1]
    server_script = sys.argv[2]
    # The verified lock wins module-name collisions. The server directory stays
    # second so local sibling modules remain importable without letting an
    # unverified sibling shadow a managed dependency.
    server_directory = str(Path(server_script).resolve().parent)
    sys.path[:0] = [site_packages, server_directory]
    sys.argv = [server_script, *sys.argv[3:]]
    runpy.run_path(server_script, run_name="__main__")


if __name__ == "__main__":
    main()
