#!/usr/bin/env python3
"""Start private display/model relay inside the already isolated namespace."""

from pathlib import Path
import os
import socket
import subprocess
import sys
import time


def main():
    runtime = Path("/tmp/runtime")
    runtime.mkdir(mode=0o700)
    Path("/tmp/.X11-unix").mkdir(mode=0o1777)
    env = dict(os.environ, DISPLAY=":99", GDK_BACKEND="x11", GSETTINGS_BACKEND="memory",
               XDG_RUNTIME_DIR=str(runtime), LIBGL_ALWAYS_SOFTWARE="1")
    children = []
    try:
        display = subprocess.Popen(["/opt/pinvou-eval/Xvfb", ":99", "-screen", "0", "1024x768x24",
                                    "-nolisten", "tcp", "-extension", "GLX", "-ac"], env=env,
                                   stdout=subprocess.DEVNULL, stderr=sys.stderr)
        children.append(display)
        relay = subprocess.Popen(["/usr/bin/socat", "TCP-LISTEN:18000,bind=127.0.0.1,reuseaddr,fork",
                                  "UNIX-CONNECT:/run/pinvou/model.sock"],
                                 stdout=subprocess.DEVNULL, stderr=sys.stderr)
        children.append(relay)
        ready = False
        for _ in range(100):
            if display.poll() is not None or relay.poll() is not None:
                break
            if Path("/tmp/.X11-unix/X99").exists():
                try:
                    with socket.create_connection(("127.0.0.1", 18000), timeout=.1):
                        ready = True
                    break
                except OSError:
                    pass
            time.sleep(.02)
        if not ready:
            raise RuntimeError(f"isolated display or model relay failed to initialize: display={display.poll()} relay={relay.poll()}")
        agent = subprocess.Popen(["/opt/pinvou-eval/agent", "/run/pinvou/request.json"], env=env)
        children.append(agent)
        return agent.wait()
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
        for child in reversed(children):
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()


if __name__ == "__main__":
    sys.exit(main())
