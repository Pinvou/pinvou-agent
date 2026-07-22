# Tauri 平台配置

`tauri.conf.json` 只保存跨平台公共配置。本目录保存平台 overlay：

- `tauri.windows.conf.json`：Windows MSI/NSIS 安装器配置。
- `tauri.linux.conf.json`：Linux DEB 和系统依赖配置。
- `tauri.macos.conf.json`：macOS DMG 配置入口。
- `tauri.wosign.conf.json`：Windows 发布签名 overlay。
- `runtime/`：按目标平台锁定私有运行时版本，不存放制品本身。

`scripts/tauri-build-with-secrets.js` 根据当前操作系统显式加载对应 overlay，禁止在公共配置中增加平台专属安装器路径。
