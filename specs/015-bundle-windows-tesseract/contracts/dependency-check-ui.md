# 契约：依赖体检 UI

## 目标

定义 Windows 内置 Tesseract 后，依赖体检页对 OCR 能力的展示和修复提示要求。

## Windows 行为

- 正常安装后，依赖体检页不得提示用户手动安装 `tesseract-ocr` 或 `tesseract-ocr-chi-sim`。
- 如果 OCR 能力仍作为可见项展示，其状态必须基于内置 runtime 和语言数据，不得基于用户全局 PATH。
- 内置 OCR 缺失或损坏时，提示文案必须指向修复安装或重新安装 pinvou。
- Windows 依赖体检中不得出现 Linux 包管理命令，例如 `sudo apt install tesseract-ocr`。
- Poppler 相关 Windows 行为保持既有内置策略，不因 OCR 调整重新暴露为手动安装项。

## Linux 行为

- Linux 依赖体检继续展示 OCR 所需系统包。
- Linux OCR 提示继续包含 `tesseract-ocr`、`tesseract-ocr-chi-sim` 和 `poppler-utils`。
- Linux 一键安装或手动安装提示不受 Windows 内置策略影响。

## 验收契约

- 在未安装系统级 Tesseract 的 Windows 机器上，正常安装 pinvou 后打开依赖体检页，不应看到“手动安装 Tesseract”的阻断提示。
- 人为删除安装目录下的 OCR runtime 后，体检或上传失败提示应能让用户定位到修复安装。
- 在 Linux 环境中运行依赖体检时，OCR 依赖提示保持现状。
