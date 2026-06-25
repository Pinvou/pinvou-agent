# 契约：Windows Tesseract 运行时

## 目标

定义 Windows 安装版 pinvou 对内置 Tesseract OCR 的文件位置、可发现性、语言数据和错误行为要求。

## 安装契约

- MSI 安装完成后，应用安装目录下必须存在 `tesseract` 文件夹。
- `tesseract` 文件夹必须至少包含：
  - `tesseract.exe`
  - Tesseract 运行所需 DLL 和辅助文件
  - `tessdata/chi_sim.traineddata`
  - `tessdata/eng.traineddata`
  - runtime、依赖库和语言数据对应的许可证、版权或来源说明
- 安装目录可以包含空格或中文字符，应用仍必须能定位并执行内置 Tesseract。
- 升级和重新安装不得删除或遗漏 `tesseract` 文件夹。

## 运行时契约

- Windows 中执行扫描件 PDF OCR 时，应用必须优先使用 `{安装目录}/tesseract/tesseract.exe`。
- Windows 中执行 Tesseract 时，应用必须显式指向 `{安装目录}/tesseract/tessdata` 或等价的内置语言数据目录。
- Windows 默认 OCR 语言参数为 `chi_sim+eng`。
- 如果内置 Tesseract 缺失、不可执行或语言数据不完整，用户可见错误必须说明安装内容异常，并建议修复安装或重新安装 pinvou。
- 如果系统 PATH 中存在其他 Tesseract 版本，Windows 安装版仍应优先使用安装目录内置版本。
- 运行外部 OCR 命令时不得弹出额外控制台窗口。

## 验收契约

- 干净 Windows 环境未安装 Tesseract 时，安装 MSI 后上传中英文扫描件 PDF 应成功返回 OCR 文本。
- 删除 `{安装目录}/tesseract/tesseract.exe` 后上传扫描件 PDF，应出现修复安装提示。
- 删除 `{安装目录}/tesseract/tessdata/chi_sim.traineddata` 后上传中文扫描件 PDF，应出现中文语言数据缺失或安装内容异常提示。
- 系统 PATH 中存在其他 Tesseract 版本时，验收记录应能确认 pinvou 使用内置版本。
- MSI 解包或安装目录检查必须能确认 runtime、语言数据和许可证材料已随包交付。
