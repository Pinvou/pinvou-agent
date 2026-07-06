# Quickstart：Windows 安装包内置 Python 与 Node 运行时验证

## 前置条件

确认两个源包存在：

```powershell
Test-Path "C:\Users\z27014\Downloads\node-v24.18.0-win-x64.zip"
Test-Path "C:\Users\z27014\Downloads\python-3.13.14-embed-amd64.zip"
```

期望大小参考：

- `node-v24.18.0-win-x64.zip`：约 `35.45 MiB`
- `python-3.13.14-embed-amd64.zip`：约 `10.46 MiB`

## 构建前准备检查

实现后应提供脚本完成以下动作：

```powershell
cd E:\Pinvou\pinvou3\pinvou3-app
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\prepare-windows-runtimes.ps1
```

检查结果应包含：

```powershell
Test-Path "src-tauri\resources\windows\python\pythonw.exe"
Test-Path "src-tauri\resources\windows\node\node.exe"
```

## 构建 NSIS 安装包

```powershell
cd E:\Pinvou\pinvou3\pinvou3-app
npm run build:nsis
```

构建后检查生成脚本中包含运行时资源和环境变量 hook：

```powershell
rg -n "PINVOU3_PYTHON|\\python|\\node" src-tauri\target\release\nsis\x64\installer.nsi
```

## 安装后验证

在测试机器安装生成的 NSIS 安装包后，打开新的 PowerShell：

```powershell
$installDir = "C:\Program Files\pinvou3"
Test-Path "$installDir\python\pythonw.exe"
Test-Path "$installDir\node\node.exe"
[Environment]::GetEnvironmentVariable("PINVOU3_PYTHON", "Machine")
[Environment]::GetEnvironmentVariable("Path", "Machine") -split ';' |
  Where-Object { $_ -like "$installDir\python" -or $_ -like "$installDir\node" }
```

期望：

- `pythonw.exe` 存在。
- `node.exe` 存在。
- `PINVOU3_PYTHON` 指向 `$installDir\python\pythonw.exe`。
- 系统 `PATH` 包含 `$installDir\python` 与 `$installDir\node`。

## Python 型 MCP 验证

在没有真实系统 Python、仅有 WindowsApps Python 占位符的测试环境中：

1. 安装应用。
2. 触发一次会产出单文件成品的任务。
3. 确认 `mcp_pinvou3_present_artifact` 可启动并返回成功。
4. 确认不出现 `Python was not found`。

可用以下静态检查辅助确认应用优先使用内置 Python：

```powershell
cargo test python --manifest-path src-tauri\Cargo.toml
```

## 卸载验证

卸载应用后打开新的 PowerShell：

```powershell
[Environment]::GetEnvironmentVariable("PINVOU3_PYTHON", "Machine")
[Environment]::GetEnvironmentVariable("Path", "Machine") -split ';' |
  Where-Object { $_ -like "*\pinvou3\python" -or $_ -like "*\pinvou3\node" }
```

期望：

- 如果 `PINVOU3_PYTHON` 原本指向本应用安装目录，则卸载后为空或不存在。
- PATH 中不再包含本应用安装目录下的 `python` 与 `node`。
- 用户其他 Python/Node 路径仍保留。

## 本次实现记录

已执行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\prepare-windows-runtimes.ps1
cargo test python --manifest-path src-tauri\Cargo.toml
cargo check --manifest-path src-tauri\Cargo.toml
npm run build:nsis
rg -n "PINVOU3_PYTHON|\\python|\\node" src-tauri\target\release\nsis\x64\installer.nsi
```

本次生成结果：

```powershell
src-tauri\target\release\bundle\nsis\pinvou3_0.5.7_x64-setup.exe
Size: 163.36 MiB
```

未执行：

- 手动安装生成的 NSIS 包后验证系统 `PINVOU3_PYTHON` 与 `PATH`。
- 手动卸载后验证系统 `PINVOU3_PYTHON` 与 `PATH` 清理行为。

原因：当前实现阶段只进行构建与静态检查，未实际运行需要管理员权限和系统环境变量写入的安装/卸载流程。
