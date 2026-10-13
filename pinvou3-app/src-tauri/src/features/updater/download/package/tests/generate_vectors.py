"""Actual ZIP files with independently hashed member/FullPack identities."""
import hashlib
import io
import json
from pathlib import Path
import sys
import zipfile

directory = Path(sys.argv[1]).resolve()
assert directory.is_dir()
installer = b"synthetic-installer\x00" * 4096
helper = b"synthetic-helper-a" * 17
other = b"synthetic-helper-b" * 31
launcher = b"synthetic-launcher" * 11
native = {"finalInstaller": installer, "helper": helper, "extraHelper": other, "launcher": launcher}
names = {"finalInstaller": "payload/installer.deb", "helper": "helpers/current-helper",
         "extraHelper": "helpers/other-helper", "launcher": "launcher/stable-launcher"}
base = {names[role]: value for role, value in native.items()}

def identity(value):
    return {"size": len(value), "sha256": hashlib.sha256(value).hexdigest()}

def archive(files, method):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=method) as writer:
        for name, value in files.items():
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = method
            info.create_system = 3
            info.external_attr = (0o100644 << 16)
            writer.writestr(info, value)
    return output.getvalue()

cases = []
def emit(name, files, method=zipfile.ZIP_STORED, expected="positive", natives=None, changes=None):
    value = archive(files, method)
    path = directory / (name + ".zip")
    path.write_bytes(value)
    cases.append({"name": name, "path": str(path), "artifact": identity(value),
                  "natives": {role: identity(body) for role, body in (natives or native).items()},
                  "changes": changes or {}, "expected": expected})

emit("store", base)
emit("deflate", base, zipfile.ZIP_DEFLATED)
emit("native-alias", {**base, "aliases/helper-copy": helper})
emit("nested-extra", {**base, "extra/nested.zip": archive({"opaque": b"extra"}, zipfile.ZIP_STORED)})
large = {**native, "finalInstaller": b"z" * (33 * 1024 * 1024 + 1)}
emit("large-33MiB", {names[role]: value for role, value in large.items()}, zipfile.ZIP_DEFLATED, natives=large)
for role in native:
    emit("missing-" + role, {name: body for name, body in base.items() if name != names[role]},
         expected="native-missing")
emit("nested-only", {"nested.zip": archive(base, zipfile.ZIP_DEFLATED)}, expected="native-missing")
emit("member-corrupt", {**base, names["finalInstaller"]: b"changed"},
     expected="native-missing")
emit("member-size", base, expected="native-missing",
     changes={"finalInstallerSize": len(installer) + 1})
emit("member-digest", base, expected="native-missing",
     changes={"finalInstallerSHA256": "b" * 64})
emit("extra-helper-digest", base, expected="native-missing",
     changes={"extraHelperSHA256": "b" * 64})
(directory / "physical-vectors.json").write_text(json.dumps(cases), encoding="utf-8")
