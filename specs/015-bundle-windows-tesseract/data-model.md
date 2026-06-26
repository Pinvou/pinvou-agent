# 数据模型：Windows 内置 Tesseract OCR

## TesseractRuntime

**含义**：随 Windows 安装包分发并安装到应用目录的 Tesseract OCR 文件集合。

**字段**：
- `source_path`：实施阶段导入 runtime 的本机源目录或离线包位置，必须记录来源和版本。
- `repo_path`：仓库内资源路径，计划为 `pinvou3-app/src-tauri/resources/windows/tesseract/`。
- `install_path`：安装后路径，计划为 `{安装目录}/tesseract`。
- `executable`：OCR 可执行文件，必须为 `tesseract.exe`。
- `required_runtime_files`：`tesseract.exe` 运行所需 DLL、配置文件和辅助文件。
- `license_files`：Tesseract runtime、依赖库和语言数据随包分发所需的许可证、版权和来源说明。
- `version`：导入的 Tesseract runtime 版本。

**验证规则**：
- `repo_path` 必须包含 `tesseract.exe`。
- `repo_path` 必须包含 `tessdata/chi_sim.traineddata` 和 `tessdata/eng.traineddata`。
- `repo_path` 必须包含可追溯的许可证和来源说明。
- MSI 安装后 `install_path` 必须存在，且在包含空格或中文字符的安装目录下仍可运行。

## TesseractLanguageData

**含义**：Tesseract OCR 用于识别文本的语言数据文件集合。

**字段**：
- `tessdata_dir`：语言数据目录，计划为 `{安装目录}/tesseract/tessdata`。
- `languages`：内置语言列表，当前必须包含 `chi_sim` 和 `eng`。
- `preferred_lang_arg`：OCR 默认语言参数，Windows 为 `chi_sim+eng`。
- `source`：语言数据来源和许可证说明。

**验证规则**：
- Windows 内置 runtime 正常状态下不得缺少 `chi_sim` 或 `eng`。
- Windows 中文语言数据缺失时，应返回安装内容异常或修复安装提示，不得静默降级为英文。
- Linux 仍可沿用系统包和现有语言探测策略。

## WindowsInstallerPackage

**含义**：面向 Windows 用户分发的 pinvou MSI 安装包。

**字段**：
- `target`：Windows MSI。
- `app_install_dir`：pinvou 应用安装目录。
- `bundled_resources`：安装包内包含的资源集合，必须包含 `TesseractRuntime`。
- `path_update`：让 `{安装目录}/tesseract` 对当前应用进程可发现的环境路径策略。

**验证规则**：
- MSI 安装后必须释放 `TesseractRuntime` 到 `app_install_dir/tesseract`。
- 升级和重新安装不得遗漏 `tesseract` 目录。
- 构建验收必须能确认 `tesseract.exe`、`tessdata`、许可证文件均进入安装包。

## OcrToolResolution

**含义**：应用在 Windows 下定位 OCR 命令和语言数据的结果。

**字段**：
- `tesseract_path`：优先指向 `{安装目录}/tesseract/tesseract.exe`。
- `tessdata_dir`：优先指向 `{安装目录}/tesseract/tessdata`。
- `source`：`bundled` 或 `system_path`。
- `status`：`available`、`missing`、`missing_language_data`、`not_executable`。
- `diagnostic_message`：面向用户或支持人员的错误说明。

**状态转换**：
- `missing` -> `available`：安装包释放 Tesseract runtime 后。
- `missing_language_data` -> `available`：恢复 `chi_sim` 和 `eng` 语言数据后。
- `available` -> `missing`：用户删除安装目录下 `tesseract.exe` 后。
- `available` -> `missing_language_data`：用户删除 `tessdata` 或语言文件后。

## DependencyCheckItem

**含义**：设置页依赖体检中的一项用户可见依赖能力。

**字段**：
- `key`：依赖能力标识，例如 `ocr`。
- `installed`：当前是否可用。
- `user_action_required`：是否需要用户手动补全第三方依赖。
- `platform_visibility`：该项在哪些平台展示。
- `repair_hint`：内置组件损坏时的修复建议。

**验证规则**：
- Windows 中 OCR 项不得以“缺失 Tesseract，需要手动安装”的形式展示。
- Windows 内置 OCR 损坏时，提示应指向修复安装或重新安装 pinvou。
- Linux 中 OCR 项保持现有系统包提示。

## ScannedPdfOcrIngest

**含义**：用户上传无文字层 PDF 后产生的 OCR 解析结果。

**字段**：
- `input_pdf`：上传的 PDF 文件。
- `render_tool`：用于 PDF 转图的 Poppler `pdftoppm`。
- `ocr_tool`：用于识别页面图片的 Tesseract 命令。
- `language_arg`：OCR 语言参数。
- `markdown`：提取出的文本内容。
- `warning`：OCR 质量提示或页数截断提示。
- `error`：无法解析时的明确错误。

**验证规则**：
- 有文字层 PDF 必须优先使用文字层解析结果。
- 无文字层 PDF 进入 OCR 兜底时，Windows 必须优先使用内置 OCR runtime。
- 内置 OCR runtime 缺失或语言数据缺失时，错误不得为空白，且必须可定位到修复安装。
