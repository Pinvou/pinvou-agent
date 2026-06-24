# 契约：依赖体检 UI

## 目标

定义 Windows 内置 Pandoc 后，依赖体检页对 Pandoc/现代文档解析能力的展示行为。

## Windows 行为

- 不展示 Pandoc 作为用户需要手动补全的依赖。
- 不展示 `pandoc` 的手动安装提示。
- 如果其他依赖缺失，例如 Poppler、Tesseract、LibreOffice、7-Zip、邮件解析工具，仍按原规则展示。
- 如果文档上传因内置 Pandoc 缺失失败，错误入口在附件上传/文档解析流程，不通过依赖体检引导用户安装 Pandoc。

## Linux 行为

- 保持现有 Pandoc/现代文档解析依赖检查。
- 仍可提示 `pandoc` 等系统包安装建议。

## 验收契约

- Windows 安装版依赖体检列表中不存在“现代文档解析缺失且需要安装 Pandoc”的状态。
- Linux 环境依赖体检仍能报告 Pandoc 缺失。
- 移除 Windows Pandoc 项后，其他缺失依赖项数量和提示不被错误清空。
