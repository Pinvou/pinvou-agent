# Windows Poppler Runtime

Version: Poppler 26.02.0 Windows runtime

Purpose: bundled PDF text extraction and page rendering for the Windows installer.

License: see `LICENSE` in this directory.

Required executables:

- `pdftotext.exe`
- `pdftoppm.exe`

Integrity snapshot:

- Source file count before adding this README: 39 files
- Required executables present: yes
- DLL/runtime files copied from source directory root: yes
- License file added from the approved Poppler runtime license text: yes

Packaging target:

- Tauri resource source: `resources/windows/poppler/`
- Tauri resource target: `poppler`

If the Poppler distribution is updated, refresh this folder from the approved
source and keep the bundled license/notice files in sync with the runtime that
is shipped in the Windows installer.
