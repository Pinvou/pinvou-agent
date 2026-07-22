# 平台打包目录

此目录只保存可审查的安装器模板、签名脚本和平台打包元数据：

- `windows/`：WiX、NSIS、WoSign 和 Windows runtime 打包说明。
- `linux/`：desktop 文件以及 DEB 安装/卸载脚本。
- `macos/`：后续放置 entitlements、公证和 DMG 定制文件。

大型或私有运行时不得提交到这里；它们由独立私有仓库、lock manifest 和构建期 staging 提供。
