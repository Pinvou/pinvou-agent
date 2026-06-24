# 契约：Windows Poppler 运行时

## 目标

定义 Windows 安装版 pinvou 对内置 Poppler 的文件位置、可发现性和错误行为要求。

## 安装契约

- MSI 安装完成后，应用安装目录下必须存在 `poppler` 文件夹。
- `poppler` 文件夹必须至少包含：
  - `pdftotext.exe`
  - `pdftoppm.exe`
  - 上述可执行文件运行所需 DLL
- 安装目录可以包含空格或中文字符，应用仍必须能定位并执行内置 Poppler。
- 升级和重新安装不得删除或遗漏 `poppler` 文件夹。

## 运行时契约

- Windows 下执行 PDF 文本提取时，应用必须优先使用 `{安装目录}/poppler/pdftotext.exe`。
- Windows 下执行扫描件 PDF OCR 兜底时，应用必须优先使用 `{安装目录}/poppler/pdftoppm.exe`。
- 如果内置路径不存在，应用可以降级探测系统 PATH，但用户可见错误不得要求用户手动安装 Poppler。
- 如果内置 Poppler 缺失或不可执行，用户可见错误必须说明安装内容异常或建议修复安装。

## 验收契约

- 干净 Windows 环境未安装 Poppler 时，安装 MSI 后上传文字层 PDF 应成功解析。
- 删除 `{安装目录}/poppler/pdftotext.exe` 后上传 PDF，应出现安装内容异常提示。
- 系统 PATH 中存在其他 Poppler 版本时，应用应优先使用安装目录内置版本。
