"""Deterministic actual ZIP32 with three opaque native byte members."""
import sys
from pathlib import Path
import zipfile
native = Path(sys.argv[1]).read_bytes()
with zipfile.ZipFile(sys.argv[2], 'w', compression=zipfile.ZIP_STORED) as archive:
    for name in ['payload/installer.exe', 'helper/helper.exe', 'launcher/launcher.exe']:
        info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
        info.create_system = 3
        info.external_attr = 0o100644 << 16
        archive.writestr(info, native)
