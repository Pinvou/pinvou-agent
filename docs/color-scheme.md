# 主题偏好

设置 → 通用 → 主题模式提供「跟随系统」「浅色」「深色」。新安装默认跟随操作系统；无法检测系统偏好时使用浅色。跟随系统会实时响应系统深浅色切换，主窗口、独立窗口和文件阅读器共用解析规则。

桌面端以 `settings.json` 的 `color_scheme` 为主题偏好。升级时，只有缺少该字段的旧设置会从 `theme` 推导：`liquid-light` 对应浅色，`genesis` / `liquid-dark` 对应深色，并在首次正常加载时落盘；已保存的 `system` / `light` / `dark` 不会重新推导。损坏设置沿用现有不覆盖原文件的保护。

Web 端保留 `localStorage` 的 `pinvou.web.theme` 键，兼容旧版保存的 `light` / `dark`；没有有效偏好或存储不可用时跟随系统。桌面端手动选择时同时写入旧 `theme` 字段的当前解析值，供降级后的旧客户端读取；系统后续切换不反复写入设置。

验证入口：`npm --prefix pinvou3-app run test:color-scheme`、`test:reader-ui`、`test:settings-ui`，以及 Rust prefs 中的主题迁移测试。

实现见 `pinvou3-app/src/shared/color-scheme.js` 与 `pinvou3-app/src-tauri/src/platform/prefs/mod.rs`（`color_scheme` 迁移与落盘）。
