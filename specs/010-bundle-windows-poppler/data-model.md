# 数据模型：Windows 内置 Poppler 安装

## PopplerRuntime

**含义**：随 Windows 安装包分发并安装到应用目录的 Poppler 文件集合。

**字段**：
- `source_path`：批准的源目录，固定为 `C:\Users\z27014\Downloads\poppler-26.02.0`。
- `repo_path`：仓库内资源路径，计划为 `pinvou3-app/src-tauri/resources/windows/poppler/`。
- `install_path`：安装后路径，计划为 `{安装目录}/poppler`。
- `required_files`：至少包含 `pdftotext.exe`；扫描件/OCR 兜底链路需要 `pdftoppm.exe`。
- `dll_files`：Poppler 可执行文件运行所需 DLL，随源目录整体复制。
- `version`：`26.02.0`。

**验证规则**：
- `source_path` 必须存在且包含 `pdftotext.exe`。
- `repo_path` 必须包含源目录中的可执行文件和 DLL。
- `install_path` 必须在 MSI 安装后存在。
- `required_files` 不可缺失，否则 Windows PDF 上传应报告安装内容异常。

## WindowsInstallerPackage

**含义**：面向 Windows 用户分发的 pinvou MSI 安装包。

**字段**：
- `target`：Windows MSI。
- `app_install_dir`：pinvou 应用安装目录。
- `bundled_resources`：安装包内包含的资源集合，必须包含 `PopplerRuntime`。
- `path_update`：安装后让 `{安装目录}/poppler` 对应用可发现的环境路径策略。

**验证规则**：
- MSI 安装后必须释放 PopplerRuntime 到 `app_install_dir/poppler`。
- 安装目录包含空格或中文字符时仍必须可用。
- 升级/重装后 `poppler` 目录仍必须存在且可用。

## PdfToolResolution

**含义**：应用在 Windows 下定位 PDF 工具命令的结果。

**字段**：
- `pdftotext_path`：优先指向 `{安装目录}/poppler/pdftotext.exe`。
- `pdftoppm_path`：优先指向 `{安装目录}/poppler/pdftoppm.exe`。
- `source`：`bundled` 或 `system_path`。
- `status`：`available`、`missing`、`not_executable`。
- `diagnostic_message`：面向用户或支持人员的错误说明。

**状态转换**：
- `missing` -> `available`：安装包释放 Poppler 后。
- `available` -> `missing`：用户删除安装目录下 Poppler 文件后。
- `not_executable` -> `available`：修复安装或恢复文件权限后。

## DependencyCheckItem

**含义**：设置页依赖体检中的一项用户可见依赖能力。

**字段**：
- `key`：依赖能力标识。
- `installed`：当前是否可用。
- `user_action_required`：是否需要用户手动补全。
- `platform_visibility`：该项在哪些平台展示。

**验证规则**：
- Windows 下 Poppler/PDF 文本提取项不得以“缺失、需手动安装”的形式展示。
- Linux 下 Poppler/PDF 文本提取项保持原有展示与安装提示。
- 其他依赖项不受 Poppler 隐藏策略影响。

## PdfAttachmentIngest

**含义**：用户上传 PDF 后的附件解析结果。

**字段**：
- `kind`：`pdf` 或异常状态。
- `markdown`：提取出的 PDF 文本内容。
- `warning`：可发送但需提示用户的非致命问题。
- `error`：无法解析时的明确错误。
- `tool_source`：用于解析 PDF 的工具来源。

**验证规则**：
- Windows 安装版在内置 Poppler 可用时，文字层 PDF 应产生可发送文本内容。
- Windows 内置 Poppler 缺失时，错误应指向安装异常/修复安装。
- 不得在 Windows 安装版把 Poppler 缺失解释为用户需要手动安装。
