# Quickstart：我要反馈

## 前置条件

- 当前 feature 指针为 `specs/014-user-feedback`。
- 开发环境可运行 `pinvou3-app/run-dev.sh`。
- Windows 验证环境能访问 H3CLogCollector 既有上传通道。

## 开发入口

1. 在 `pinvou3-app/src/index.html` 的 `SettingsView` 中新增“帮助与反馈”区域和“我要反馈”按钮。
2. 在 `pinvou3-app/src/tauri-bridge.js` 中新增 `submitFeedback`，调用 Tauri 命令 `submit_feedback`。
3. 在 `pinvou3-app/src-tauri/src/feedback.rs` 中实现反馈校验、反馈包生成、H3C 兼容打包、XOR 和上传。
4. 在 `pinvou3-app/src-tauri/src/commands.rs` 暴露 `submit_feedback` 命令，并在 `lib.rs` 的 `generate_handler!` 注册。
5. 在 `pinvou3-app/src-tauri/src/bridge/paths.rs` 增加反馈目录 helper。

## 本地运行

```bash
./pinvou3-app/run-dev.sh
```

Windows PowerShell 如需直接检查后端：

```powershell
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

## 验证命令

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml feedback --lib
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

## 手动验收

1. 打开 app，进入设置页，确认能看到“我要反馈”入口。
2. 不填写说明直接提交，应提示补充内容。
3. 填写文字反馈，不加附件，提交后应显示成功或可重试失败。
4. 添加支持格式图片，附件列表应显示文件名、大小和删除按钮。
5. 添加超限视频或不支持文件，应阻止提交并保留已填写文字。
6. 模拟断网或接收通道失败，应展示可重试提示，并保留草稿。
7. 成功提交后，检查 `~/.pinvou3/feedback/` 下仅保留必要回执，不保留已成功上传的原始附件副本。

## 隐私检查

- 反馈包内不得出现聊天正文。
- 反馈包内不得出现用户文件正文，除非用户明确选为反馈附件。
- `manifest.json` 不得包含完整原始附件绝对路径。
- 不得包含模型 API key、搜索 API key 或 settings 中的敏感配置。

## H3C 兼容检查

1. 反馈目录可以生成 `tar.gz`。
2. `.dbg` 字节等于 `tar.gz` 每个字节 XOR `0x55`。
3. `checkCode` 与 H3CLogCollector 相同输入下结果一致。
4. 上传请求包含 `GwSn`、`FileName`、`checkCode` 和 `application/octet-stream`。
5. `retCode = 0` 时前端显示成功回执。

## 执行记录

- 执行日期：2026-06-24。
- 执行环境：Windows PowerShell，工作区 `E:\Pinvou\pinvou3 - 副本`。
- 后端检查：`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 通过；仅有既有 warning。
- 后端测试：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml feedback --lib` 通过，9 passed。
- 前端语法检查：使用本地 `pinvou3-app/src/vendor/babel.min.js` 转译 `pinvou3-app/src/index.html` 中的 `text/babel` 主脚本，通过。
- 契约检查：反馈 Tauri 命令字段、H3C 上传 URL、请求头、XOR `0x55`、`checkCode` 与契约一致。
- 隐私检查：`manifest.json` 只写入用户填写的反馈说明、白名单环境摘要、附件安全包内名和哈希；不写入附件原始绝对路径、聊天正文、模型 API key 或搜索 API key。
- 底座边界：`git status --short DeepSeek-TUI` 无输出，未修改 `DeepSeek-TUI/`。
- 未覆盖项：尚未启动 Tauri 桌面 app 执行完整手工 smoke，包括设置页真实点击、系统文件选择器、断网失败、真实 H3C 通道成功回执和成功后目录清理。
