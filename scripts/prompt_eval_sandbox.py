"""Linux evaluation namespace: expose one trial, never harness or neighboring data."""

import json
import atexit
from functools import lru_cache
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


VISIBLE_ROOT = "/work/trial"
VISIBLE_AGENT = "/opt/pinvou-eval/agent"


@lru_cache(maxsize=1)
def identity_files():
    directory = Path(tempfile.mkdtemp(prefix="pinvou-namespace-identity-"))
    (directory / "passwd").write_text(f"user:x:{os.getuid()}:{os.getgid()}:User:{Path.home()}:/bin/bash\n")
    (directory / "group").write_text(f"user:x:{os.getgid()}:\n")
    atexit.register(shutil.rmtree, directory)
    return directory


def translate(value, root):
    if isinstance(value, dict):
        return {k: translate(v, root) for k, v in value.items()}
    if isinstance(value, list):
        return [translate(v, root) for v in value]
    if isinstance(value, str) and (value == str(root) or value.startswith(str(root) + "/")):
        return VISIBLE_ROOT + value[len(str(root)):]
    return value


def normalize_event_paths(events, root):
    for event in events:
        path = event.get("input", {}).get("path")
        if isinstance(path, str) and path.startswith(VISIBLE_ROOT + "/"):
            event["input"]["path"] = str(root) + path[len(VISIBLE_ROOT):]
    return events


def base_args():
    bwrap = shutil.which("bwrap")
    if bwrap is None or os.name != "posix":
        raise RuntimeError("this opt-in live suite requires Linux bubblewrap")
    args = [bwrap, "--unshare-all", "--hostname", "pinvou", "--die-with-parent", "--new-session",
            "--clearenv", "--ro-bind", "/usr", "/usr", "--ro-bind", "/lib", "/lib",
            "--symlink", "usr/bin", "/bin", "--symlink", "usr/sbin", "/sbin",
            "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
            "--dir", "/run/pinvou", "--dir", "/opt/pinvou-eval", "--dir", VISIBLE_ROOT,
            "--dir", str(Path.home())]
    # x86_64 ELF interpreters use /lib64 even on usr-merged distributions.
    # Keep the host's loader view without exposing any mutable host data.
    if Path("/lib64").exists():
        args += ["--ro-bind", "/lib64", "/lib64"]
    for path in ["/etc/ssl", "/etc/hosts", "/etc/resolv.conf", "/etc/nsswitch.conf", "/etc/localtime", "/etc/fonts"]:
        if Path(path).exists():
            args += ["--ro-bind", path, path]
    identity = identity_files()
    args += ["--ro-bind", str(identity / "passwd"), "/etc/passwd",
             "--ro-bind", str(identity / "group"), "/etc/group"]
    # No real-home subtree, credentials, D-Bus socket, source checkout, or
    # neighboring trial is mounted. This corpus needs only system/stdlib tools.
    return args


def workspace_mounts(request, root):
    """Expose declared fixture projects, never their controller parent."""
    root = Path(root).resolve(strict=True)
    selected = [f"{VISIBLE_ROOT}/primary"]
    if request.get("workspace"):
        selected.append(request["workspace"])
    additional = request.get("workspace_roots", [])
    if not isinstance(additional, list):
        raise ValueError("workspace_roots must be a list of fixture directories")
    selected.extend(additional)
    mounts = {}
    for visible in selected:
        if not isinstance(visible, str):
            raise ValueError("workspace root must be a string")
        path = Path(visible)
        try:
            relative = path.relative_to(VISIBLE_ROOT)
        except ValueError as error:
            raise ValueError("workspace must stay inside the visible fixture") from error
        if not relative.parts or ".." in relative.parts or relative.parts[0] == "home":
            raise ValueError("workspace cannot expose controller or private runtime state")
        source = root / relative
        if any((root / Path(*relative.parts[:index])).is_symlink()
               for index in range(1, len(relative.parts) + 1)) or not source.is_dir():
            raise ValueError("workspace must be an existing nonsymlink fixture directory")
        mounts[source] = str(path)
    # A selected ancestor already exposes its selected descendants. This also
    # keeps nested selections from hiding a writable project with another bind.
    result = []
    for source, visible in sorted(mounts.items(), key=lambda pair: len(pair[0].parts)):
        if not any(source.is_relative_to(parent) for parent, _ in result):
            result.append((source, visible))
    return result


def command(binary, request_file, root, home, program=None, proxy_socket=None, xvfb=None):
    args = base_args()
    request = json.loads(request_file.read_text())
    for path, visible in workspace_mounts(request, root):
        args += ["--bind", str(path), visible]
    args += ["--bind", str(home), f"{VISIBLE_ROOT}/home",
             "--ro-bind", str(binary), VISIBLE_AGENT,
             "--ro-bind", str(request_file), "/run/pinvou/request.json"]
    if program is None:
        if proxy_socket is None or xvfb is None:
            raise RuntimeError("isolated model execution requires its scoped proxy socket and Xvfb")
        args += ["--ro-bind", str(proxy_socket), "/run/pinvou/model.sock",
                 "--ro-bind", str(xvfb), "/opt/pinvou-eval/Xvfb",
                 "--ro-bind", str(Path(__file__).with_name("prompt_eval_namespace_bootstrap.py")),
                 "/run/pinvou/bootstrap.py"]
    # Attachments reside outside primary in the fixture. Mount only those
    # explicitly declared files, never the parent containing case.json.
    for attachment in request.get("attachments", []):
        visible = attachment.get("path", "")
        if visible.startswith(VISIBLE_ROOT + "/"):
            source = root / visible[len(VISIBLE_ROOT) + 1:]
            if source.is_file() and source.parent == root:
                args += ["--ro-bind", str(source), visible]
    variables = {
        "PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8",
        "SHELL": "/bin/bash",
        "HOME": str(Path.home()), "PINVOU3_HOME": f"{VISIBLE_ROOT}/home",
        "CODEWHALE_HOME": f"{VISIBLE_ROOT}/home/.codewhale",
        "CODEWHALE_SECRET_BACKEND": "file", "PINVOU3_AGENT_TASK_KEEP_SESSION": "1",
        "DEEPSEEK_ALLOW_INSECURE_HTTP": "1", "DEEPSEEK_FORCE_HTTP1": "1",
        "TMPDIR": "/tmp",
    }
    for name, value in variables.items():
        args += ["--setenv", name, value]
    return args + ["--chdir", f"{VISIBLE_ROOT}/primary"] + (program or ["/usr/bin/python3", "/run/pinvou/bootstrap.py"])


class VerifierSubprocess:
    """Run generated Python code in a second read-only, networkless namespace."""

    TimeoutExpired = subprocess.TimeoutExpired

    @staticmethod
    def run(argv, **kwargs):
        if argv[0] == sys.executable:
            project = kwargs.pop("cwd", None)
            if project is None and str(argv[-1]).endswith(".py"):
                project = Path(argv[-1]).parent
            if project is None:
                raise RuntimeError("generated-code verifier requires an explicit project")
            project = Path(project).resolve()
            translated = [str(value).replace(str(project), "/work/project") for value in argv[1:]]
            args = base_args()
            args += ["--ro-bind", str(project), "/work/project", "--chdir", "/work/project",
                     "--setenv", "PATH", "/usr/local/bin:/usr/bin:/bin",
                     "--setenv", "LANG", "C.UTF-8", "--setenv", "PYTHONDONTWRITEBYTECODE", "1",
                     "/usr/bin/python3", *translated]
            return subprocess.run(args, **kwargs)
        return subprocess.run(argv, **kwargs)


def self_test():
    with tempfile.TemporaryDirectory(prefix="pinvou-sandbox-check-") as temporary:
        root = Path(temporary) / "trial"
        for name in ["primary", "secondary", "third root", "四号 项目", "unselected", "home"]:
            (root / name).mkdir(parents=True)
        (root / "primary/visible.txt").write_text("VISIBLE")
        (root / "case.json").write_text("GRADER_METADATA_MUST_BE_HIDDEN")
        (Path(temporary) / "neighbor.txt").write_text("NEIGHBOR_MUST_BE_HIDDEN")
        req = root / "request.json"
        req.write_text(json.dumps({"workspace_roots": [
            f"{VISIBLE_ROOT}/{name}" for name in ("primary", "secondary", "third root", "四号 项目")
        ]}))
        program = """from pathlib import Path
assert Path('/work/trial/primary/visible.txt').read_text() == 'VISIBLE'
assert not Path('/work/trial/case.json').exists()
assert not Path('/work/trial/request.json').exists()
assert not Path('/work/trial/unselected').exists()
assert not Path(%r).exists()
assert not Path(%r).exists()
Path('/work/trial/secondary/result.txt').write_text('OK')
Path('/work/trial/third root/result.txt').write_text('THIRD_OK')
Path('/work/trial/四号 项目/result.txt').write_text('FOURTH_OK')
print('ISOLATION_OK')
""" % (str(Path.home() / ".pinvou3/settings.json"), str(Path(temporary) / "neighbor.txt"))
        result = subprocess.run(command(Path("/usr/bin/true"), req, root, root / "home",
                                        ["/usr/bin/python3", "-c", program]), capture_output=True, text=True, timeout=10)
        if result.returncode or "ISOLATION_OK" not in result.stdout:
            raise RuntimeError("filesystem isolation self-test failed: " + result.stderr[:500])
        if (root / "secondary/result.txt").read_text() != "OK":
            raise RuntimeError("trial write mapping failed")
        if (root / "third root/result.txt").read_text() != "THIRD_OK" or (root / "四号 项目/result.txt").read_text() != "FOURTH_OK":
            raise RuntimeError("additional workspace write mapping failed")


if __name__ == "__main__":
    self_test()
    print("evaluation namespace isolation verified")
