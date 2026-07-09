# Windows runtime resources

`python/`, `node/`, `pandoc/`, and `onnxruntime/` are generated during Windows
packaging from local runtime archives, then installed under `runtime/`:

- `src-tauri/resources/windows/python-3.13.14-embed-amd64.zip`
- `src-tauri/resources/windows/node-v24.18.0-win-x64.zip`
- `src-tauri/resources/windows/pandoc-3.10-windows-x86_64.zip`
- `src-tauri/resources/windows/onnxruntime-win-x64-1.20.0-runtime.zip`

Run this before packaging, or use `npm run build:nsis` which runs it
automatically:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\prepare-windows-runtimes.ps1
```

Expected generated layout:

```text
src-tauri/resources/windows/python/pythonw.exe
src-tauri/resources/windows/node/node.exe
src-tauri/resources/windows/pandoc/pandoc.exe
src-tauri/resources/windows/onnxruntime/onnxruntime.dll
```

Expected installed layout:

```text
{install_dir}/runtime/python/pythonw.exe
{install_dir}/runtime/node/node.exe
{install_dir}/runtime/pandoc/pandoc.exe
{install_dir}/runtime/onnxruntime/onnxruntime.dll
```

The generated runtime directories are intentionally ignored by git because they
are large binary payloads derived from the local release archives.
