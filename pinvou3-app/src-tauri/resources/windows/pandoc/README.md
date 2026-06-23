# Windows Pandoc Runtime

Version: Pandoc 3.10 Windows runtime

Purpose: bundled modern document conversion for the Windows installer.

License and notices: see `COPYING.rtf` and `COPYRIGHT.txt` in this directory.

Required executable:

- `pandoc.exe`

Integrity snapshot:

- Source file count before adding this README: 4 files
- Required executable present: yes
- License and copyright files copied from source directory root: yes
- Manual file copied from source directory root: yes

Packaging target:

- Tauri resource source: `resources/windows/pandoc/`
- Tauri resource target: `pandoc`

If the Pandoc distribution is updated, refresh this folder from the approved
source and keep the bundled license/notice files in sync with the runtime that
is shipped in the Windows installer.
