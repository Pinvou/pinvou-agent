# 契约：Windows Pandoc 运行时

## 目标

定义 Windows 安装版 pinvou 对内置 Pandoc 的文件位置、可发现性和错误行为要求。

## 安装契约

- MSI 安装完成后，应用安装目录下必须存在 `pandoc` 文件夹。
- `pandoc` 文件夹必须至少包含：
  - `pandoc.exe`
  - Pandoc 发行包随附许可/版权文件
- 安装目录可以包含空格或中文字符，应用仍必须能定位并执行内置 Pandoc。
- 升级和重新安装不得删除或遗漏 `pandoc` 文件夹。

## 运行时契约

- Windows 下执行依赖 Pandoc 的文档解析时，应用必须优先使用 `{安装目录}/pandoc/pandoc.exe`。
- 如果内置路径不存在，应用可以降级探测系统 PATH，但用户可见错误不得要求用户手动安装 Pandoc。
- 如果内置 Pandoc 缺失或不可执行，用户可见错误必须说明安装内容异常或建议修复安装。
- 内置 Pandoc 的进程可发现性应覆盖当前应用进程，并与安装器 PATH 配置保持一致。

## 验收契约

- 干净 Windows 环境未安装 Pandoc 时，安装 MSI 后上传依赖 Pandoc 的支持文档应成功解析。
- 删除 `{安装目录}/pandoc/pandoc.exe` 后上传相关文档，应出现安装内容异常提示。
- 系统 PATH 中存在其他 Pandoc 版本时，应用应优先使用安装目录内置版本。
