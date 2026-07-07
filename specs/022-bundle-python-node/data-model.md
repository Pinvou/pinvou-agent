# 数据模型：Windows 内置 Python 与 Node 运行时

## RuntimeSourceArchive（运行时源包）

表示打包前维护者提供的离线运行时 zip。

**字段**

- `kind`：`python` 或 `node`
- `source_path`：本地 zip 路径
- `expected_version`：期望版本标识，例如 `3.13.14` 或 `24.18.0`
- `required_files`：必须存在的关键文件列表
- `expanded_root_policy`：源包展开后如何规范化顶层目录
- `validation_status`：`pending`、`valid`、`invalid`
- `failure_reason`：校验失败原因

**校验规则**

- `source_path` 必须存在且可读。
- `python` 源包必须能提供 `pythonw.exe`。
- `node` 源包必须能提供 `node.exe`。
- 源包展开后不得产生影响最终路径的多余顶层版本目录。

## InstalledRuntimeDirectory（安装后的运行时目录）

表示最终用户机器安装目录下的运行时文件树。

**字段**

- `install_dir`：应用安装目录
- `python_dir`：`install_dir\python`
- `node_dir`：`install_dir\node`
- `pythonw_path`：`install_dir\python\pythonw.exe`
- `node_path`：`install_dir\node\node.exe`
- `owner`：本应用安装包

**校验规则**

- `pythonw_path` 必须存在且可执行。
- `node_path` 必须存在且可执行。
- 目录名必须稳定为 `python` 与 `node`。

## RuntimeEnvironmentVariable（运行时环境变量）

表示安装器写入系统级环境变量的状态。

**字段**

- `pinvou3_python`：系统环境变量 `PINVOU3_PYTHON`
- `path_entries`：系统 `PATH` 中与本应用相关的条目
- `scope`：系统级
- `managed_by_install_dir`：用于判断卸载时是否可清理

**校验规则**

- `PINVOU3_PYTHON` 必须等于 `install_dir\python\pythonw.exe`。
- `PATH` 必须包含 `install_dir\python` 与 `install_dir\node`。
- 卸载时只能删除指向当前应用安装目录的变量和值。
- 如果 `PINVOU3_PYTHON` 指向用户自定义路径，不得删除。
- 如果 `PATH` 中存在其他 Python/Node 路径，不得删除。

## RuntimeResolutionState（应用运行时解析状态）

表示应用启动 Python 型 MCP 时的解释器选择结果。

**字段**

- `selected_python`：最终选择的 Python 命令或路径
- `source`：`PINVOU3_PYTHON`、`bundled_python`、`system_python`、`fallback`
- `is_windowsapps_alias`：是否为 Microsoft Store 占位符
- `usable`：是否可实际启动

**状态流转**

```text
未解析
  -> 读取 PINVOU3_PYTHON
  -> 检查安装目录 python\pythonw.exe
  -> 检查真实系统 Python
  -> 兜底失败/提示不可用
```

**校验规则**

- 当 `PINVOU3_PYTHON` 指向存在的 `pythonw.exe` 时，必须选择该路径。
- 当安装目录下存在 `python\pythonw.exe` 时，不应选择 WindowsApps alias。
- WindowsApps Python alias 不应被视为可用系统 Python。
