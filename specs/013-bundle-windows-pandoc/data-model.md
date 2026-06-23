# 数据模型：Windows 内置 Pandoc 安装

## PandocRuntime

**含义**：随 Windows 安装包分发并安装到应用目录的 Pandoc 文件集合。

**字段**：
- `source_path`：批准的源目录，固定为 `C:\Users\z27014\Downloads\pandoc-3.10`。
- `repo_path`：仓库内资源路径，计划为 `pinvou3-app/src-tauri/resources/windows/pandoc/`。
- `install_path`：安装后路径，计划为 `{安装目录}/pandoc`。
- `required_files`：至少包含 `pandoc.exe`。
- `license_files`：Pandoc 发行包随附许可/版权文件，例如 `COPYING.rtf` 和 `COPYRIGHT.txt`。
- `version`：`3.10`。

**验证规则**：
- `source_path` 必须存在且包含 `pandoc.exe`。
- `repo_path` 必须包含源目录中的可执行文件和许可/版权文件。
- `install_path` 必须在 MSI 安装后存在。
- `required_files` 不可缺失，否则 Windows 文档上传应报告安装内容异常。

## WindowsInstallerPackage

**含义**：面向 Windows 用户分发的 pinvou MSI 安装包。

**字段**：
- `target`：Windows MSI。
- `app_install_dir`：pinvou 应用安装目录。
- `bundled_resources`：安装包内包含的资源集合，必须包含 `PandocRuntime`。
- `path_update`：安装后让 `{安装目录}/pandoc` 对应用可发现的环境路径策略。

**验证规则**：
- MSI 安装后必须释放 PandocRuntime 到 `app_install_dir/pandoc`。
- 安装目录包含空格或中文字符时仍必须可用。
- 升级/重装后 `pandoc` 目录仍必须存在且可用。

## PandocToolResolution

**含义**：应用在 Windows 下定位 Pandoc 工具命令的结果。

**字段**：
- `pandoc_path`：优先指向 `{安装目录}/pandoc/pandoc.exe`。
- `source`：`bundled` 或 `system_path`。
- `status`：`available`、`missing`、`not_executable`。
- `diagnostic_message`：面向用户或支持人员的错误说明。

**状态转换**：
- `missing` -> `available`：安装包释放 Pandoc 后。
- `available` -> `missing`：用户删除安装目录下 Pandoc 文件后。
- `not_executable` -> `available`：修复安装或恢复文件权限后。

## DependencyCheckItem

**含义**：设置页依赖体检中的一项用户可见依赖能力。

**字段**：
- `key`：依赖能力标识。
- `installed`：当前是否可用。
- `user_action_required`：是否需要用户手动补全。
- `platform_visibility`：该项在哪些平台展示。

**验证规则**：
- Windows 下 Pandoc/现代文档解析项不得以“缺失、需手动安装”的形式展示。
- Linux 下 Pandoc/现代文档解析项保持原有展示与安装提示。
- 其他依赖项不受 Pandoc 隐藏策略影响。

## DocumentAttachmentIngest

**含义**：用户上传依赖 Pandoc 的文档后产生的附件解析结果。

**字段**：
- `kind`：文档类型或异常状态。
- `markdown`：提取出的文档文本内容。
- `warning`：可发送但需提示用户的非致命问题。
- `error`：无法解析时的明确错误。
- `tool_source`：用于解析文档的 Pandoc 来源。

**验证规则**：
- Windows 安装版在内置 Pandoc 可用时，支持的现代文档应产生可发送文本内容。
- Windows 内置 Pandoc 缺失时，错误应指向安装异常/修复安装。
- 不得在 Windows 安装版把 Pandoc 缺失解释为用户需要手动安装。
