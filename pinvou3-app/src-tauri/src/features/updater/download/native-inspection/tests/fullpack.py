"""Deterministic actual ZIP32 with three opaque native byte members."""
import sys
from pathlib import Path
import zipfile
native = Path(sys.argv[1]).read_bytes()
members = {'payload/installer.exe':native, 'helper/helper.exe':native,
           'launcher/launcher.exe':native}
if len(sys.argv) == 6:
    members['helper/first.exe'] = Path(sys.argv[3]).read_bytes()
    members['helper/execution.exe'] = Path(sys.argv[4]).read_bytes()
    members['aliases/execution.exe'] = members['helper/execution.exe']
    members['launcher/launcher.exe'] = Path(sys.argv[5]).read_bytes()
with zipfile.ZipFile(sys.argv[2], 'w', compression=zipfile.ZIP_STORED) as archive:
    for name, content in members.items():
        info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
        info.create_system = 3
        info.external_attr = 0o100644 << 16
        archive.writestr(info, content)
