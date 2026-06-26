# Windows 7-Zip Runtime

This directory contains the minimal 7-Zip CLI runtime bundled with the Windows
MSI so Pinvou can inspect and extract user-uploaded archives without requiring a
system-level 7-Zip installation.

## Source

- Version: 7-Zip 26.01 (x64), 2026-04-27
- Upstream: https://www.7-zip.org/

## Bundled Files

- `7z.exe` - command line executable
- `7z.dll` - 7-Zip engine module
- `License.txt` - upstream license
- `readme.txt` - upstream readme

## Excluded Files

The MSI intentionally excludes Windows Shell plugins, GUI tools, SFX modules,
help files, upstream history, uninstallers, and language packs. Pinvou only runs
`7z.exe l -slt` and `7z.exe x` for archive ingest.

Excluded examples: `7-zip.dll`, `7-zip32.dll`, `7zFM.exe`, `7zG.exe`,
`7z.sfx`, `7zCon.sfx`, `7-zip.chm`, `History.txt`, `Uninstall.exe`, and
`Lang/`.

## Verified Formats

`7z.exe i` with only `7z.exe` and `7z.dll` present still reports support for
zip, 7z, Rar, Rar5, and the Rar1/2/3/5 decoders.
