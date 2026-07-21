# WoSign command-line signing tool

This directory contains the build-time WoSign command-line tool used to sign
Windows NSIS artifacts. It is not listed in `tauri.conf.json` bundle resources
and must not be shipped with the application.

- Tool: `wosigncodecmd.exe`
- File version: `1.0.0.4`
- SHA-256: `955E6FD4B94A4EDF5CEED2800645A1BC5A6E58D7B7E4465C6F882DD573BE7A7E`

The certificate thumbprint and UKey password used by the current build process
are configured directly in `sign.ps1`. Update that script when the signing
certificate or UKey password changes.
