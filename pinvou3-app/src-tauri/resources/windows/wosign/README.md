# WoSign command-line signing tool

This directory contains the build-time WoSign tools used to sign Windows NSIS
artifacts. It is not listed in `tauri.conf.json` bundle resources and must not
be shipped with the application. `wosigncodecmd.exe` requires
`wosigncode.exe` in the same directory even when signing from the command line.

- Tool: `wosigncodecmd.exe`; file version: `1.0.0.4`; SHA-256:
  `955E6FD4B94A4EDF5CEED2800645A1BC5A6E58D7B7E4465C6F882DD573BE7A7E`
- Companion tool: `wosigncode.exe`; file version: `3.0.1.26`; SHA-256:
  `73825CE95F524BCD932E5852B7D59E890318E268ED55B7AD65315460613CAA11`

The certificate thumbprint and UKey password are read from the repository-local,
gitignored `scripts/.builtin-secrets.env` file through
`PINVOU3_WOSIGN_THUMBPRINT` and `PINVOU3_WOSIGN_PASSWORD`. The script always uses
the tools in this directory, runs the command from this directory so the companion
executable can be resolved, and keeps `/isf` to ignore already-signed inputs. The
command still requests an RFC 3161 timestamp. The script treats a zero WoSign exit
code as success and does not perform an additional Authenticode, certificate
thumbprint, or timestamp validation. Update the private secrets file when the
signing certificate or UKey password changes.
