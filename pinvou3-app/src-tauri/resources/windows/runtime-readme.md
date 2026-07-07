# Windows runtime resources

`python/` and `node/` are generated during Windows packaging from local runtime
archives, then installed under `runtime/python` and `runtime/node`:

- `C:\Users\z27014\Downloads\python-3.13.14-embed-amd64.zip`
- `C:\Users\z27014\Downloads\node-v24.18.0-win-x64.zip`

Run this before packaging, or use `npm run build:nsis` which runs it
automatically:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\prepare-windows-runtimes.ps1
```

Expected generated layout:

```text
src-tauri/resources/windows/python/pythonw.exe
src-tauri/resources/windows/node/node.exe
```

Expected installed layout:

```text
{install_dir}/runtime/python/pythonw.exe
{install_dir}/runtime/node/node.exe
```

The generated runtime directories are intentionally ignored by git because they
are large binary payloads derived from the local release archives.
