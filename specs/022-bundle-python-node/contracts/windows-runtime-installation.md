# 契约：Windows 运行时安装与环境变量

## 构建输入契约

构建 Windows 安装包前，以下文件必须存在并可读取：

```text
C:\Users\z27014\Downloads\node-v24.18.0-win-x64.zip
C:\Users\z27014\Downloads\python-3.13.14-embed-amd64.zip
```

构建前校验必须确认：

- Python zip 可展开，且包含 `pythonw.exe`。
- Node zip 可展开，且包含 `node.exe`。
- 展开结果可规范化为 `python` 与 `node` 两个固定目录。

## 安装后文件契约

安装完成后，应用安装目录必须满足：

```text
$INSTDIR\
├── python\
│   └── pythonw.exe
└── node\
    └── node.exe
```

允许目录内包含运行时所需的其他文件，但不得要求调用方知道源包版本目录名。

## 系统环境变量契约

安装完成后，新进程读取系统环境变量应满足：

```text
PINVOU3_PYTHON=$INSTDIR\python\pythonw.exe
PATH contains $INSTDIR\python
PATH contains $INSTDIR\node
```

要求：

- 路径必须支持安装目录包含空格。
- 覆盖安装或升级后，变量必须指向当前安装目录。
- 不要求设置 Node 专用环境变量。

### NSIS 行为

NSIS 安装包通过 `resources/windows/nsis/runtime-env.ps1` 在安装后配置机器级环境变量：

- 安装时设置 `PINVOU3_PYTHON=$INSTDIR\python\pythonw.exe`。
- 安装时将 `$INSTDIR\python` 与 `$INSTDIR\node` 追加到机器级 `Path`，并按规范化路径去重。
- 卸载时仅删除指向本安装目录的 `PINVOU3_PYTHON`。
- 卸载时仅从机器级 `Path` 删除本安装目录下的 `python` 与 `node` 项。
- 安装和卸载后广播 `WM_SETTINGCHANGE`，让新进程尽快看到环境变化。

### MSI/WiX 行为

MSI 安装包通过 `resources/windows/python-node-path.wxs` 声明环境变量组件：

- `PythonNodeRuntimeEnvironment` 组件设置机器级 `PINVOU3_PYTHON`。
- 同一组件将 `[INSTALLDIR]python` 与 `[INSTALLDIR]node` 写入机器级 `PATH`。
- 组件卸载时由 WiX/Windows Installer 移除对应环境变量项。

## Python 解析契约

应用解析 Python 命令时必须满足：

1. 优先使用有效的 `PINVOU3_PYTHON`。
2. 其次使用当前可执行文件同级安装目录下的 `python\pythonw.exe`。
3. 系统 Python 仅作为开发或兜底路径。
4. Microsoft Store `WindowsApps\python.exe` 或 `pythonw.exe` 占位符不得被视为可用 Python。

## 卸载契约

卸载时必须满足：

- 如果 `PINVOU3_PYTHON` 指向 `$INSTDIR\python\pythonw.exe`，删除该变量。
- 如果 `PINVOU3_PYTHON` 指向其他路径，保留该变量。
- 从系统 `PATH` 中删除 `$INSTDIR\python` 与 `$INSTDIR\node`。
- 保留 PATH 中其他 Python/Node 路径。
- 已勾选“删除应用程序数据”时，继续保留现有 `.pinvou3` 用户数据清理语义。

## 验收命令示例

```powershell
$installDir = "C:\Program Files\pinvou3"
Test-Path "$installDir\python\pythonw.exe"
Test-Path "$installDir\node\node.exe"
[Environment]::GetEnvironmentVariable("PINVOU3_PYTHON", "Machine")
[Environment]::GetEnvironmentVariable("Path", "Machine")
```
