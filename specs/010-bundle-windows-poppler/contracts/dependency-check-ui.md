# 契约：依赖体检 UI

## 目标

定义 Windows 内置 Poppler 后，依赖体检页对 Poppler/PDF 文本提取能力的展示行为。

## Windows 行为

- 不展示 Poppler 作为用户需要手动补全的依赖。
- 不展示 `poppler-utils`、`pdftotext`、`pdftoppm` 的手动安装提示。
- 如果其他依赖缺失，例如 Tesseract、Pandoc、LibreOffice、7-Zip、邮件解析工具，仍按原规则展示。
- 如果 PDF 上传因内置 Poppler 缺失失败，错误入口在附件上传/PDF 解析流程，不通过依赖体检引导用户安装 Poppler。

## Linux 行为

- 保持现有 Poppler/PDF 文本提取依赖检查。
- 仍可提示 `poppler-utils` 等系统包安装建议。

## 验收契约

- Windows 安装版依赖体检列表中不存在“PDF 文本提取缺失且需要安装 Poppler”的状态。
- Linux 环境依赖体检仍能报告 Poppler 缺失。
- 移除 Windows Poppler 项后，其他缺失依赖项数量和提示不被错误清空。
