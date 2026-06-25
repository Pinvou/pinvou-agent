# 研究：Windows 内置 Tesseract OCR

## 决策 1：Tesseract 作为 Windows 安装包运行时资源分发

**Decision**：将受控的 Windows Tesseract runtime 导入 `pinvou3-app/src-tauri/resources/windows/tesseract/`，安装后释放到 `{安装目录}/tesseract`。

**Rationale**：当前 Windows 已通过同一资源机制内置 Poppler 和 Pandoc。Tesseract 同样属于附件解析链路所需的本地命令行工具，放入 Windows 资源目录能保持安装包结构、运行时发现方式和验收方式一致。用户未指定固定源目录，因此实施时先从本机已验证 runtime 或离线包导入，并记录来源、版本和许可证；导入后的仓库目录成为后续构建的受控源。

**Alternatives considered**：
- 继续要求用户手动安装 Tesseract：拒绝，无法满足安装后开箱可用。
- 构建时在线下载 Tesseract：拒绝，会引入外部网络和构建不稳定性。
- 只把 `tesseract.exe` 放入资源目录：拒绝，Tesseract 运行还依赖 DLL、`tessdata` 和许可证材料。

## 决策 2：Windows 运行时优先使用内置 `tesseract.exe` 和内置 `tessdata`

**Decision**：Windows 下 OCR 命令优先解析 `{当前可执行文件目录}/tesseract/tesseract.exe`，并显式传入 `{当前可执行文件目录}/tesseract/tessdata`。找不到内置 runtime 时可降级到系统 PATH，但用户可见错误仍指向“修复或重新安装 pinvou”。

**Rationale**：内置 runtime 是本 feature 的受控依赖。显式路径和显式 `tessdata` 能避免用户机器上其他 Tesseract 版本或 `TESSDATA_PREFIX` 影响识别结果，也能保证简体中文数据缺失时暴露明确错误。PATH 降级只用于开发态和异常诊断，不作为 Windows 安装版的主要保证。

**Alternatives considered**：
- 只依赖 PATH：拒绝，无法保证受控版本和语言数据。
- 在 `file_ingest` 中硬编码 Windows 路径：拒绝，平台差异应由 OS 层封装。
- 仅设置 `TESSDATA_PREFIX` 环境变量：可作为补充，但命令参数仍应显式传入 `--tessdata-dir`，降低环境变量顺序不确定性。

## 决策 3：语言选择固定优先 `chi_sim+eng`

**Decision**：Windows 内置 OCR 正常状态必须包含 `tessdata/chi_sim.traineddata` 和 `tessdata/eng.traineddata`；OCR 调用优先使用 `chi_sim+eng`。若中文数据缺失，Windows 错误应提示安装内容异常，不再静默降级为英文。

**Rationale**：pinvou 面向国内政企场景，扫描件 PDF 的中文识别是核心验收目标。Linux 现有逻辑可以继续根据系统语言包降级，但 Windows 内置包的缺失属于安装包完整性问题，应以可修复错误暴露。

**Alternatives considered**：
- 继续 `--list-langs` 后降级 `eng`：拒绝，Windows 内置包场景下会隐藏中文数据缺失。
- 只内置 `eng`：拒绝，不满足简体中文 OCR 需求。
- 内置更多语言包：暂不采用，会扩大 MSI 体积，且不在当前需求范围内。

## 决策 4：依赖体检在 Windows 不再提示手动安装 Tesseract

**Decision**：Windows 依赖体检不把 OCR 显示为“需要用户手动安装 Tesseract”的阻断项。若内置 runtime 缺失或损坏，相关错误文案指向修复安装或重新安装 pinvou。Linux 保持 `tesseract-ocr tesseract-ocr-chi-sim poppler-utils` 的系统包提示。

**Rationale**：Windows 内置后，OCR 可用性属于 pinvou 安装内容完整性，不应再让用户自行寻找第三方安装方式。Linux 仍通过系统包管理器补齐依赖，现有提示符合当前平台策略。

**Alternatives considered**：
- Windows 继续显示 `tesseract-ocr tesseract-ocr-chi-sim`：拒绝，会误导用户。
- 全平台隐藏 OCR 体检：拒绝，会破坏 Linux 可维护性。
- 显示“已内置”信息项：可作为后续优化，但当前规格要求是去掉手动安装检查。

## 决策 5：继续只在扫描件 PDF 兜底链路使用 OCR

**Decision**：保留 PDF 解析顺序：先 `pdftotext -layout`，文本为空时才用 `pdftoppm` 渲染页面并调用 Tesseract。普通图片上传不恢复到 Tesseract 预解析主路径。

**Rationale**：现有附件解析已经按文档类型区分，普通图片上传目前不以 OCR 文本为主产物。此 feature 目标是补齐扫描件 PDF 兜底依赖，不扩大行为面能降低回归风险。

**Alternatives considered**：
- 所有 PDF 都同时 OCR：拒绝，会增加耗时并可能降低文本层 PDF 质量。
- 普通图片也走 OCR：拒绝，超出当前规格范围。
- 改用其他 OCR 引擎：拒绝，当前需求明确为内置 Tesseract OCR。
